use serde::{Deserialize, Serialize};
use sqlx::{PgPool, types::Json};
use thiserror::Error;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::core::{CoreError, LocationId, Name, Role, TenantId, TenantSession, UserId};

#[derive(Debug, Error)]
pub enum QueueError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("queue database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("queue or ticket not found")]
    NotFound,
    #[error("operation is not allowed for this member")]
    Forbidden,
    #[error("idempotency key has a different command")]
    IdempotencyConflict,
    #[error("ticket is not in the required state")]
    StateConflict,
    #[error("operator already has a called ticket in this queue")]
    ActiveTicket,
    #[error("selected service is unavailable")]
    ServiceUnavailable,
    #[error("selected station is unavailable")]
    StationUnavailable,
    #[error("selected station is busy")]
    StationBusy,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct Queue {
    pub id: Uuid,
    pub tenant_id: TenantId,
    pub location_id: LocationId,
    pub name: String,
    pub version: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum TicketStatus {
    Waiting,
    Called,
    Completed,
    Cancelled,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct Ticket {
    pub id: Uuid,
    pub queue_id: Uuid,
    pub position: i64,
    pub service_date: String,
    pub number: i32,
    pub status: TicketStatus,
    pub called_by: Option<UserId>,
    pub issued_at: String,
    pub called_at: Option<String>,
    pub closed_at: Option<String>,
    #[serde(default)]
    pub service_id: Option<Uuid>,
    #[serde(default)]
    pub service_name: Option<String>,
    #[serde(default)]
    pub station_id: Option<Uuid>,
    #[serde(default)]
    pub station_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Issue {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        service_id: Option<Uuid>,
    },
    CallNext {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        station_id: Option<Uuid>,
    },
    Complete {
        ticket_id: Uuid,
    },
    Cancel {
        ticket_id: Uuid,
    },
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CommandResult {
    pub queue_id: Uuid,
    pub version: i64,
    pub ticket: Option<Ticket>,
}

pub async fn create(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    location: LocationId,
    name: &Name,
) -> Result<Queue, QueueError> {
    let mut session = TenantSession::begin(pool, user, tenant).await?;
    if !session.role().can_manage_locations() {
        return Err(QueueError::Forbidden);
    }
    let row = sqlx::query_as(
        "INSERT INTO queue.queues (tenant_id, id, location_id, name) \
         SELECT $1, $2, id, $4 FROM core.locations WHERE tenant_id = $1 AND id = $3 \
         RETURNING id, tenant_id, location_id, name, version",
    )
    .bind(tenant)
    .bind(Uuid::new_v4())
    .bind(location)
    .bind(name.as_str())
    .fetch_optional(session.connection())
    .await?
    .ok_or(QueueError::NotFound)?;
    session.commit().await?;
    Ok(row)
}

/// Returns at most limit + 1 rows so the HTTP adapter can produce a cursor.
pub async fn list(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    location: LocationId,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<Queue>, QueueError> {
    let mut session = TenantSession::begin(pool, user, tenant).await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM core.locations WHERE tenant_id = $1 AND id = $2)",
    )
    .bind(tenant)
    .bind(location)
    .fetch_one(session.connection())
    .await?;
    if !exists {
        return Err(QueueError::NotFound);
    }
    let rows = sqlx::query_as(
        "SELECT id, tenant_id, location_id, name, version FROM queue.queues \
         WHERE tenant_id = $1 AND location_id = $2 AND ($3::uuid IS NULL OR id > $3) \
         ORDER BY id LIMIT $4",
    )
    .bind(tenant)
    .bind(location)
    .bind(after)
    .bind(limit.clamp(1, 100) + 1)
    .fetch_all(session.connection())
    .await?;
    session.commit().await?;
    Ok(rows)
}

pub async fn get(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    queue: Uuid,
) -> Result<Queue, QueueError> {
    let mut session = TenantSession::begin(pool, user, tenant).await?;
    let row = sqlx::query_as(
        "SELECT id, tenant_id, location_id, name, version FROM queue.queues \
         WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant)
    .bind(queue)
    .fetch_optional(session.connection())
    .await?
    .ok_or(QueueError::NotFound)?;
    session.commit().await?;
    Ok(row)
}

#[derive(Default)]
pub struct TicketPage {
    pub after: Option<i64>,
    pub status: Option<TicketStatus>,
    pub limit: i64,
    pub mine: bool,
}

pub async fn tickets(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    queue: Uuid,
    page: TicketPage,
) -> Result<Vec<Ticket>, QueueError> {
    let mut session = TenantSession::begin(pool, user, tenant).await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM queue.queues WHERE tenant_id = $1 AND id = $2)",
    )
    .bind(tenant)
    .bind(queue)
    .fetch_one(session.connection())
    .await?;
    if !exists {
        return Err(QueueError::NotFound);
    }
    let rows = sqlx::query_as(
        "SELECT id, queue_id, position, service_date, number, status, called_by, \
         issued_at, called_at, closed_at, service_id, service_name, station_id, station_name FROM queue.ticket_details \
         WHERE tenant_id = $1 AND queue_id = $2 AND ($3::bigint IS NULL OR position > $3) \
         AND ($4::text IS NULL OR status = $4) \
         AND ($5::uuid IS NULL OR called_by = $5) ORDER BY position LIMIT $6",
    )
    .bind(tenant)
    .bind(queue)
    .bind(page.after)
    .bind(page.status)
    .bind(page.mine.then_some(user))
    .bind(page.limit.clamp(1, 100) + 1)
    .fetch_all(session.connection())
    .await?;
    session.commit().await?;
    Ok(rows)
}

pub async fn execute(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    queue: Uuid,
    key: Uuid,
    command: Command,
) -> Result<CommandResult, QueueError> {
    let mut session = TenantSession::begin(pool, user, tenant).await?;
    if session.role() == Role::Employee {
        return Err(QueueError::Forbidden);
    }
    sqlx::query("SET LOCAL lock_timeout = '2s'")
        .execute(session.connection())
        .await?;
    // All queue mutations take this lock first. READ COMMITTED re-reads the row
    // after a waiter acquires it; independent queues do not share the lock.
    let mut version: i64 = sqlx::query_scalar(
        "SELECT version FROM queue.queues WHERE tenant_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(tenant)
    .bind(queue)
    .fetch_optional(session.connection())
    .await?
    .ok_or(QueueError::NotFound)?;
    let previous: Option<(Json<Command>, Json<CommandResult>)> = sqlx::query_as(
        "SELECT command, result FROM queue.commands \
         WHERE tenant_id = $1 AND queue_id = $2 AND actor_id = $3 AND idempotency_key = $4",
    )
    .bind(tenant)
    .bind(queue)
    .bind(user)
    .bind(key)
    .fetch_optional(session.connection())
    .await?;
    if let Some((Json(previous), Json(result))) = previous {
        if previous != command {
            return Err(QueueError::IdempotencyConflict);
        }
        session.commit().await?;
        return Ok(result);
    }

    let ticket_id = match command {
        Command::Issue { service_id } => {
            Some(issue(&mut session, tenant, queue, service_id).await?)
        }
        Command::CallNext { station_id } => {
            call_next(&mut session, tenant, queue, user, station_id).await?
        }
        Command::Complete { ticket_id } | Command::Cancel { ticket_id } => {
            close(&mut session, tenant, queue, user, ticket_id, &command).await?;
            Some(ticket_id)
        }
    };
    let ticket = if let Some(id) = ticket_id {
        version = sqlx::query_scalar(
            "UPDATE queue.queues SET version = version + 1 \
             WHERE tenant_id = $1 AND id = $2 RETURNING version",
        )
        .bind(tenant)
        .bind(queue)
        .fetch_one(session.connection())
        .await?;
        Some(read_ticket(&mut session, tenant, queue, id).await?)
    } else {
        None
    };
    let result = CommandResult {
        queue_id: queue,
        version,
        ticket,
    };
    sqlx::query(
        "INSERT INTO queue.commands (tenant_id, queue_id, actor_id, idempotency_key, command, result) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(tenant)
    .bind(queue)
    .bind(user)
    .bind(key)
    .bind(Json(&command))
    .bind(Json(&result))
    .execute(session.connection())
    .await?;
    session.commit().await?;
    Ok(result)
}

async fn issue(
    session: &mut TenantSession,
    tenant: TenantId,
    queue: Uuid,
    service: Option<Uuid>,
) -> Result<Uuid, QueueError> {
    sqlx::query_scalar("SELECT queue.allocate_ticket($1, $2, $3)")
        .bind(tenant)
        .bind(queue)
        .bind(service)
        .fetch_one(session.connection())
        .await
        .map_err(|error: sqlx::Error| {
            if error.as_database_error().and_then(|e| e.code()).as_deref() == Some("Q2010") {
                QueueError::ServiceUnavailable
            } else {
                QueueError::Database(error)
            }
        })
}
async fn call_next(
    session: &mut TenantSession,
    tenant: TenantId,
    queue: Uuid,
    user: UserId,
    station: Option<Uuid>,
) -> Result<Option<Uuid>, QueueError> {
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM queue.tickets \
         WHERE tenant_id = $1 AND queue_id = $2 AND called_by = $3 AND status = 'called')",
    )
    .bind(tenant)
    .bind(queue)
    .bind(user)
    .fetch_one(session.connection())
    .await?;
    if active {
        return Err(QueueError::ActiveTicket);
    }
    let station_name: Option<String> = if let Some(id) = station {
        let name = sqlx::query_scalar("SELECT name FROM queue.stations WHERE tenant_id=$1 AND queue_id=$2 AND id=$3 AND enabled")
            .bind(tenant).bind(queue).bind(id).fetch_optional(session.connection()).await?.ok_or(QueueError::StationUnavailable)?;
        let busy: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM queue.tickets WHERE tenant_id=$1 AND queue_id=$2 AND station_id=$3 AND status='called')")
            .bind(tenant).bind(queue).bind(id).fetch_one(session.connection()).await?;
        if busy {
            return Err(QueueError::StationBusy);
        }
        Some(name)
    } else {
        None
    };
    sqlx::query_scalar(
        "UPDATE queue.tickets SET status = 'called', called_by = $3, called_at = clock_timestamp(), station_id=$4, station_name=$5 \
         WHERE tenant_id = $1 AND queue_id = $2 AND id = (SELECT id FROM queue.tickets \
             WHERE tenant_id = $1 AND queue_id = $2 AND status = 'waiting' \
             ORDER BY position LIMIT 1) RETURNING id",
    )
    .bind(tenant)
    .bind(queue)
    .bind(user)
    .bind(station)
    .bind(station_name)
    .fetch_optional(session.connection())
    .await
    .map_err(|error: sqlx::Error| {
        if error.as_database_error().and_then(|e| e.constraint()) == Some("tickets_station_active_idx") {
            QueueError::StationBusy
        } else {
            QueueError::Database(error)
        }
    })
}

async fn read_ticket(
    session: &mut TenantSession,
    tenant: TenantId,
    queue: Uuid,
    id: Uuid,
) -> Result<Ticket, QueueError> {
    sqlx::query_as(
        "SELECT id, queue_id, position, service_date, number, status, called_by, \
         issued_at, called_at, closed_at, service_id, service_name, station_id, station_name FROM queue.ticket_details \
         WHERE tenant_id = $1 AND queue_id = $2 AND id = $3",
    )
    .bind(tenant)
    .bind(queue)
    .bind(id)
    .fetch_optional(session.connection())
    .await?
    .ok_or(QueueError::NotFound)
}

async fn close(
    session: &mut TenantSession,
    tenant: TenantId,
    queue: Uuid,
    user: UserId,
    id: Uuid,
    command: &Command,
) -> Result<(), QueueError> {
    let ticket = read_ticket(session, tenant, queue, id).await?;
    let allowed = match command {
        Command::Complete { .. } => ticket.status == TicketStatus::Called,
        Command::Cancel { .. } => {
            matches!(ticket.status, TicketStatus::Waiting | TicketStatus::Called)
        }
        _ => false,
    };
    if !allowed {
        return Err(QueueError::StateConflict);
    }
    if ticket.called_by.is_some_and(|actor| actor != user) && !session.role().can_manage_locations()
    {
        return Err(QueueError::Forbidden);
    }
    let status = if matches!(command, Command::Complete { .. }) {
        TicketStatus::Completed
    } else {
        TicketStatus::Cancelled
    };
    sqlx::query(
        "UPDATE queue.tickets SET status = $4, closed_at = clock_timestamp() \
         WHERE tenant_id = $1 AND queue_id = $2 AND id = $3",
    )
    .bind(tenant)
    .bind(queue)
    .bind(id)
    .bind(status)
    .execute(session.connection())
    .await?;
    Ok(())
}
