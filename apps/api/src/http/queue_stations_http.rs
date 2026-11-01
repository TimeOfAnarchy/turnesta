use super::{
    AppState,
    api::{ApiError, SignedIn, check_mutation},
};
use crate::{
    core::{Name, TenantId},
    queue::QueueError,
    queue_stations::{self, Station, StationDraft},
};
use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode},
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::{future::Future, time::Duration};
use utoipa::ToSchema;
use uuid::Uuid;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/organizations/{tenant_id}/queues/{queue_id}/stations",
            get(stations).post(create),
        )
        .route(
            "/api/organizations/{tenant_id}/queues/{queue_id}/stations/{station_id}",
            axum::routing::patch(update),
        )
}
async fn bounded<T>(future: impl Future<Output = Result<T, QueueError>>) -> Result<T, ApiError> {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .map_err(|_| ApiError::unavailable())?
        .map_err(Into::into)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Pagination {
    after: Option<Uuid>,
    limit: Option<i32>,
}
impl Pagination {
    fn limit(&self) -> Result<i32, ApiError> {
        let value = self.limit.unwrap_or(50);
        if !(1..=100).contains(&value) {
            return Err(ApiError::bad_request());
        }
        Ok(value)
    }
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct StationInput {
    name: String,
    enabled: bool,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct UpdateStationInput {
    name: String,
    enabled: bool,
    expected_version: i32,
}
#[derive(Serialize, ToSchema)]
pub(super) struct StationPage {
    items: Vec<Station>,
    next_cursor: Option<Uuid>,
}
#[utoipa::path(get,path="/api/organizations/{tenant_id}/queues/{queue_id}/stations",tag="queue",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("queue_id"=Uuid,Path),("after"=Option<Uuid>,Query),("limit"=Option<i32>,Query)),responses((status=200,body=StationPage)))]
pub(super) async fn stations(
    State(state): State<AppState>,
    signed: SignedIn,
    path: Result<Path<(TenantId, Uuid)>, PathRejection>,
    query: Result<Query<Pagination>, QueryRejection>,
) -> Result<Json<StationPage>, ApiError> {
    let Path((tenant, queue)) = path.map_err(|_| ApiError::bad_request())?;
    let Query(query) = query.map_err(|_| ApiError::bad_request())?;
    let limit = query.limit()? as usize;
    let mut items = bounded(queue_stations::list(
        &state.pool,
        signed.user_id,
        tenant,
        queue,
        query.after,
        limit as i64,
    ))
    .await?;
    let next_cursor = if items.len() > limit {
        items.truncate(limit);
        items.last().map(|s| s.id)
    } else {
        None
    };
    Ok(Json(StationPage { items, next_cursor }))
}
#[utoipa::path(post,path="/api/organizations/{tenant_id}/queues/{queue_id}/stations",tag="queue",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("queue_id"=Uuid,Path)),request_body=StationInput,responses((status=201,body=Station)))]
pub(super) async fn create(
    State(state): State<AppState>,
    signed: SignedIn,
    headers: HeaderMap,
    path: Result<Path<(TenantId, Uuid)>, PathRejection>,
    body: Result<Json<StationInput>, JsonRejection>,
) -> Result<(StatusCode, Json<Station>), ApiError> {
    check_mutation(&headers, &state.auth, &signed)?;
    let Path((tenant, queue)) = path.map_err(|_| ApiError::bad_request())?;
    let Json(body) = body.map_err(|_| ApiError::bad_request())?;
    let name = Name::parse(&body.name)?;
    let result = bounded(queue_stations::save(
        &state.pool,
        signed.user_id,
        tenant,
        queue,
        None,
        StationDraft {
            name: &name,
            enabled: body.enabled,
        },
    ))
    .await?;
    Ok((StatusCode::CREATED, Json(result)))
}
#[utoipa::path(patch,path="/api/organizations/{tenant_id}/queues/{queue_id}/stations/{station_id}",tag="queue",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("queue_id"=Uuid,Path),("station_id"=Uuid,Path)),request_body=UpdateStationInput,responses((status=200,body=Station),(status=409,description="Version conflict")))]
pub(super) async fn update(
    State(state): State<AppState>,
    signed: SignedIn,
    headers: HeaderMap,
    path: Result<Path<(TenantId, Uuid, Uuid)>, PathRejection>,
    body: Result<Json<UpdateStationInput>, JsonRejection>,
) -> Result<Json<Station>, ApiError> {
    check_mutation(&headers, &state.auth, &signed)?;
    let Path((tenant, queue, id)) = path.map_err(|_| ApiError::bad_request())?;
    let Json(body) = body.map_err(|_| ApiError::bad_request())?;
    let name = Name::parse(&body.name)?;
    Ok(Json(
        bounded(queue_stations::save(
            &state.pool,
            signed.user_id,
            tenant,
            queue,
            Some((id, body.expected_version)),
            StationDraft {
                name: &name,
                enabled: body.enabled,
            },
        ))
        .await?,
    ))
}
