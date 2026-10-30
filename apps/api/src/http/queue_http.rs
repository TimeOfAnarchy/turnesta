use std::{future::Future, time::Duration};

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
use utoipa::ToSchema;
use uuid::Uuid;

use super::{
    AppState,
    api::{ApiError, SignedIn, check_mutation, single_header},
};
use crate::{
    core::{LocationId, Name, TenantId},
    queue::{self, Command, CommandResult, Queue, QueueError, Ticket, TicketStatus},
};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/organizations/{tenant_id}/locations/{location_id}/queues",
            get(queues).post(create_queue),
        )
        .route(
            "/api/organizations/{tenant_id}/queues/{queue_id}",
            get(get_queue),
        )
        .route(
            "/api/organizations/{tenant_id}/queues/{queue_id}/tickets",
            get(tickets),
        )
        .route(
            "/api/organizations/{tenant_id}/queues/{queue_id}/commands",
            post(command),
        )
}

impl From<QueueError> for ApiError {
    fn from(error: QueueError) -> Self {
        match error {
            QueueError::Core(error) => error.into(),
            QueueError::Database(error) => error.into(),
            QueueError::NotFound => Self::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "Queue or ticket not found",
            ),
            QueueError::Forbidden => Self::forbidden(),
            QueueError::IdempotencyConflict => Self::new(
                StatusCode::CONFLICT,
                "idempotency_conflict",
                "The same key has a different queue command",
            ),
            QueueError::StateConflict => Self::new(
                StatusCode::CONFLICT,
                "state_conflict",
                "Ticket is not in the required state",
            ),
            QueueError::ActiveTicket => Self::new(
                StatusCode::CONFLICT,
                "active_ticket",
                "Complete or cancel your current ticket first",
            ),
            QueueError::ServiceUnavailable => Self::new(
                StatusCode::CONFLICT,
                "service_unavailable",
                "Selected service is unavailable",
            ),
        }
    }
}

