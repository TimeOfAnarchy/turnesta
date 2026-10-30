use std::{future::Future, time::Duration};

use askama::Template;
use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, FromRequestParts, Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{
        HeaderMap, StatusCode,
        header::{CONTENT_DISPOSITION, CONTENT_TYPE},
        request::Parts,
    },
    response::Html,
    routing::get,
};
use serde::{Deserialize, Serialize};
use tokio::sync::SemaphorePermit;
use utoipa::ToSchema;
use uuid::Uuid;

use super::{
    AppState,
    api::{ApiError, SignedIn, check_mutation, check_origin, single_header},
};
use crate::{
    auth::token::SecretToken,
    core::TenantId,
    queue_public::{
        self, PublicDisplay, PublicProfile, PublicQueueError, PublicSettings, PublicTicket,
    },
};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/organizations/{tenant_id}/queues/{queue_id}/public-access",
            get(settings).put(configure),
        )
        .route(
            "/api/organizations/{tenant_id}/queues/{queue_id}/public-access/qr.svg",
            get(qr),
        )
        .route(
            "/api/organizations/{tenant_id}/queues/{queue_id}/public-access/display",
            axum::routing::put(configure_display),
        )
        .route("/display/{slug}", get(display_page))
        .route("/api/public/queues/{slug}/display", get(display))
        .route(
            "/assets/queue-display.css",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../../web/queue-display.css"),
                )
            }),
        )
        .route(
            "/assets/queue-display.js",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../web/queue-display.js"),
                )
            }),
        )
        .route(
            "/assets/queue-display-client.js",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../web/queue-display-client.mjs"),
                )
            }),
        )
        .route("/q/{slug}", get(page))
        .route("/api/public/queues/{slug}", get(profile))
        .route(
            "/api/public/queues/{slug}/ticket",
            get(ticket).post(issue).layer(DefaultBodyLimit::max(1024)),
        )
        .route(
            "/assets/queue-guest.css",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../../web/queue-guest.css"),
                )
            }),
        )
        .route(
            "/assets/queue-guest.js",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../web/queue-guest.js"),
                )
            }),
        )
        .route(
            "/assets/queue-client.js",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../web/queue-client.mjs"),
                )
            }),
        )
}

impl From<PublicQueueError> for ApiError {
    fn from(error: PublicQueueError) -> Self {
        match error {
            PublicQueueError::Core(error) => error.into(),
            PublicQueueError::Database(error) => error.into(),
            PublicQueueError::NotFound => Self::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "Queue or ticket not found",
            ),
            PublicQueueError::InvalidToken => Self::bad_request(),
            PublicQueueError::Expired => Self::new(
                StatusCode::GONE,
                "ticket_expired",
                "Public ticket access expired",
            ),
            PublicQueueError::Closed => Self::new(
                StatusCode::CONFLICT,
                "queue_closed",
                "Public issuance is paused",
            ),
            PublicQueueError::RateLimit => Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                "queue_rate_limit",
                "Please retry later",
            ),
            PublicQueueError::Capacity => Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                "queue_capacity",
                "Queue waiting capacity reached",
            ),
            PublicQueueError::ServiceUnavailable => Self::new(
                StatusCode::CONFLICT,
                "service_unavailable",
                "Selected service is unavailable",
            ),
        }
    }
}

async fn bounded<T>(
    future: impl Future<Output = Result<T, PublicQueueError>>,
) -> Result<T, ApiError> {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .map_err(|_| ApiError::unavailable())?
        .map_err(Into::into)
}

pub(super) fn permit(state: &AppState) -> Result<SemaphorePermit<'_>, ApiError> {
    state.public_gate.try_acquire().map_err(|_| {
        ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_requests",
            "Please retry later",
        )
    })
}

pub(super) struct GuestTicketAccess([u8; 32]);
impl FromRequestParts<AppState> for GuestTicketAccess {
    type Rejection = ApiError;
    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if !parts.headers.contains_key("x-queue-ticket") {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "ticket_token_required",
                "Ticket token required",
            ));
        }
        let token = SecretToken::parse(
            single_header(&parts.headers, "x-queue-ticket").map_err(|_| ApiError::bad_request())?,
        )
        .map_err(|_| ApiError::bad_request())?;
        Ok(Self(token.hash()))
    }
}

#[derive(Serialize, ToSchema)]
pub(super) struct SettingsResponse {
    public_url: Option<String>,
    qr_url: Option<String>,
    join_open: bool,
    display_enabled: bool,
    display_url: Option<String>,
    version: i32,
}

