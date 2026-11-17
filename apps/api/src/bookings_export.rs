use std::collections::BTreeMap;

use futures_util::TryStreamExt;
use serde::Deserialize;
use serde_json::Value;
use sqlx::{PgPool, types::Json};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    audit::{self, Access},
    bookings::BookingError,
    core::{CoreError, LocationId, TenantId, UserId},
};

#[derive(Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Json,
    Csv,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Contact {
    Phone,
    Email,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportRequest {
    pub contact: Contact,
    pub format: ExportFormat,
    pub expected_version: i32,
}

// Only client-facing visit and consent fields belong in the file. The JSON object
// supplies the same columns to both encoders, including for historical form schemas.
const VISITS: &str = "
    SELECT to_jsonb(visit) FROM (
        SELECT a.id, l.name AS location, a.location_id, d.title, d.specialist_name,
            d.status, d.source, d.mode, d.date, d.starts_local, d.ends_local,
            d.time_zone, d.starts_at, d.ends_at, d.client_name, d.phone, d.email, d.note,
            d.form_snapshot, d.answers, d.created_at, d.updated_at, d.cancelled_at,
            a.consent_url, to_char(a.consent_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS consent_at,
            a.consent_version, encode(a.consent_text_sha256,'hex') AS consent_text_sha256
        FROM bookings.appointments a
        JOIN bookings.appointment_details d ON d.tenant_id=a.tenant_id AND d.id=a.id
        JOIN core.locations l ON l.tenant_id=a.tenant_id AND l.id=a.location_id
        WHERE a.tenant_id=$1 AND a.anonymized_at IS NULL AND (
            (a.phone<>'' AND bookings.export_phone_key(a.phone)=$2) OR
            (a.email<>'' AND lower(btrim(a.email))=$3))
    ) visit ORDER BY visit.created_at, visit.id";

pub async fn export(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    location: LocationId,
    id: Uuid,
    request: ExportRequest,
) -> Result<Vec<u8>, BookingError> {
    let mut session = audit::owner(pool, user, tenant).await?;
    let (version, phone, email): (i32, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT version,bookings.export_phone_key(phone),nullif(lower(btrim(email)),'')
         FROM bookings.appointments WHERE tenant_id=$1 AND location_id=$2 AND id=$3 FOR SHARE",
    )
    .bind(tenant)
    .bind(location)
    .bind(id)
    .fetch_optional(session.connection())
    .await?
    .ok_or(BookingError::NotFound)?;
    if version != request.expected_version {
        return Err(CoreError::VersionConflict.into());
    }
    let (phone, email) = match request.contact {
        Contact::Phone if phone.is_some() => (phone, None),
        Contact::Email if email.is_some() => (None, email),
        _ => return Err(BookingError::InvalidClient),
    };

    let mut output = match request.format {
        ExportFormat::Json => vec![b'['],
        ExportFormat::Csv => vec![0xef, 0xbb, 0xbf],
    };
    let mut count: i32 = 0;
    let mut visits = sqlx::query_scalar::<_, Json<BTreeMap<String, Value>>>(VISITS)
        .bind(tenant)
        .bind(phone)
        .bind(email)
        .fetch(session.connection());
    while let Some(Json(row)) = visits.try_next().await? {
        match request.format {
            ExportFormat::Json => {
                if count > 0 {
                    output.push(b',');
                }
                serde_json::to_writer(&mut output, &row)
                    .map_err(|e| sqlx::Error::Encode(e.into()))?;
            }
            ExportFormat::Csv => {
                if count == 0 {
                    csv_line(&mut output, row.keys().map(String::as_str));
                }
                let fields: Vec<String> = row
                    .into_values()
                    .map(|v| match v {
                        Value::Null => String::new(),
                        Value::String(s) => s,
                        other => other.to_string(),
                    })
                    .collect();
                csv_line(&mut output, fields.iter().map(String::as_str));
            }
        }
        count = count
            .checked_add(1)
            .ok_or_else(|| sqlx::Error::Protocol("export count exceeds audit range".into()))?;
    }
    drop(visits);
    if matches!(request.format, ExportFormat::Json) {
        output.push(b']');
    }
    audit::record(
        &mut session,
        location,
        Access::Export {
            id,
            format: request.format,
            count,
        },
    )
    .await?;
    session.commit().await?;
    Ok(output)
}

fn csv_line<'a>(out: &mut Vec<u8>, fields: impl Iterator<Item = &'a str>) {
    for (i, field) in fields.enumerate() {
        if i > 0 {
            out.push(b',');
        }
        out.push(b'"');
        if field
            .trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
            .starts_with(['=', '+', '-', '@'])
            || field.starts_with(['\t', '\r', '\n'])
        {
            out.push(b'\'');
        }
        out.extend_from_slice(field.replace('"', "\"\"").as_bytes());
        out.push(b'"');
    }
    out.extend_from_slice(b"\r\n");
}
