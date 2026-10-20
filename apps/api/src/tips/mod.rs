use serde::{Deserialize, Serialize};
use sqlx::{PgPool, types::Json};
use thiserror::Error;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::core::{CoreError, LocationId, Name, TenantId, TenantSession, UserId};

#[derive(Debug, Error)]
pub enum TipsError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("tips database operation failed")]
    Database(sqlx::Error),
    #[error("mock amount must be 100..1000000 kopecks")]
    InvalidAmount,
    #[error("tips record not found")]
    NotFound,
    #[error("tips management requires owner or manager")]
    Forbidden,
    #[error("idempotency key was used for a different amount")]
    IdempotencyConflict,
    #[error("mock payment is already terminal or outcome is invalid")]
    StateConflict,
    #[error("mock event does not match provider evidence")]
    EventMismatch,
}

impl From<sqlx::Error> for TipsError {
    fn from(error: sqlx::Error) -> Self {
        match error
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref()
        {
            Some("T1001") => Self::InvalidAmount,
            Some("T1002" | "23503") => Self::NotFound,
            Some("T1003" | "42501") => Self::Forbidden,
            Some("T1004") => Self::IdempotencyConflict,
            Some("T1005") => Self::StateConflict,
            Some("T1006") => Self::EventMismatch,
            _ => Self::Database(error),
        }
    }
}

#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct Recipient {
    pub id: Uuid,
    pub tenant_id: TenantId,
    pub location_id: LocationId,
    pub display_name: String,
    pub public_slug: Uuid,
    pub environment: String,
}
#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct Attempt {
    pub id: Uuid,
    pub recipient_id: Uuid,
    pub recipient_name: String,
    pub amount_minor: i64,
    pub currency: String,
    pub status: String,
    pub environment: String,
    pub created_at: String,
}
#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct PublicRecipient {
    pub display_name: String,
    pub location_name: String,
    pub organization_name: String,
    pub public_slug: Uuid,
    pub environment: String,
}
#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct Checkout {
    pub checkout_token: Uuid,
    pub status_token: Uuid,
}
#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct PublicCheckout {
    pub display_name: String,
    pub location_name: String,
    pub amount_minor: i64,
    pub currency: String,
    pub status: String,
    pub status_token: Uuid,
    pub environment: String,
}
#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct PublicAttempt {
    pub display_name: String,
    pub amount_minor: i64,
    pub currency: String,
    pub status: String,
    pub environment: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MockEvent {
    pub event_id: Uuid,
    pub attempt_id: Uuid,
    pub provider_payment_id: Uuid,
    pub recipient_id: Uuid,
    pub amount_minor: i64,
    pub currency: String,
    pub status: String,
}
pub struct MockDelivery {
    pub job_id: Uuid,
    pub lease_token: Uuid,
    pub event: MockEvent,
    pub attempt: i32,
}
#[derive(sqlx::FromRow)]
struct DeliveryRow {
    job_id: Uuid,
    lease_token: Uuid,
    event: Json<MockEvent>,
    attempt: i32,
}

async fn manager(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
) -> Result<TenantSession, TipsError> {
    let session = TenantSession::begin(pool, user, tenant).await?;
    if !session.role().can_manage_locations() {
        return Err(TipsError::Forbidden);
    }
    Ok(session)
}

pub async fn create_recipient(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    location: LocationId,
    name: &Name,
) -> Result<Recipient, TipsError> {
    let mut session = manager(pool, user, tenant).await?;
    let recipient = sqlx::query_as("INSERT INTO tips.recipients (tenant_id, id, location_id, display_name, public_slug) VALUES ($1, $2, $3, $4, $5) RETURNING id, tenant_id, location_id, display_name, public_slug, environment")
        .bind(tenant).bind(Uuid::new_v4()).bind(location).bind(name.as_str()).bind(Uuid::new_v4())
        .fetch_one(session.connection()).await?;
    session.commit().await?;
    Ok(recipient)
}

