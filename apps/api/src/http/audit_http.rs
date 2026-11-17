use super::{
    AppState,
    api::{ApiError, SignedIn, check_mutation},
};
use crate::{
    audit::{self, AuditEntry, AuditError, AuditQuery, AuditSettings},
    core::{TenantId, UserId},
};
use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::HeaderMap,
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::{future::Future, time::Duration};
use utoipa::ToSchema;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/organizations/{tenant_id}/audit/access", get(access))
        .route(
            "/api/organizations/{tenant_id}/audit/settings",
            get(settings).put(configure),
        )
}

impl From<AuditError> for ApiError {
    fn from(error: AuditError) -> Self {
        match error {
            AuditError::Core(error) => error.into(),
            AuditError::Invalid => Self::bad_request(),
            AuditError::Database(error) => {
                match error.as_database_error().and_then(|e| e.code()).as_deref() {
                    Some("42501") => Self::forbidden(),
                    Some("B2001") => Self::bad_request(),
                    _ => error.into(),
                }
            }
        }
    }
}

async fn bounded<T>(future: impl Future<Output = Result<T, AuditError>>) -> Result<T, ApiError> {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .map_err(|_| ApiError::unavailable())?
        .map_err(Into::into)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AccessQuery {
    from: Option<String>,
    to: Option<String>,
    actor: Option<UserId>,
    before_at: Option<String>,
    before_id: Option<i64>,
    limit: Option<i32>,
}

#[derive(Serialize, ToSchema)]
pub(super) struct AuditCursor {
    before_at: String,
    before_id: String,
}
#[derive(Serialize, ToSchema)]
pub(super) struct AuditPage {
    items: Vec<AuditEntry>,
    next_cursor: Option<AuditCursor>,
}

#[utoipa::path(get,path="/api/organizations/{tenant_id}/audit/access",tag="audit",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("from"=Option<String>,Query),("to"=Option<String>,Query),("actor"=Option<Uuid>,Query),("before_at"=Option<String>,Query),("before_id"=Option<i64>,Query),("limit"=Option<i32>,Query)),responses((status=200,body=AuditPage),(status=403,description="Owner required")))]
pub(super) async fn access(
    State(state): State<AppState>,
    signed: SignedIn,
    path: Result<Path<TenantId>, PathRejection>,
    query: Result<Query<AccessQuery>, QueryRejection>,
) -> Result<Json<AuditPage>, ApiError> {
    let Path(tenant) = path.map_err(|_| ApiError::bad_request())?;
    let Query(q) = query.map_err(|_| ApiError::bad_request())?;
    let limit = q.limit.unwrap_or(50);
    let mut items = bounded(audit::list(
        &state.pool,
        signed.user_id,
        tenant,
        AuditQuery {
            from: q.from.as_deref(),
            to: q.to.as_deref(),
            actor: q.actor,
            before_at: q.before_at.as_deref(),
            before_id: q.before_id,
            limit,
        },
    ))
    .await?;
    let next_cursor = if items.len() > limit as usize {
        items.truncate(limit as usize);
        items.last().map(|item| AuditCursor {
            before_at: item.created_at.clone(),
            before_id: item.id.clone(),
        })
    } else {
        None
    };
    Ok(Json(AuditPage { items, next_cursor }))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AuditSettingsInput {
    retention_days: i32,
    expected_version: i32,
}

#[utoipa::path(get,path="/api/organizations/{tenant_id}/audit/settings",tag="audit",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path)),responses((status=200,body=AuditSettings),(status=403,description="Owner required")))]
pub(super) async fn settings(
    State(state): State<AppState>,
    signed: SignedIn,
    path: Result<Path<TenantId>, PathRejection>,
) -> Result<Json<AuditSettings>, ApiError> {
    let Path(tenant) = path.map_err(|_| ApiError::bad_request())?;
    Ok(Json(
        bounded(audit::settings(&state.pool, signed.user_id, tenant)).await?,
    ))
}

#[utoipa::path(put,path="/api/organizations/{tenant_id}/audit/settings",tag="audit",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path)),request_body=AuditSettingsInput,responses((status=200,body=AuditSettings),(status=403,description="Owner required"),(status=409,description="Version conflict")))]
pub(super) async fn configure(
    State(state): State<AppState>,
    signed: SignedIn,
    headers: HeaderMap,
    path: Result<Path<TenantId>, PathRejection>,
    body: Result<Json<AuditSettingsInput>, JsonRejection>,
) -> Result<Json<AuditSettings>, ApiError> {
    check_mutation(&headers, &state.auth, &signed)?;
    let Path(tenant) = path.map_err(|_| ApiError::bad_request())?;
    let Json(body) = body.map_err(|_| ApiError::bad_request())?;
    Ok(Json(
        bounded(audit::configure(
            &state.pool,
            signed.user_id,
            tenant,
            body.retention_days,
            body.expected_version,
        ))
        .await?,
    ))
}
