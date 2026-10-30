use serde::Serialize;
use sqlx::PgPool;
use thiserror::Error;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::core::{CoreError, Name, TenantId, TenantSession, UserId};

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("catalog database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("queue or service not found")]
    NotFound,
    #[error("invalid description")]
    InvalidDescription,
}

#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct Service {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub version: i32,
}

pub struct ServiceDraft<'a> {
    pub name: &'a Name,
    pub description: &'a str,
    pub enabled: bool,
}

impl ServiceDraft<'_> {
    fn description(&self) -> Result<&str, CatalogError> {
        let value = self.description.trim();
        if value.chars().count() > 500 || value.chars().any(char::is_control) {
            return Err(CatalogError::InvalidDescription);
        }
        Ok(value)
    }
}

pub async fn list(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    queue: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<Service>, CatalogError> {
    let mut session = TenantSession::begin(pool, user, tenant).await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM queue.queues WHERE tenant_id=$1 AND id=$2)",
    )
    .bind(tenant)
    .bind(queue)
    .fetch_one(session.connection())
    .await?;
    if !exists {
        return Err(CatalogError::NotFound);
    }
    let result=sqlx::query_as(
        "SELECT s.id,s.name,s.description,s.enabled,s.version FROM catalog.services s JOIN queue.service_links link \
         ON link.tenant_id=s.tenant_id AND link.service_id=s.id WHERE link.tenant_id=$1 AND link.queue_id=$2 \
         AND ($3::uuid IS NULL OR s.id>$3) ORDER BY s.id LIMIT $4",
    ).bind(tenant).bind(queue).bind(after).bind(limit.clamp(1,100)+1).fetch_all(session.connection()).await?;
    session.commit().await?;
    Ok(result)
}

pub async fn save(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    queue: Uuid,
    existing: Option<(Uuid, i32)>,
    draft: ServiceDraft<'_>,
) -> Result<Service, CatalogError> {
    let description = draft.description()?;
    if existing.is_some_and(|(_, version)| version < 1) {
        return Err(CoreError::InvalidVersion.into());
    }
    let mut session = TenantSession::begin(pool, user, tenant).await?;
    if !session.role().can_manage_locations() {
        return Err(CoreError::Forbidden.into());
    }
    sqlx::query("SET LOCAL lock_timeout='2s'")
        .execute(session.connection())
        .await?;
    let point: Uuid = sqlx::query_scalar(
        "SELECT location_id FROM queue.queues WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(tenant)
    .bind(queue)
    .fetch_optional(session.connection())
    .await?
    .ok_or(CatalogError::NotFound)?;
    let result = if let Some((id, version)) = existing {
        let linked:bool=sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM queue.service_links WHERE tenant_id=$1 AND queue_id=$2 AND service_id=$3)")
            .bind(tenant).bind(queue).bind(id).fetch_one(session.connection()).await?;
        if !linked {
            return Err(CatalogError::NotFound);
        }
        sqlx::query_as(
            "UPDATE catalog.services SET name=$3,description=$4,enabled=$5,version=version+1 \
             WHERE tenant_id=$1 AND id=$2 AND version=$6 RETURNING id,name,description,enabled,version",
        ).bind(tenant).bind(id).bind(draft.name.as_str()).bind(description).bind(draft.enabled).bind(version)
            .fetch_optional(session.connection()).await?.ok_or(CoreError::VersionConflict)?
    } else {
        let id = Uuid::new_v4();
        let result=sqlx::query_as("INSERT INTO catalog.services (tenant_id,id,location_id,name,description,enabled) VALUES ($1,$2,$3,$4,$5,$6) RETURNING id,name,description,enabled,version")
            .bind(tenant).bind(id).bind(point).bind(draft.name.as_str()).bind(description).bind(draft.enabled).fetch_one(session.connection()).await?;
        sqlx::query("INSERT INTO queue.service_links (tenant_id,queue_id,location_id,service_id) VALUES ($1,$2,$3,$4)")
            .bind(tenant).bind(queue).bind(point).bind(id).execute(session.connection()).await?;
        result
    };
    sqlx::query("UPDATE queue.queues SET version=version+1 WHERE tenant_id=$1 AND id=$2")
        .bind(tenant)
        .bind(queue)
        .execute(session.connection())
        .await?;
    session.commit().await?;
    Ok(result)
}
