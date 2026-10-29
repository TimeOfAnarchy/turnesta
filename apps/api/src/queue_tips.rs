use serde::Serialize;
use sqlx::PgPool;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    core::{CoreError, TenantId, TenantSession, UserId},
    queue_public::PublicQueueError,
};

#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct TipSettings {
    pub recipient_id: Option<Uuid>,
    pub recipient_name: Option<String>,
    pub public_slug: Option<Uuid>,
    pub version: i32,
}

pub async fn settings(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    queue: Uuid,
) -> Result<TipSettings, PublicQueueError> {
    let mut session = TenantSession::begin(pool, user, tenant).await?;
    if !session.role().can_manage_locations() {
        return Err(CoreError::Forbidden.into());
    }
    let result = sqlx::query_as(
        "SELECT ts.recipient_id, r.display_name AS recipient_name, r.public_slug, coalesce(ts.version,0) AS version \
         FROM queue.queues q LEFT JOIN queue.tip_settings ts ON ts.tenant_id=q.tenant_id AND ts.queue_id=q.id \
         LEFT JOIN tips.recipients r ON r.tenant_id=ts.tenant_id AND r.id=ts.recipient_id \
         WHERE q.tenant_id=$1 AND q.id=$2",
    ).bind(tenant).bind(queue).fetch_optional(session.connection()).await?.ok_or(PublicQueueError::NotFound)?;
    session.commit().await?;
    Ok(result)
}

pub async fn configure(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    queue: Uuid,
    recipient: Option<Uuid>,
    expected_version: i32,
) -> Result<TipSettings, PublicQueueError> {
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
    let location: Uuid = sqlx::query_scalar(
        "SELECT location_id FROM queue.queues WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(tenant)
    .bind(queue)
    .fetch_optional(session.connection())
    .await?
    .ok_or(PublicQueueError::NotFound)?;
    let details: Option<(String, Uuid)> = if let Some(id) = recipient {
        Some(
            sqlx::query_as(
                "SELECT display_name, public_slug FROM tips.recipients \
             WHERE tenant_id=$1 AND id=$2 AND location_id=$3 AND environment='mock'",
            )
            .bind(tenant)
            .bind(id)
            .bind(location)
            .fetch_optional(session.connection())
            .await?
            .ok_or(PublicQueueError::NotFound)?,
        )
    } else {
        None
    };
    let version = if expected_version == 0 {
        sqlx::query_scalar(
            "INSERT INTO queue.tip_settings (tenant_id,queue_id,location_id,recipient_id) VALUES ($1,$2,$3,$4) \
             ON CONFLICT (tenant_id,queue_id) DO NOTHING RETURNING version",
        ).bind(tenant).bind(queue).bind(location).bind(recipient).fetch_optional(session.connection()).await?
    } else {
        sqlx::query_scalar(
            "UPDATE queue.tip_settings SET recipient_id=$3, version=version+1 \
             WHERE tenant_id=$1 AND queue_id=$2 AND version=$4 RETURNING version",
        ).bind(tenant).bind(queue).bind(recipient).bind(expected_version).fetch_optional(session.connection()).await?
    }.ok_or(CoreError::VersionConflict)?;
    session.commit().await?;
    Ok(TipSettings {
        recipient_id: recipient,
        recipient_name: details.as_ref().map(|(name, _)| name.clone()),
        public_slug: details.map(|(_, slug)| slug),
        version,
    })
}