fn links(
    state: &AppState,
    tenant: TenantId,
    queue: Uuid,
    settings: PublicSettings,
) -> SettingsResponse {
    SettingsResponse {
        public_url: settings
            .public_slug
            .map(|slug| format!("{}/q/{slug}", state.auth.public_origin)),
        qr_url: settings
            .public_slug
            .map(|_| format!("/api/organizations/{tenant}/queues/{queue}/public-access/qr.svg")),
        join_open: settings.join_open,
        display_enabled: settings.display_enabled,
        display_url: settings
            .public_slug
            .filter(|_| settings.display_enabled)
            .map(|slug| format!("{}/display/{slug}", state.auth.public_origin)),
        version: settings.version,
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ConfigureRequest {
    join_open: bool,
    expected_version: i32,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ConfigureDisplayRequest {
    enabled: bool,
    expected_version: i32,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DisplayQuery {
    #[serde(default)]
    page: i32,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct IssueRequest {
    service_id: Option<Uuid>,
}

#[utoipa::path(get, path="/api/organizations/{tenant_id}/queues/{queue_id}/public-access", tag="queue", security(("sessionCookie"=[])), params(("tenant_id"=Uuid,Path),("queue_id"=Uuid,Path)), responses((status=200,body=SettingsResponse)))]
pub(super) async fn settings(
    State(state): State<AppState>,
    signed: SignedIn,
    path: Result<Path<(TenantId, Uuid)>, PathRejection>,
) -> Result<Json<SettingsResponse>, ApiError> {
    let Path((tenant, queue)) = path.map_err(|_| ApiError::bad_request())?;
    Ok(Json(links(
        &state,
        tenant,
        queue,
        bounded(queue_public::settings(
            &state.pool,
            signed.user_id,
            tenant,
            queue,
        ))
        .await?,
    )))
}

#[utoipa::path(put, path="/api/organizations/{tenant_id}/queues/{queue_id}/public-access", tag="queue", security(("sessionCookie"=[])), params(("tenant_id"=Uuid,Path),("queue_id"=Uuid,Path)), request_body=ConfigureRequest, responses((status=200,body=SettingsResponse),(status=409,description="Settings changed; reload current version")))]
pub(super) async fn configure(
    State(state): State<AppState>,
    signed: SignedIn,
    headers: HeaderMap,
    path: Result<Path<(TenantId, Uuid)>, PathRejection>,
    body: Result<Json<ConfigureRequest>, JsonRejection>,
) -> Result<Json<SettingsResponse>, ApiError> {
    check_mutation(&headers, &state.auth, &signed)?;
    let Path((tenant, queue)) = path.map_err(|_| ApiError::bad_request())?;
    let Json(body) = body.map_err(|_| ApiError::bad_request())?;
    let settings = bounded(queue_public::configure(
        &state.pool,
        signed.user_id,
        tenant,
        queue,
        body.join_open,
        body.expected_version,
    ))
    .await?;
    Ok(Json(links(&state, tenant, queue, settings)))
}

#[utoipa::path(put, path="/api/organizations/{tenant_id}/queues/{queue_id}/public-access/display", tag="queue", security(("sessionCookie"=[])), params(("tenant_id"=Uuid,Path),("queue_id"=Uuid,Path)), request_body=ConfigureDisplayRequest, responses((status=200,body=SettingsResponse),(status=409,description="Settings changed; reload current version")))]
pub(super) async fn configure_display(
    State(state): State<AppState>,
    signed: SignedIn,
    headers: HeaderMap,
    path: Result<Path<(TenantId, Uuid)>, PathRejection>,
    body: Result<Json<ConfigureDisplayRequest>, JsonRejection>,
) -> Result<Json<SettingsResponse>, ApiError> {
    check_mutation(&headers, &state.auth, &signed)?;
    let Path((tenant, queue)) = path.map_err(|_| ApiError::bad_request())?;
    let Json(body) = body.map_err(|_| ApiError::bad_request())?;
    let settings = bounded(queue_public::configure_display(
        &state.pool,
        signed.user_id,
        tenant,
        queue,
        body.enabled,
        body.expected_version,
    ))
    .await?;
    Ok(Json(links(&state, tenant, queue, settings)))
}

#[utoipa::path(get, path="/api/public/queues/{slug}/display", tag="queue-public", params(("slug"=Uuid,Path),("page"=Option<i32>,Query,description="Zero-based page, 100 active calls per page")), responses((status=200,body=PublicDisplay),(status=404,description="Display not published")))]
pub(super) async fn display(
    State(state): State<AppState>,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<DisplayQuery>, QueryRejection>,
) -> Result<Json<PublicDisplay>, ApiError> {
    let _permit = permit(&state)?;
    let Path(slug) = path.map_err(|_| ApiError::bad_request())?;
    let Query(query) = query.map_err(|_| ApiError::bad_request())?;
    Ok(Json(
        bounded(queue_public::display(&state.pool, slug, query.page)).await?,
    ))
}

#[derive(Template)]
#[template(path = "queue_display.html")]
struct DisplayTemplate<'a> {
    slug: Uuid,
    profile: Option<&'a PublicDisplay>,
}

async fn display_page(
    State(state): State<AppState>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<(StatusCode, Html<String>), ApiError> {
    let _permit = permit(&state)?;
    let Path(slug) = path.map_err(|_| ApiError::bad_request())?;
    let snapshot = match tokio::time::timeout(
        Duration::from_secs(5),
        queue_public::display(&state.pool, slug, 0),
    )
    .await
    {
        Ok(Ok(value)) => Some(value),
        Ok(Err(PublicQueueError::NotFound)) => None,
        Ok(Err(error)) => return Err(error.into()),
        Err(_) => return Err(ApiError::unavailable()),
    };
    let status = if snapshot.is_some() {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    };
    let html = DisplayTemplate {
        slug,
        profile: snapshot.as_ref(),
    }
    .render()
    .map_err(|_| ApiError::unavailable())?;
    Ok((status, Html(html)))
}

#[utoipa::path(get, path="/api/organizations/{tenant_id}/queues/{queue_id}/public-access/qr.svg", tag="queue", security(("sessionCookie"=[])), params(("tenant_id"=Uuid,Path),("queue_id"=Uuid,Path)), responses((status=200,description="SVG QR for the public queue URL",content_type="image/svg+xml")))]
pub(super) async fn qr(
    State(state): State<AppState>,
    signed: SignedIn,
    path: Result<Path<(TenantId, Uuid)>, PathRejection>,
) -> Result<([(axum::http::HeaderName, &'static str); 2], String), ApiError> {
    let Path((tenant, queue)) = path.map_err(|_| ApiError::bad_request())?;
    let settings = bounded(queue_public::settings(
        &state.pool,
        signed.user_id,
        tenant,
        queue,
    ))
    .await?;
    let slug = settings.public_slug.ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "Public queue has not been configured",
        )
    })?;
    let url = format!("{}/q/{slug}", state.auth.public_origin);
    let code = qrcode::QrCode::new(url.as_bytes()).map_err(|_| ApiError::unavailable())?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .build();
    Ok((
        [
            (CONTENT_TYPE, "image/svg+xml; charset=utf-8"),
            (CONTENT_DISPOSITION, "inline; filename=turnesta-queue.svg"),
        ],
        svg,
    ))
}

#[utoipa::path(get, path="/api/public/queues/{slug}", tag="queue-public", params(("slug"=Uuid,Path)), responses((status=200,body=PublicProfile),(status=404,description="Queue not published")))]
pub(super) async fn profile(
    State(state): State<AppState>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<PublicProfile>, ApiError> {
    let _permit = permit(&state)?;
    let Path(slug) = path.map_err(|_| ApiError::bad_request())?;
    Ok(Json(
        bounded(queue_public::profile(&state.pool, slug)).await?,
    ))
}

#[utoipa::path(get, path="/api/public/queues/{slug}/ticket", tag="queue-public", security(("queueTicket"=[])), params(("slug"=Uuid,Path)), responses((status=200,body=PublicTicket),(status=404,description="Ticket not found"),(status=410,description="Ticket access expired")))]
pub(super) async fn ticket(
    State(state): State<AppState>,
    access: GuestTicketAccess,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<PublicTicket>, ApiError> {
    let _permit = permit(&state)?;
    let Path(slug) = path.map_err(|_| ApiError::bad_request())?;
    let mut ticket = bounded(queue_public::ticket(&state.pool, slug, &access.0)).await?;
    if !state.payments.enabled() {
        ticket.tips = None;
    }
    Ok(Json(ticket))
}

#[utoipa::path(post, path="/api/public/queues/{slug}/ticket", tag="queue-public", security(("queueTicket"=[])), params(("slug"=Uuid,Path)), request_body=IssueRequest, responses((status=200,body=PublicTicket),(status=409,description="Issuance paused"),(status=429,description="Queue budget or waiting capacity exceeded")))]
pub(super) async fn issue(
    State(state): State<AppState>,
    access: GuestTicketAccess,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
    body: Result<Json<IssueRequest>, JsonRejection>,
) -> Result<Json<PublicTicket>, ApiError> {
    check_origin(&headers, &state.auth)?;
    let _permit = permit(&state)?;
    let Path(slug) = path.map_err(|_| ApiError::bad_request())?;
    let Json(body) = body.map_err(|_| ApiError::bad_request())?;
    let mut ticket = bounded(queue_public::issue_for_service(
        &state.pool,
        slug,
        &access.0,
        body.service_id,
    ))
    .await?;
    if !state.payments.enabled() {
        ticket.tips = None;
    }
    Ok(Json(ticket))
}

#[derive(Template)]
#[template(path = "queue_guest.html")]
struct GuestTemplate<'a> {
    profile: &'a PublicProfile,
}

async fn page(
    State(state): State<AppState>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Html<String>, ApiError> {
    let _permit = permit(&state)?;
    let Path(slug) = path.map_err(|_| ApiError::bad_request())?;
    let profile = bounded(queue_public::profile(&state.pool, slug)).await?;
    GuestTemplate { profile: &profile }
        .render()
        .map(Html)
        .map_err(|_| ApiError::unavailable())
}
