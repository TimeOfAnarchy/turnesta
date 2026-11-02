use super::{
    AppState,
    api::{ApiError, SignedIn, check_mutation, single_header},
};
use crate::{
    core::{TenantId, UserId},
    team::{self, TeamCommand, TeamError, TeamMember, TeamReceipt},
};
use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::{future::Future, time::Duration};
use utoipa::ToSchema;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/organizations/{tenant_id}/team", get(members))
        .route(
            "/api/organizations/{tenant_id}/team/commands",
            post(command),
        )
}
impl From<TeamError> for ApiError {
    fn from(error: TeamError) -> Self {
        match error {
            TeamError::Core(e) => e.into(),
            TeamError::Database(e) => e.into(),
            TeamError::Delivery => Self::unavailable(),
            TeamError::Invalid => Self::new(
                StatusCode::BAD_REQUEST,
                "invalid_team_command",
                "Check member details",
            ),
            TeamError::Idempotency => Self::new(
                StatusCode::CONFLICT,
                "idempotency_conflict",
                "Key reused for a different command",
            ),
            TeamError::NotFound => Self::new(
                StatusCode::NOT_FOUND,
                "team_member_unavailable",
                "Member or email unavailable",
            ),
            TeamError::RateLimited => Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                "team_rate_limited",
                "Wait before requesting another invitation",
            ),
            TeamError::Exists => Self::new(
                StatusCode::CONFLICT,
                "team_member_exists",
                "Member already exists; edit their access explicitly",
            ),
            TeamError::OwnerProtected => Self::new(
                StatusCode::CONFLICT,
                "team_owner_protected",
                "Owner membership cannot be changed here",
            ),
            TeamError::Version => Self::new(
                StatusCode::CONFLICT,
                "team_version_conflict",
                "Member changed; reload before saving",
            ),
        }
    }
}
async fn bounded<T>(future: impl Future<Output = Result<T, TeamError>>) -> Result<T, ApiError> {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .map_err(|_| ApiError::unavailable())?
        .map_err(Into::into)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TeamQuery {
    after: Option<UserId>,
    limit: Option<i32>,
}
#[derive(Serialize, ToSchema)]
pub(super) struct TeamPage {
    items: Vec<TeamMember>,
    next_cursor: Option<UserId>,
}
#[utoipa::path(get,path="/api/organizations/{tenant_id}/team",tag="team",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("after"=Option<Uuid>,Query),("limit"=Option<i32>,Query)),responses((status=200,body=TeamPage),(status=403,description="Owner required")))]
pub(super) async fn members(
    State(state): State<AppState>,
    signed: SignedIn,
    path: Result<Path<TenantId>, PathRejection>,
    query: Result<Query<TeamQuery>, QueryRejection>,
) -> Result<Json<TeamPage>, ApiError> {
    let Path(tenant) = path.map_err(|_| ApiError::bad_request())?;
    let Query(q) = query.map_err(|_| ApiError::bad_request())?;
    let limit = q.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::bad_request());
    }
    let mut items = bounded(team::members(
        &state.pool,
        signed.user_id,
        tenant,
        q.after,
        limit,
    ))
    .await?;
    let next_cursor = if items.len() > limit as usize {
        items.truncate(limit as usize);
        items.last().map(|m| m.user_id)
    } else {
        None
    };
    Ok(Json(TeamPage { items, next_cursor }))
}
#[utoipa::path(post,path="/api/organizations/{tenant_id}/team/commands",tag="team",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("Idempotency-Key"=Uuid,Header)),request_body=TeamCommand,responses((status=200,body=TeamReceipt),(status=409,description="Membership or idempotency conflict"),(status=429,description="Invitation cooldown or hourly limit")))]
pub(super) async fn command(
    State(state): State<AppState>,
    signed: SignedIn,
    headers: HeaderMap,
    path: Result<Path<TenantId>, PathRejection>,
    body: Result<Json<TeamCommand>, JsonRejection>,
) -> Result<Json<TeamReceipt>, ApiError> {
    check_mutation(&headers, &state.auth, &signed)?;
    let Path(tenant) = path.map_err(|_| ApiError::bad_request())?;
    let Json(command) = body.map_err(|_| ApiError::bad_request())?;
    let key = single_header(&headers, "idempotency-key")?
        .parse()
        .map_err(|_| ApiError::bad_request())?;
    Ok(Json(
        bounded(team::execute(
            &state.pool,
            signed.user_id,
            tenant,
            key,
            command,
            &state.auth.public_origin,
        ))
        .await?,
    ))
}
