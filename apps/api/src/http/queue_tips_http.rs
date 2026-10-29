use std::time::Duration;

use axum::{
    Json, Router,
    extract::{
        Path, State,
        rejection::{JsonRejection, PathRejection},
    },
    http::HeaderMap,
    routing::get,
};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use super::{
    AppState,
    api::{ApiError, SignedIn, check_mutation},
    tips_http,
};
use crate::{
    core::TenantId,
    queue_tips::{self, TipSettings},
};

pub(super) fn routes() -> Router<AppState> {
    Router::new().route(
        "/api/organizations/{tenant_id}/queues/{queue_id}/tips",
        get(settings).put(configure),
    )
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ConfigureTipsRequest {
    // Required nullable field: omitting it must not silently disable the link.
    #[serde(deserialize_with = "required_recipient")]
    #[schema(required = true)]
    recipient_id: Option<Uuid>,
    expected_version: i32,
}

fn required_recipient<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Uuid>, D::Error> {
    Option::deserialize(deserializer)
}

#[utoipa::path(get, path="/api/organizations/{tenant_id}/queues/{queue_id}/tips", tag="queue", security(("sessionCookie"=[])), params(("tenant_id"=Uuid,Path),("queue_id"=Uuid,Path)), responses((status=200,body=TipSettings)))]
pub(super) async fn settings(
    State(state): State<AppState>,
    signed: SignedIn,
    path: Result<Path<(TenantId, Uuid)>, PathRejection>,
) -> Result<Json<TipSettings>, ApiError> {
    tips_http::enabled(&state)?;
    let Path((tenant, queue)) = path.map_err(|_| ApiError::bad_request())?;
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        queue_tips::settings(&state.pool, signed.user_id, tenant, queue),
    )
    .await
    .map_err(|_| ApiError::unavailable())??;
    Ok(Json(result))
}

#[utoipa::path(put, path="/api/organizations/{tenant_id}/queues/{queue_id}/tips", tag="queue", security(("sessionCookie"=[])), params(("tenant_id"=Uuid,Path),("queue_id"=Uuid,Path)), request_body=ConfigureTipsRequest, responses((status=200,body=TipSettings),(status=409,description="Settings changed; reload current version")))]
pub(super) async fn configure(
    State(state): State<AppState>,
    signed: SignedIn,
    headers: HeaderMap,
    path: Result<Path<(TenantId, Uuid)>, PathRejection>,
    body: Result<Json<ConfigureTipsRequest>, JsonRejection>,
) -> Result<Json<TipSettings>, ApiError> {
    check_mutation(&headers, &state.auth, &signed)?;
    tips_http::enabled(&state)?;
    let Path((tenant, queue)) = path.map_err(|_| ApiError::bad_request())?;
    let Json(body) = body.map_err(|_| ApiError::bad_request())?;
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        queue_tips::configure(
            &state.pool,
            signed.user_id,
            tenant,
            queue,
            body.recipient_id,
            body.expected_version,
        ),
    )
    .await
    .map_err(|_| ApiError::unavailable())??;
    Ok(Json(result))
}
