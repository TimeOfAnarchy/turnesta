use serde::Serialize;
use sqlx::PgPool;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    core::{CoreError, Name, TenantId, TenantSession, UserId},
    queue::QueueError,
};

#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct Station {
    pub id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub version: i32,
    pub busy: bool,
}

pub async fn list(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    queue: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<Station>, QueueError> {
    let mut session = TenantSession::begin(pool, user, tenant).await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM queue.queues WHERE tenant_id=$1 AND id=$2)",
    )
    .bind(tenant)
    .bind(queue)
    .fetch_one(session.connection())
    .await?;
    if !exists {
        return Err(QueueError::NotFound);
    }
    let rows = sqlx::query_as(
        "SELECT s.id,s.name,s.enabled,s.version,EXISTS (SELECT 1 FROM queue.tickets t \
         WHERE t.tenant_id=s.tenant_id AND t.queue_id=s.queue_id AND t.station_id=s.id AND t.status='called') AS busy \
         FROM queue.stations s WHERE s.tenant_id=$1 AND s.queue_id=$2 AND ($3::uuid IS NULL OR s.id>$3) ORDER BY s.id LIMIT $4",
    ).bind(tenant).bind(queue).bind(after).bind(limit.clamp(1,100)+1).fetch_all(session.connection()).await?;
    session.commit().await?;
    Ok(rows)
}

pub struct StationDraft<'a> {
    pub name: &'a Name,
    pub enabled: bool,
}

pub async fn save(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    queue: Uuid,
    existing: Option<(Uuid, i32)>,
    draft: StationDraft<'_>,
) -> Result<Station, QueueError> {
    if existing.is_some_and(|(_, version)| version < 1) {
        return Err(CoreError::InvalidVersion.into());
    }
    let mut session = TenantSession::begin(pool, user, tenant).await?;
    if !session.role().can_manage_locations() {
        return Err(QueueError::Forbidden);
    }
    sqlx::query("SET LOCAL lock_timeout='2s'")
        .execute(session.connection())
        .await?;
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM queue.queues WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(tenant)
    .bind(queue)
    .fetch_optional(session.connection())
    .await?
    .ok_or(QueueError::NotFound)?;
    let id = existing.map(|(id, _)| id).unwrap_or_else(Uuid::new_v4);
    if let Some((_, version)) = existing {
        let current: Option<i32> = sqlx::query_scalar(
            "SELECT version FROM queue.stations WHERE tenant_id=$1 AND queue_id=$2 AND id=$3",
        )
        .bind(tenant)
        .bind(queue)
        .bind(id)
        .fetch_optional(session.connection())
        .await?;
        match current {
            None => return Err(QueueError::NotFound),
            Some(current) if current != version => return Err(CoreError::VersionConflict.into()),
            _ => {}
        }
        sqlx::query("UPDATE queue.stations SET name=$4,enabled=$5,version=version+1 WHERE tenant_id=$1 AND queue_id=$2 AND id=$3")
            .bind(tenant).bind(queue).bind(id).bind(draft.name.as_str()).bind(draft.enabled).execute(session.connection()).await?;
    } else {
        sqlx::query("INSERT INTO queue.stations (tenant_id,queue_id,id,name,enabled) VALUES ($1,$2,$3,$4,$5)")
            .bind(tenant).bind(queue).bind(id).bind(draft.name.as_str()).bind(draft.enabled).execute(session.connection()).await?;
    }
    let row=sqlx::query_as("SELECT s.id,s.name,s.enabled,s.version,EXISTS (SELECT 1 FROM queue.tickets t \
        WHERE t.tenant_id=s.tenant_id AND t.queue_id=s.queue_id AND t.station_id=s.id AND t.status='called') AS busy \
        FROM queue.stations s WHERE s.tenant_id=$1 AND s.queue_id=$2 AND s.id=$3")
        .bind(tenant).bind(queue).bind(id).fetch_one(session.connection()).await?;
    sqlx::query("UPDATE queue.queues SET version=version+1 WHERE tenant_id=$1 AND id=$2")
        .bind(tenant)
        .bind(queue)
        .execute(session.connection())
        .await?;
    session.commit().await?;
    Ok(row)
}