pub async fn recipients(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<Recipient>, TipsError> {
    let mut session = manager(pool, user, tenant).await?;
    let rows = sqlx::query_as("SELECT id, tenant_id, location_id, display_name, public_slug, environment FROM tips.recipients WHERE tenant_id = $1 AND ($2::uuid IS NULL OR id > $2) ORDER BY id LIMIT $3")
        .bind(tenant).bind(after).bind(limit.clamp(1, 100) + 1).fetch_all(session.connection()).await?;
    session.commit().await?;
    Ok(rows)
}

pub async fn attempts(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<Attempt>, TipsError> {
    let mut session = manager(pool, user, tenant).await?;
    let rows = sqlx::query_as("SELECT a.id, a.recipient_id, r.display_name AS recipient_name, a.amount_minor, a.currency, a.status, a.environment, to_char(a.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at FROM tips.attempts a JOIN tips.recipients r ON r.tenant_id = a.tenant_id AND r.id = a.recipient_id WHERE a.tenant_id = $1 AND ($2::uuid IS NULL OR a.id > $2) ORDER BY a.id LIMIT $3")
        .bind(tenant).bind(after).bind(limit.clamp(1, 100) + 1).fetch_all(session.connection()).await?;
    session.commit().await?;
    Ok(rows)
}

pub async fn public_recipient(
    pool: &PgPool,
    slug: Uuid,
) -> Result<Option<PublicRecipient>, TipsError> {
    Ok(sqlx::query_as("SELECT display_name, location_name, organization_name, public_slug, environment FROM tips.public_recipient($1)").bind(slug).fetch_optional(pool).await?)
}
pub async fn create_attempt(
    pool: &PgPool,
    slug: Uuid,
    idempotency_key: Uuid,
    amount_minor: i64,
) -> Result<Checkout, TipsError> {
    if !(100..=1_000_000).contains(&amount_minor) {
        return Err(TipsError::InvalidAmount);
    }
    Ok(
        sqlx::query_as("SELECT checkout_token, status_token FROM tips.create_attempt($1, $2, $3)")
            .bind(slug)
            .bind(idempotency_key)
            .bind(amount_minor)
            .fetch_one(pool)
            .await?,
    )
}
pub async fn checkout(
    pool: &PgPool,
    checkout_token: Uuid,
) -> Result<Option<PublicCheckout>, TipsError> {
    Ok(sqlx::query_as("SELECT display_name, location_name, amount_minor, currency, status, status_token, environment FROM tips.public_checkout($1)").bind(checkout_token).fetch_optional(pool).await?)
}
pub async fn complete_mock_checkout(
    pool: &PgPool,
    checkout_token: Uuid,
    outcome: &str,
) -> Result<Uuid, TipsError> {
    Ok(
        sqlx::query_scalar("SELECT tips.complete_mock_checkout($1, $2)")
            .bind(checkout_token)
            .bind(outcome)
            .fetch_one(pool)
            .await?,
    )
}
pub async fn public_attempt(
    pool: &PgPool,
    status_token: Uuid,
) -> Result<Option<PublicAttempt>, TipsError> {
    Ok(sqlx::query_as("SELECT display_name, amount_minor, currency, status, environment FROM tips.public_attempt($1)").bind(status_token).fetch_optional(pool).await?)
}
pub async fn accept_event(pool: &PgPool, event: &MockEvent) -> Result<bool, TipsError> {
    Ok(sqlx::query_scalar("SELECT tips.apply_mock_event($1)")
        .bind(Json(event))
        .fetch_one(pool)
        .await?)
}
pub async fn reconcile_attempt(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    attempt_id: Uuid,
) -> Result<Attempt, TipsError> {
    let mut session = manager(pool, user, tenant).await?;
    sqlx::query("SELECT tips.reconcile_attempt($1, $2)")
        .bind(tenant)
        .bind(attempt_id)
        .execute(session.connection())
        .await?;
    let row = sqlx::query_as("SELECT a.id, a.recipient_id, r.display_name AS recipient_name, a.amount_minor, a.currency, a.status, a.environment, to_char(a.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at FROM tips.attempts a JOIN tips.recipients r ON r.tenant_id = a.tenant_id AND r.id = a.recipient_id WHERE a.tenant_id = $1 AND a.id = $2")
        .bind(tenant).bind(attempt_id).fetch_one(session.connection()).await?;
    session.commit().await?;
    Ok(row)
}
pub async fn claim_mock_delivery(pool: &PgPool) -> Result<Option<MockDelivery>, TipsError> {
    let row: Option<DeliveryRow> = sqlx::query_as(
        "SELECT job_id, lease_token, event, attempt FROM tips.claim_mock_delivery()",
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| MockDelivery {
        job_id: row.job_id,
        lease_token: row.lease_token,
        event: row.event.0,
        attempt: row.attempt,
    }))
}
pub async fn finish_mock_delivery(
    pool: &PgPool,
    job_id: Uuid,
    lease_token: Uuid,
    success: bool,
) -> Result<bool, TipsError> {
    Ok(
        sqlx::query_scalar("SELECT tips.finish_mock_delivery($1, $2, $3)")
            .bind(job_id)
            .bind(lease_token)
            .bind(success)
            .fetch_one(pool)
            .await?,
    )
}
