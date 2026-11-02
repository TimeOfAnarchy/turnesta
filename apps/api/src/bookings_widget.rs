use crate::core::{CoreError, LocationId, TenantId, TenantSession, UserId};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use thiserror::Error;
use url::{Host, Url};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum WidgetError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("widget database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("invalid widget origins")]
    InvalidOrigins,
    #[error("booking location not found")]
    NotFound,
}

pub fn normalize_origin(value: &str) -> Result<String, WidgetError> {
    if value.chars().any(char::is_control) {
        return Err(WidgetError::InvalidOrigins);
    }
    let value = value.trim();
    if value.is_empty()
        || value.len() > 1024
        || value.chars().any(char::is_control)
        || value.contains(['\\', '?', '#', '@'])
    {
        return Err(WidgetError::InvalidOrigins);
    }
    let (_, authority) = value.split_once("://").ok_or(WidgetError::InvalidOrigins)?;
    if authority
        .split_once('/')
        .is_some_and(|(_, path)| !path.is_empty())
    {
        return Err(WidgetError::InvalidOrigins);
    }
    let url = Url::parse(value).map_err(|_| WidgetError::InvalidOrigins)?;
    if let Some(Host::Domain(host)) = url.host()
        && host.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err(WidgetError::InvalidOrigins);
    }
    let loopback = matches!(url.host(), Some(Host::Domain("localhost")))
        || matches!(url.host(), Some(Host::Ipv4(ip)) if ip.octets()==[127,0,0,1])
        || matches!(url.host(), Some(Host::Ipv6(ip)) if ip.is_loopback());
    if url.host().is_none()
        || url.host_str().is_some_and(|h| h.ends_with('.'))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.port() == Some(0)
        || !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
    {
        return Err(WidgetError::InvalidOrigins);
    }
    let origin = url.origin().ascii_serialization();
    if origin.len() > 255 {
        return Err(WidgetError::InvalidOrigins);
    }
    Ok(origin)
}

fn normalize_origins(origins: Vec<String>, service: &str) -> Result<Vec<String>, WidgetError> {
    if origins.len() > 10 {
        return Err(WidgetError::InvalidOrigins);
    }
    let mut normalized = origins
        .iter()
        .map(|o| normalize_origin(o))
        .collect::<Result<Vec<_>, _>>()?;
    if normalized.iter().any(|o| o == service) {
        return Err(WidgetError::InvalidOrigins);
    }
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct WidgetSettings {
    pub allowed_origins: Vec<String>,
    pub version: i32,
}
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WidgetConfigure {
    pub allowed_origins: Vec<String>,
    pub expected_version: i32,
}

async fn check_location(s: &mut TenantSession, point: LocationId) -> Result<(), WidgetError> {
    let tenant = s.tenant_id();
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM core.locations WHERE tenant_id=$1 AND id=$2)",
    )
    .bind(tenant)
    .bind(point)
    .fetch_one(s.connection())
    .await?;
    if !exists {
        return Err(WidgetError::NotFound);
    }
    Ok(())
}
pub async fn settings(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    point: LocationId,
) -> Result<WidgetSettings, WidgetError> {
    let mut s = TenantSession::begin(pool, user, tenant).await?;
    check_location(&mut s, point).await?;
    let row:Option<(Vec<String>,i32)>=sqlx::query_as(
        "SELECT allowed_origins,version FROM bookings.widget_settings WHERE tenant_id=$1 AND location_id=$2"
    ).bind(tenant).bind(point).fetch_optional(s.connection()).await?;
    s.commit().await?;
    Ok(row.map_or(
        WidgetSettings {
            allowed_origins: Vec::new(),
            version: 0,
        },
        |(allowed_origins, version)| WidgetSettings {
            allowed_origins,
            version,
        },
    ))
}
pub async fn configure(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    point: LocationId,
    service: &str,
    input: WidgetConfigure,
) -> Result<WidgetSettings, WidgetError> {
    let mut s = TenantSession::begin(pool, user, tenant).await?;
    if !s.role().can_manage_locations() {
        return Err(CoreError::Forbidden.into());
    }
    check_location(&mut s, point).await?;
    if input.expected_version < 0 {
        return Err(CoreError::InvalidVersion.into());
    }
    let origins = normalize_origins(input.allowed_origins, service)?;
    let row: Option<(Vec<String>, i32)> = if input.expected_version == 0 {
        sqlx::query_as(
            r#"
            INSERT INTO bookings.widget_settings(tenant_id,location_id,allowed_origins)
            VALUES($1,$2,$3) ON CONFLICT DO NOTHING RETURNING allowed_origins,version
        "#,
        )
        .bind(tenant)
        .bind(point)
        .bind(&origins)
        .fetch_optional(s.connection())
        .await?
    } else {
        sqlx::query_as(
            r#"
            UPDATE bookings.widget_settings SET allowed_origins=$3,version=version+1
            WHERE tenant_id=$1 AND location_id=$2 AND version=$4 RETURNING allowed_origins,version
        "#,
        )
        .bind(tenant)
        .bind(point)
        .bind(&origins)
        .bind(input.expected_version)
        .fetch_optional(s.connection())
        .await?
    };
    let (allowed_origins, version) = row.ok_or(CoreError::VersionConflict)?;
    s.commit().await?;
    Ok(WidgetSettings {
        allowed_origins,
        version,
    })
}
pub async fn permits(
    pool: &PgPool,
    slug: Uuid,
    parent: &str,
    service: &str,
) -> Result<bool, WidgetError> {
    let normalized = normalize_origin(parent)?;
    if normalized != parent || normalized == service {
        return Ok(false);
    }
    let origins: Vec<String> = sqlx::query_scalar("SELECT bookings.widget_origins($1)")
        .bind(slug)
        .fetch_one(pool)
        .await?;
    Ok(origins.contains(&normalized))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn origins_are_exact_canonical_and_bounded() {
        for (input, output) in [
            (" HTTPS://Clinic.Example:443/ ", "https://clinic.example"),
            ("https://клиника.рф", "https://xn--80apagcdp.xn--p1ai"),
            ("http://[::1]:8000/", "http://[::1]:8000"),
            ("http://localhost:8000", "http://localhost:8000"),
        ] {
            assert_eq!(normalize_origin(input).unwrap(), output);
        }
        for bad in [
            "null",
            "*",
            "https://*.example",
            "http://clinic.example",
            "https://example/path/..",
            "https://example//",
            "https://@example",
            "https://a:b@example",
            "https://example?",
            "https://example#",
            "https://example\\evil",
            "https://example\r\nX: y",
            "https://example.",
            "ftp://example",
            "https://example:0",
        ] {
            assert!(normalize_origin(bad).is_err(), "{bad}");
        }
        assert!(
            normalize_origins(
                vec!["https://turnesta.example".into()],
                "https://turnesta.example"
            )
            .is_err()
        );
        assert_eq!(
            normalize_origins(
                vec!["https://A.example/".into(), "https://a.example".into()],
                "https://turnesta.example"
            )
            .unwrap(),
            vec!["https://a.example"]
        );
        assert!(
            normalize_origins(
                vec!["https://a.example".into(); 11],
                "https://turnesta.example"
            )
            .is_err()
        );
    }
}
