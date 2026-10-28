use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use thiserror::Error;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::core::{CoreError, TenantId, TenantSession, UserId};
use crate::queue::TicketStatus;

#[derive(Debug, Error)]
pub enum PublicQueueError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("public queue database operation failed")]
    Database(sqlx::Error),
    #[error("queue or ticket not found")]
    NotFound,
    #[error("invalid ticket token")]
    InvalidToken,
    #[error("ticket public access expired")]
    Expired,
    #[error("public issuance is paused")]
    Closed,
    #[error("public issuance rate exceeded")]
    RateLimit,
    #[error("queue waiting capacity exceeded")]
    Capacity,
}

impl From<sqlx::Error> for PublicQueueError {
    fn from(error: sqlx::Error) -> Self {
        match error.as_database_error().and_then(|e| e.code()).as_deref() {
            Some("Q2001") => Self::NotFound,
            Some("Q2002") => Self::InvalidToken,
            Some("Q2003") => Self::Expired,
            Some("Q2004") => Self::Closed,
            Some("Q2005") => Self::RateLimit,
            Some("Q2006") => Self::Capacity,
            _ => Self::Database(error),
        }
    }
}

#[derive(Serialize, sqlx::FromRow)]
pub struct PublicSettings {
    pub public_slug: Option<Uuid>,
    pub join_open: bool,
    pub display_enabled: bool,
    pub version: i32,
}

#[derive(Serialize, sqlx::FromRow, ToSchema)]
pub struct PublicProfile {
    pub public_slug: Uuid,
    pub queue_name: String,
    pub location_name: String,
    pub organization_name: String,
    pub time_zone: String,
    pub join_open: bool,
}

#[derive(Debug, PartialEq, Eq, Serialize, sqlx::FromRow, ToSchema)]
pub struct PublicTicket {
    pub number: i32,
    pub service_date: String,
    pub status: TicketStatus,
    pub issued_at: String,
    pub people_ahead: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct DisplayTicket {
    pub number: i32,
    pub service_date: String,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct PublicDisplay {
    pub queue_name: String,
    pub location_name: String,
    pub organization_name: String,
    pub time_zone: String,
    pub version: String,
    pub observed_at: String,
    pub join_open: bool,
    pub waiting_count: i64,
    pub called_count: i64,
    pub page: i32,
    pub next_page: Option<i32>,
    pub called: Vec<DisplayTicket>,
}

pub async fn settings(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    queue: Uuid,
) -> Result<PublicSettings, PublicQueueError> {
    let mut session = TenantSession::begin(pool, user, tenant).await?;
    let result = sqlx::query_as(
        "SELECT s.public_slug, coalesce(s.join_open, false) AS join_open, \
         coalesce(s.display_enabled, false) AS display_enabled, coalesce(s.version, 0) AS version \
         FROM queue.queues q LEFT JOIN queue.public_settings s ON s.tenant_id = q.tenant_id AND s.queue_id = q.id \
         WHERE q.tenant_id = $1 AND q.id = $2",
    ).bind(tenant).bind(queue).fetch_optional(session.connection()).await?.ok_or(PublicQueueError::NotFound)?;
    session.commit().await?;
    Ok(result)
}

pub async fn configure(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    queue: Uuid,
    join_open: bool,
    expected_version: i32,
) -> Result<PublicSettings, PublicQueueError> {
    configure_setting(
        pool,
        user,
        tenant,
        queue,
        Setting::Joining(join_open),
        expected_version,
    )
    .await
}

pub async fn configure_display(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    queue: Uuid,
    enabled: bool,
    expected_version: i32,
) -> Result<PublicSettings, PublicQueueError> {
    configure_setting(
        pool,
        user,
        tenant,
        queue,
        Setting::Display(enabled),
        expected_version,
    )
    .await
}

enum Setting {
    Joining(bool),
    Display(bool),
}

async fn configure_setting(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    queue: Uuid,
    setting: Setting,
    expected_version: i32,
) -> Result<PublicSettings, PublicQueueError> {
    if expected_version < 0 {
        return Err(CoreError::InvalidVersion.into());
    }
    let mut session = TenantSession::begin(pool, user, tenant).await?;
    if !session.role().can_manage_locations() {
        return Err(CoreError::Forbidden.into());
    }
    sqlx::query("SET LOCAL lock_timeout = '2s'")
        .execute(session.connection())
        .await?;
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM queue.queues WHERE tenant_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(tenant)
    .bind(queue)
    .fetch_optional(session.connection())
    .await?
    .ok_or(PublicQueueError::NotFound)?;
    let (joining, display) = match setting {
        Setting::Joining(value) => (Some(value), None),
        Setting::Display(value) => (None, Some(value)),
    };
    let result = if expected_version == 0 {
        sqlx::query_as(
            "INSERT INTO queue.public_settings (tenant_id, queue_id, public_slug, join_open, display_enabled) \
             VALUES ($1, $2, $3, coalesce($4, false), coalesce($5, false)) ON CONFLICT (tenant_id, queue_id) DO NOTHING \
             RETURNING public_slug, join_open, display_enabled, version",
        ).bind(tenant).bind(queue).bind(Uuid::new_v4()).bind(joining).bind(display).fetch_optional(session.connection()).await?
    } else {
        sqlx::query_as(
            "UPDATE queue.public_settings SET join_open = coalesce($3, join_open), \
             display_enabled = coalesce($4, display_enabled), version = version + 1 \
             WHERE tenant_id = $1 AND queue_id = $2 AND version = $5 RETURNING public_slug, join_open, display_enabled, version",
        ).bind(tenant).bind(queue).bind(joining).bind(display).bind(expected_version).fetch_optional(session.connection()).await?
    }.ok_or(CoreError::VersionConflict)?;
    session.commit().await?;
    Ok(result)
}

pub async fn display(
    pool: &PgPool,
    slug: Uuid,
    page: i32,
) -> Result<PublicDisplay, PublicQueueError> {
    if !(0..=21_474_835).contains(&page) {
        return Err(CoreError::InvalidVersion.into());
    }
    let result: Option<sqlx::types::Json<PublicDisplay>> =
        sqlx::query_scalar("SELECT queue.public_display($1, $2)")
            .bind(slug)
            .bind(page)
            .fetch_one(pool)
            .await?;
    result
        .map(|value| value.0)
        .ok_or(PublicQueueError::NotFound)
}

pub async fn profile(pool: &PgPool, slug: Uuid) -> Result<PublicProfile, PublicQueueError> {
    sqlx::query_as("SELECT * FROM queue.public_profile($1)")
        .bind(slug)
        .fetch_optional(pool)
        .await?
        .ok_or(PublicQueueError::NotFound)
}

pub async fn ticket(
    pool: &PgPool,
    slug: Uuid,
    token_hash: &[u8; 32],
) -> Result<PublicTicket, PublicQueueError> {
    Ok(sqlx::query_as("SELECT * FROM queue.public_ticket($1, $2)")
        .bind(slug)
        .bind(token_hash.as_slice())
        .fetch_one(pool)
        .await?)
}

pub async fn issue(
    pool: &PgPool,
    slug: Uuid,
    token_hash: &[u8; 32],
) -> Result<PublicTicket, PublicQueueError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL lock_timeout = '2s'")
        .execute(&mut *tx)
        .await?;
    let result = sqlx::query_as("SELECT * FROM queue.public_issue($1, $2)")
        .bind(slug)
        .bind(token_hash.as_slice())
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(result)
}
