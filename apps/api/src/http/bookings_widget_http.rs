use super::{
    AppState, WidgetFrameAncestor,
    api::{ApiError, SignedIn, check_mutation},
    queue_public_http::permit,
};
use crate::{
    bookings_public,
    bookings_widget::{self, WidgetConfigure, WidgetError, WidgetSettings},
    core::{LocationId, TenantId},
};
use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header::CONTENT_TYPE},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::{future::Future, time::Duration};
use utoipa::ToSchema;
use uuid::Uuid;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/organizations/{tenant_id}/locations/{location_id}/bookings/widget",
            get(settings).put(configure),
        )
        .route("/embed/book/{slug}", get(page))
        .route(
            "/assets/booking-widget.js",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../web/booking-widget.js"),
                )
            }),
        )
        .route(
            "/assets/booking-widget-protocol.js",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../web/booking-widget-protocol.mjs"),
                )
            }),
        )
        .route(
            "/assets/bookings-embed.js",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../web/bookings-embed.js"),
                )
            }),
        )
}
impl From<WidgetError> for ApiError {
    fn from(error: WidgetError) -> Self {
        match error {
            WidgetError::Core(e) => e.into(),
            WidgetError::Database(e) => e.into(),
            WidgetError::NotFound => Self::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "Booking point not found",
            ),
            WidgetError::InvalidOrigins => Self::new(
                StatusCode::BAD_REQUEST,
                "invalid_widget_origins",
                "Use exact HTTPS origins; HTTP only on loopback",
            ),
        }
    }
}
async fn bounded<T>(future: impl Future<Output = Result<T, WidgetError>>) -> Result<T, ApiError> {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .map_err(|_| ApiError::unavailable())?
        .map_err(Into::into)
}
#[derive(Serialize, ToSchema)]
pub(super) struct WidgetResponse {
    #[serde(flatten)]
    settings: WidgetSettings,
    public_url: Option<String>,
    script_url: String,
}
async fn response(
    state: &AppState,
    user: crate::core::UserId,
    tenant: TenantId,
    point: LocationId,
    settings: WidgetSettings,
) -> Result<Json<WidgetResponse>, ApiError> {
    let public = tokio::time::timeout(
        Duration::from_secs(5),
        bookings_public::settings(&state.pool, user, tenant, point),
    )
    .await
    .map_err(|_| ApiError::unavailable())??;
    Ok(Json(WidgetResponse {
        settings,
        public_url: public
            .public_slug
            .map(|slug| format!("{}/book/{slug}", state.auth.public_origin)),
        script_url: format!("{}/assets/booking-widget.js", state.auth.public_origin),
    }))
}
#[utoipa::path(get,path="/api/organizations/{tenant_id}/locations/{location_id}/bookings/widget",tag="bookings",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("location_id"=Uuid,Path)),responses((status=200,body=WidgetResponse)))]
pub(super) async fn settings(
    State(state): State<AppState>,
    signed: SignedIn,
    path: Result<Path<(TenantId, LocationId)>, PathRejection>,
) -> Result<Json<WidgetResponse>, ApiError> {
    let Path((tenant, point)) = path.map_err(|_| ApiError::bad_request())?;
    let settings = bounded(bookings_widget::settings(
        &state.pool,
        signed.user_id,
        tenant,
        point,
    ))
    .await?;
    response(&state, signed.user_id, tenant, point, settings).await
}
#[utoipa::path(put,path="/api/organizations/{tenant_id}/locations/{location_id}/bookings/widget",tag="bookings",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("location_id"=Uuid,Path)),request_body=WidgetConfigure,responses((status=200,body=WidgetResponse)))]
pub(super) async fn configure(
    State(state): State<AppState>,
    signed: SignedIn,
    headers: HeaderMap,
    path: Result<Path<(TenantId, LocationId)>, PathRejection>,
    body: Result<Json<WidgetConfigure>, JsonRejection>,
) -> Result<Json<WidgetResponse>, ApiError> {
    check_mutation(&headers, &state.auth, &signed)?;
    let Path((tenant, point)) = path.map_err(|_| ApiError::bad_request())?;
    let Json(input) = body.map_err(|_| ApiError::bad_request())?;
    let settings = bounded(bookings_widget::configure(
        &state.pool,
        signed.user_id,
        tenant,
        point,
        &state.auth.public_origin,
        input,
    ))
    .await?;
    response(&state, signed.user_id, tenant, point, settings).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmbedQuery {
    parent_origin: String,
    channel: Uuid,
}
async fn page(
    State(state): State<AppState>,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<EmbedQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let _permit = permit(&state)?;
    let Path(slug) = path.map_err(|_| ApiError::bad_request())?;
    let Query(q) = query.map_err(|_| ApiError::bad_request())?;
    if !bounded(bookings_widget::permits(
        &state.pool,
        slug,
        &q.parent_origin,
        &state.auth.public_origin,
    ))
    .await?
    {
        return Err(ApiError::forbidden());
    }
    let profile = tokio::time::timeout(
        Duration::from_secs(5),
        bookings_public::profile(&state.pool, slug),
    )
    .await
    .map_err(|_| ApiError::unavailable())??;
    let policy=HeaderValue::from_str(&format!("default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self'; base-uri 'none'; frame-ancestors {}; form-action 'self'",q.parent_origin)).map_err(|_|ApiError::unavailable())?;
    let html = super::bookings_public_http::render_page(
        slug,
        Some(profile),
        q.parent_origin,
        q.channel.to_string(),
    )?;
    let mut response = Html(html).into_response();
    response
        .extensions_mut()
        .insert(WidgetFrameAncestor(policy));
    Ok(response)
}
