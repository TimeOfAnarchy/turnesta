use super::{
    AppState,
    api::{ApiError, SignedIn, check_mutation},
    queue_public_http,
};
use crate::{
    catalog::{self, CatalogError, Service, ServiceDraft},
    core::{Name, TenantId},
    queue_public::{self, PublicService},
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
            "/api/organizations/{tenant_id}/queues/{queue_id}/services",
            get(services).post(create),
        )
        .route(
            "/api/organizations/{tenant_id}/queues/{queue_id}/services/{service_id}",
            axum::routing::patch(update),
        )
        .route("/api/public/queues/{slug}/services", get(public_services))
}
impl From<CatalogError> for ApiError {
    fn from(error: CatalogError) -> Self {
        match error {
            CatalogError::Core(error) => error.into(),
            CatalogError::Database(error) => error.into(),
            CatalogError::NotFound => Self::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "Queue or service not found",
            ),
            CatalogError::InvalidDescription => Self::new(
                StatusCode::BAD_REQUEST,
                "invalid_description",
                "Description must be at most 500 characters without controls",
            ),
        }
    }
}
async fn bounded<T>(future: impl Future<Output = Result<T, CatalogError>>) -> Result<T, ApiError> {
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
pub(super) struct ServiceInput {
    name: String,
    description: String,
    enabled: bool,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct UpdateInput {
    name: String,
    description: String,
    enabled: bool,
    expected_version: i32,
}
#[derive(Serialize, ToSchema)]
pub(super) struct ServicePage {
    items: Vec<Service>,
    next_cursor: Option<Uuid>,
}
#[derive(Serialize, ToSchema)]
pub(super) struct PublicServicePage {
    items: Vec<PublicService>,
    next_cursor: Option<Uuid>,
}

#[utoipa::path(get,path="/api/organizations/{tenant_id}/queues/{queue_id}/services",tag="catalog",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("queue_id"=Uuid,Path),("after"=Option<Uuid>,Query),("limit"=Option<i32>,Query)),responses((status=200,body=ServicePage)))]
pub(super) async fn services(
    State(state): State<AppState>,
    signed: SignedIn,
    path: Result<Path<(TenantId, Uuid)>, PathRejection>,
    query: Result<Query<Pagination>, QueryRejection>,
) -> Result<Json<ServicePage>, ApiError> {
    let Path((tenant, queue)) = path.map_err(|_| ApiError::bad_request())?;
    let Query(query) = query.map_err(|_| ApiError::bad_request())?;
    let limit = query.limit()? as usize;
    let mut items = bounded(catalog::list(
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
    Ok(Json(ServicePage { items, next_cursor }))
}
#[utoipa::path(post,path="/api/organizations/{tenant_id}/queues/{queue_id}/services",tag="catalog",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("queue_id"=Uuid,Path)),request_body=ServiceInput,responses((status=201,body=Service)))]
pub(super) async fn create(
    State(state): State<AppState>,
    signed: SignedIn,
    headers: HeaderMap,
    path: Result<Path<(TenantId, Uuid)>, PathRejection>,
    body: Result<Json<ServiceInput>, JsonRejection>,
) -> Result<(StatusCode, Json<Service>), ApiError> {
    check_mutation(&headers, &state.auth, &signed)?;
    let Path((tenant, queue)) = path.map_err(|_| ApiError::bad_request())?;
    let Json(body) = body.map_err(|_| ApiError::bad_request())?;
    let name = Name::parse(&body.name)?;
    let result = bounded(catalog::save(
        &state.pool,
        signed.user_id,
        tenant,
        queue,
        None,
        ServiceDraft {
            name: &name,
            description: &body.description,
            enabled: body.enabled,
        },
    ))
    .await?;
    Ok((StatusCode::CREATED, Json(result)))
}
#[utoipa::path(patch,path="/api/organizations/{tenant_id}/queues/{queue_id}/services/{service_id}",tag="catalog",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("queue_id"=Uuid,Path),("service_id"=Uuid,Path)),request_body=UpdateInput,responses((status=200,body=Service),(status=409,description="Version conflict")))]
pub(super) async fn update(
    State(state): State<AppState>,
    signed: SignedIn,
    headers: HeaderMap,
    path: Result<Path<(TenantId, Uuid, Uuid)>, PathRejection>,
    body: Result<Json<UpdateInput>, JsonRejection>,
) -> Result<Json<Service>, ApiError> {
    check_mutation(&headers, &state.auth, &signed)?;
    let Path((tenant, queue, id)) = path.map_err(|_| ApiError::bad_request())?;
    let Json(body) = body.map_err(|_| ApiError::bad_request())?;
    let name = Name::parse(&body.name)?;
    Ok(Json(
        bounded(catalog::save(
            &state.pool,
            signed.user_id,
            tenant,
            queue,
            Some((id, body.expected_version)),
            ServiceDraft {
                name: &name,
                description: &body.description,
                enabled: body.enabled,
            },
        ))
        .await?,
    ))
}
#[utoipa::path(get,path="/api/public/queues/{slug}/services",tag="queue-public",params(("slug"=Uuid,Path),("after"=Option<Uuid>,Query),("limit"=Option<i32>,Query)),responses((status=200,body=PublicServicePage),(status=404,description="Queue not published")))]
pub(super) async fn public_services(
    State(state): State<AppState>,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<Pagination>, QueryRejection>,
) -> Result<Json<PublicServicePage>, ApiError> {
    let _permit = queue_public_http::permit(&state)?;
    let Path(slug) = path.map_err(|_| ApiError::bad_request())?;
    let Query(query) = query.map_err(|_| ApiError::bad_request())?;
    let limit = query.limit()? as usize;
    let mut items = tokio::time::timeout(
        Duration::from_secs(5),
        queue_public::services(&state.pool, slug, query.after, limit as i32),
    )
    .await
    .map_err(|_| ApiError::unavailable())??;
    let next_cursor = if items.len() > limit {
        items.truncate(limit);
        items.last().map(|s| s.id)
    } else {
        None
    };
    Ok(Json(PublicServicePage { items, next_cursor }))
}
