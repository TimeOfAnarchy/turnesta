use crate::core::TenantPool;
use crate::core::{CoreError, LocationId, Role, TenantId, TenantSession, UserId};
use serde::Serialize;
use thiserror::Error;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum AuditError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("invalid audit filter or retention")]
    Invalid,
    #[error("audit database operation failed")]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, Serialize, sqlx::FromRow, ToSchema)]
pub struct AuditEntry {
    pub id: String,
    pub actor_id: UserId,
    pub actor_name: String,
    pub location_id: LocationId,
    pub location_name: String,
    pub action: String,
    pub appointment_id: Option<Uuid>,
    pub specialist_id: Option<Uuid>,
    pub specialist_name: Option<String>,
    pub visit_date: Option<String>,
    pub after_id: Option<Uuid>,
    pub page_limit: Option<i32>,
    pub include_cancelled: Option<bool>,
    pub result_count: Option<i32>,
    pub created_at: String,
}

#[derive(Debug, Serialize, sqlx::FromRow, ToSchema)]
pub struct AuditSettings {
    pub retention_days: i32,
    pub version: i32,
}

pub enum Access<'a> {
    List {
        specialist: Uuid,
        date: &'a str,
        after: Option<Uuid>,
        limit: i32,
        include_cancelled: bool,
        returned: i32,
    },
    Detail(Uuid),
    Command(Uuid),
    Export {
        id: Uuid,
        format: crate::bookings_export::ExportFormat,
        count: i32,
    },
}

pub async fn record(
    session: &mut TenantSession,
    location: LocationId,
    access: Access<'_>,
) -> Result<(), sqlx::Error> {
    let (action, appointment, specialist, date, after, limit, cancelled, count) = match access {
        Access::List {
            specialist,
            date,
            after,
            limit,
            include_cancelled,
            returned,
        } => (
            "list",
            None,
            Some(specialist),
            Some(date),
            after,
            Some(limit),
            Some(include_cancelled),
            Some(returned),
        ),
        Access::Detail(id) => ("detail", Some(id), None, None, None, None, None, None),
        Access::Command(id) => ("command", Some(id), None, None, None, None, None, None),
        Access::Export { id, format, count } => (
            match format {
                crate::bookings_export::ExportFormat::Json => "export_json",
                crate::bookings_export::ExportFormat::Csv => "export_csv",
            },
            Some(id),
            None,
            None,
            None,
            None,
            None,
            Some(count),
        ),
    };
    let tenant = session.tenant_id();
    sqlx::query("SELECT audit.record_access($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
        .bind(tenant)
        .bind(location)
        .bind(action)
        .bind(appointment)
        .bind(specialist)
        .bind(date)
        .bind(after)
        .bind(limit)
        .bind(cancelled)
        .bind(count)
        .execute(session.connection())
        .await?;
    Ok(())
}

pub(crate) async fn owner(
    pool: &impl TenantPool,
    user: UserId,
    tenant: TenantId,
) -> Result<TenantSession, CoreError> {
    let mut session = TenantSession::begin(pool, user, tenant).await?;
    if session.role() != Role::Owner {
        return Err(CoreError::Forbidden);
    }
    sqlx::query("SELECT audit.lock_owner($1)")
        .bind(tenant)
        .execute(session.connection())
        .await?;
    Ok(session)
}

pub async fn settings(
    pool: &impl TenantPool,
    user: UserId,
    tenant: TenantId,
) -> Result<AuditSettings, AuditError> {
    let mut session = owner(pool, user, tenant).await?;
    let row =
        sqlx::query_as("SELECT retention_days,version FROM audit.settings WHERE tenant_id=$1")
            .bind(tenant)
            .fetch_optional(session.connection())
            .await?;
    session.commit().await?;
    Ok(row.unwrap_or(AuditSettings {
        retention_days: 365,
        version: 0,
    }))
}

pub async fn configure(
    pool: &impl TenantPool,
    user: UserId,
    tenant: TenantId,
    days: i32,
    version: i32,
) -> Result<AuditSettings, AuditError> {
    if !(30..=3650).contains(&days) || version < 0 {
        return Err(AuditError::Invalid);
    }
    let mut session = owner(pool, user, tenant).await?;
    let result = if version == 0 {
        sqlx::query_as("INSERT INTO audit.settings(tenant_id,retention_days) VALUES($1,$2) ON CONFLICT DO NOTHING RETURNING retention_days,version")
            .bind(tenant).bind(days).fetch_optional(session.connection()).await?
    } else {
        sqlx::query_as("UPDATE audit.settings SET retention_days=$2,version=version+1 WHERE tenant_id=$1 AND version=$3 RETURNING retention_days,version")
            .bind(tenant).bind(days).bind(version).fetch_optional(session.connection()).await?
    };
    let saved = result.ok_or(CoreError::VersionConflict)?;
    session.commit().await?;
    Ok(saved)
}

pub struct AuditQuery<'a> {
    pub from: Option<&'a str>,
    pub to: Option<&'a str>,
    pub actor: Option<UserId>,
    pub before_at: Option<&'a str>,
    pub before_id: Option<i64>,
    pub limit: i32,
}

pub async fn list(
    pool: &impl TenantPool,
    user: UserId,
    tenant: TenantId,
    query: AuditQuery<'_>,
) -> Result<Vec<AuditEntry>, AuditError> {
    let valid_date = |value: &str| {
        value.len() == 10
            && value
                .parse::<jiff::civil::Date>()
                .is_ok_and(|date| date.to_string() == value)
    };
    if !(1..=100).contains(&query.limit)
        || query.from.is_some_and(|v| !valid_date(v))
        || query.to.is_some_and(|v| !valid_date(v))
        || query.from.zip(query.to).is_some_and(|(from, to)| from > to)
        || query
            .before_at
            .is_some_and(|v| v.len() > 40 || v.parse::<jiff::Timestamp>().is_err())
        || query.before_at.is_some() != query.before_id.is_some()
        || query.before_id.is_some_and(|id| id <= 0)
    {
        return Err(AuditError::Invalid);
    }
    let mut session = owner(pool, user, tenant).await?;
    let rows = sqlx::query_as(
        "SELECT entry.*, s.name AS specialist_name
         FROM audit.list_access($1,$2,$3,$4,$5,$6,$7) entry
         LEFT JOIN bookings.specialists s ON s.tenant_id=$1 AND s.location_id=entry.location_id AND s.id=entry.specialist_id
         ORDER BY entry.created_at DESC, entry.id::bigint DESC",
    )
        .bind(tenant)
        .bind(query.from)
        .bind(query.to)
        .bind(query.actor)
        .bind(query.before_at)
        .bind(query.before_id)
        .bind(query.limit)
        .fetch_all(session.connection())
        .await?;
    session.commit().await?;
    Ok(rows)
}