async fn bounded<T>(future: impl Future<Output = Result<T, QueueError>>) -> Result<T, ApiError> {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .map_err(|_| ApiError::unavailable())?
        .map_err(Into::into)
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateQueueRequest {
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct QueuePagination {
    after: Option<Uuid>,
    limit: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TicketPagination {
    after: Option<i64>,
    limit: Option<i64>,
    status: Option<TicketStatus>,
    mine: Option<bool>,
}

fn limit(value: Option<i64>) -> Result<i64, ApiError> {
    let value = value.unwrap_or(50);
    if (1..=100).contains(&value) {
        Ok(value)
    } else {
        Err(ApiError::bad_request())
    }
}

#[derive(Serialize, ToSchema)]
pub(super) struct QueuePage {
    items: Vec<Queue>,
    next_cursor: Option<Uuid>,
}

#[derive(Serialize, ToSchema)]
pub(super) struct TicketList {
    items: Vec<Ticket>,
    next_cursor: Option<i64>,
}

#[utoipa::path(get, path="/api/organizations/{tenant_id}/locations/{location_id}/queues", tag="queue", security(("sessionCookie"=[])), params(("tenant_id"=Uuid,Path),("location_id"=Uuid,Path),("after"=Option<Uuid>,Query),("limit"=Option<i64>,Query)), responses((status=200,body=QueuePage),(status=404,description="Location not found")))]
pub(super) async fn queues(
    State(state): State<AppState>,
    signed: SignedIn,
    path: Result<Path<(TenantId, LocationId)>, PathRejection>,
    query: Result<Query<QueuePagination>, QueryRejection>,
) -> Result<Json<QueuePage>, ApiError> {
    let Path((tenant, location)) = path.map_err(|_| ApiError::bad_request())?;
    let Query(page) = query.map_err(|_| ApiError::bad_request())?;
    let limit = limit(page.limit)?;
    let mut items = bounded(queue::list(
        &state.pool,
        signed.user_id,
        tenant,
        location,
        page.after,
        limit,
    ))
    .await?;
    let next_cursor = if items.len() > limit as usize {
        items.truncate(limit as usize);
        items.last().map(|q| q.id)
    } else {
        None
    };
    Ok(Json(QueuePage { items, next_cursor }))
}

#[utoipa::path(post, path="/api/organizations/{tenant_id}/locations/{location_id}/queues", tag="queue", security(("sessionCookie"=[])), params(("tenant_id"=Uuid,Path),("location_id"=Uuid,Path)), request_body=CreateQueueRequest, responses((status=201,body=Queue),(status=403,description="Owner/manager and CSRF required")))]
pub(super) async fn create_queue(
    State(state): State<AppState>,
    signed: SignedIn,
    headers: HeaderMap,
    path: Result<Path<(TenantId, LocationId)>, PathRejection>,
    body: Result<Json<CreateQueueRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Queue>), ApiError> {
    check_mutation(&headers, &state.auth, &signed)?;
    let Path((tenant, location)) = path.map_err(|_| ApiError::bad_request())?;
    let Json(body) = body.map_err(|_| ApiError::bad_request())?;
    let name = Name::parse(&body.name).map_err(ApiError::from)?;
    let queue = bounded(queue::create(
        &state.pool,
        signed.user_id,
        tenant,
        location,
        &name,
    ))
    .await?;
    Ok((StatusCode::CREATED, Json(queue)))
}

#[utoipa::path(get, path="/api/organizations/{tenant_id}/queues/{queue_id}", tag="queue", security(("sessionCookie"=[])), params(("tenant_id"=Uuid,Path),("queue_id"=Uuid,Path)), responses((status=200,body=Queue),(status=404,description="Queue not found")))]
pub(super) async fn get_queue(
    State(state): State<AppState>,
    signed: SignedIn,
    path: Result<Path<(TenantId, Uuid)>, PathRejection>,
) -> Result<Json<Queue>, ApiError> {
    let Path((tenant, queue)) = path.map_err(|_| ApiError::bad_request())?;
    Ok(Json(
        bounded(queue::get(&state.pool, signed.user_id, tenant, queue)).await?,
    ))
}

#[utoipa::path(get, path="/api/organizations/{tenant_id}/queues/{queue_id}/tickets", tag="queue", security(("sessionCookie"=[])), params(("tenant_id"=Uuid,Path),("queue_id"=Uuid,Path),("after"=Option<i64>,Query),("limit"=Option<i64>,Query),("status"=Option<TicketStatus>,Query),("mine"=Option<bool>,Query,description="Only tickets assigned to the authenticated user")), responses((status=200,body=TicketList),(status=400,description="Invalid pagination or status")))]
pub(super) async fn tickets(
    State(state): State<AppState>,
    signed: SignedIn,
    path: Result<Path<(TenantId, Uuid)>, PathRejection>,
    query: Result<Query<TicketPagination>, QueryRejection>,
) -> Result<Json<TicketList>, ApiError> {
    let Path((tenant, queue)) = path.map_err(|_| ApiError::bad_request())?;
    let Query(page) = query.map_err(|_| ApiError::bad_request())?;
    let limit = limit(page.limit)?;
    if page.after.is_some_and(|value| value < 1) {
        return Err(ApiError::bad_request());
    }
    let mut items = bounded(queue::tickets(
        &state.pool,
        signed.user_id,
        tenant,
        queue,
        queue::TicketPage {
            after: page.after,
            status: page.status,
            limit,
            mine: page.mine.unwrap_or(false),
        },
    ))
    .await?;
    let next_cursor = if items.len() > limit as usize {
        items.truncate(limit as usize);
        items.last().map(|t| t.position)
    } else {
        None
    };
    Ok(Json(TicketList { items, next_cursor }))
}

#[utoipa::path(post, path="/api/organizations/{tenant_id}/queues/{queue_id}/commands", tag="queue", security(("sessionCookie"=[])), params(("tenant_id"=Uuid,Path),("queue_id"=Uuid,Path),("Idempotency-Key"=Uuid,Header,description="Required; reuse for retries of the same command")), request_body=Command, responses((status=200,body=CommandResult),(status=400,description="Invalid command or key"),(status=403,description="Operator role and CSRF required"),(status=409,description="Key mismatch, active ticket or invalid state")))]
pub(super) async fn command(
    State(state): State<AppState>,
    signed: SignedIn,
    headers: HeaderMap,
    path: Result<Path<(TenantId, Uuid)>, PathRejection>,
    body: Result<Json<Command>, JsonRejection>,
) -> Result<Json<CommandResult>, ApiError> {
    check_mutation(&headers, &state.auth, &signed)?;
    let Path((tenant, queue)) = path.map_err(|_| ApiError::bad_request())?;
    let Json(command) = body.map_err(|_| ApiError::bad_request())?;
    let key = single_header(&headers, "idempotency-key")
        .map_err(|_| ApiError::bad_request())?
        .parse()
        .map_err(|_| ApiError::bad_request())?;
    Ok(Json(
        bounded(queue::execute(
            &state.pool,
            signed.user_id,
            tenant,
            queue,
            key,
            command,
        ))
        .await?,
    ))
}
