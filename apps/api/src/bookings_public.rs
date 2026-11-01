use crate::core::{CoreError, LocationId, TenantId, TenantSession, UserId};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, types::Json};
use std::collections::BTreeMap;
use thiserror::Error;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum PublicBookingError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("public booking database operation failed")]
    Database(sqlx::Error),
    #[error("booking page or receipt not found")]
    NotFound,
    #[error("public booking paused")]
    Closed,
    #[error("booking quota exceeded")]
    RateLimit,
    #[error("invalid booking form or answers")]
    Invalid,
    #[error("calendar or form changed")]
    Changed,
    #[error("different request for capability")]
    IdempotencyConflict,
    #[error("receipt access expired")]
    Expired,
    #[error("offer has no places")]
    Full,
    #[error("specialist already booked")]
    Busy,
    #[error("offer unavailable")]
    Unavailable,
}
impl From<sqlx::Error> for PublicBookingError {
    fn from(error: sqlx::Error) -> Self {
        match error.as_database_error().and_then(|e| e.code()).as_deref() {
            Some("B2201") => Self::NotFound,
            Some("B2202") => Self::Closed,
            Some("B2203") => Self::RateLimit,
            Some("B2204" | "B2001") => Self::Invalid,
            Some("B2205") => Self::Changed,
            Some("B2206") => Self::IdempotencyConflict,
            Some("B2207") => Self::Expired,
            Some("B2103") => Self::Unavailable,
            Some("B2104") => Self::Full,
            Some("B2105") => Self::Busy,
            _ => Self::Database(error),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BookingForm {
    pub fields: Vec<BookingFormField>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BookingFormField {
    pub key: String,
    pub label: String,
    pub kind: BookingFieldKind,
    pub required: bool,
    pub options: Vec<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BookingFieldKind {
    ClientName,
    Phone,
    Email,
    Note,
    Text,
    Select,
}
impl Default for BookingForm {
    fn default() -> Self {
        Self {
            fields: [
                (
                    "client_name",
                    "Ваше имя",
                    BookingFieldKind::ClientName,
                    true,
                ),
                ("phone", "Телефон", BookingFieldKind::Phone, true),
                ("email", "Email", BookingFieldKind::Email, false),
                ("note", "Комментарий", BookingFieldKind::Note, false),
            ]
            .into_iter()
            .map(|(key, label, kind, required)| BookingFormField {
                key: key.into(),
                label: label.into(),
                kind,
                required,
                options: Vec::new(),
            })
            .collect(),
        }
    }
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BookingPublicSettings {
    pub public_slug: Option<Uuid>,
    pub enabled: bool,
    pub form: BookingForm,
    pub version: i32,
}
pub async fn settings(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    location: LocationId,
) -> Result<BookingPublicSettings, PublicBookingError> {
    let mut s = TenantSession::begin(pool, user, tenant).await?;
    check_location(&mut s, location).await?;
    let row:Option<(Uuid,bool,Json<BookingForm>,i32)>=sqlx::query_as("SELECT public_slug,enabled,form,version FROM bookings.public_settings WHERE tenant_id=$1 AND location_id=$2").bind(tenant).bind(location).fetch_optional(s.connection()).await?;
    s.commit().await?;
    Ok(match row {
        Some((slug, enabled, Json(form), version)) => BookingPublicSettings {
            public_slug: Some(slug),
            enabled,
            form,
            version,
        },
        None => BookingPublicSettings {
            public_slug: None,
            enabled: false,
            form: BookingForm::default(),
            version: 0,
        },
    })
}
async fn check_location(
    s: &mut TenantSession,
    location: LocationId,
) -> Result<(), PublicBookingError> {
    let tenant = s.tenant_id();
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM core.locations WHERE tenant_id=$1 AND id=$2)",
    )
    .bind(tenant)
    .bind(location)
    .fetch_one(s.connection())
    .await?;
    if exists {
        Ok(())
    } else {
        Err(PublicBookingError::NotFound)
    }
}
pub async fn configure(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    location: LocationId,
    enabled: bool,
    mut form: BookingForm,
    version: i32,
) -> Result<BookingPublicSettings, PublicBookingError> {
    let mut s = TenantSession::begin(pool, user, tenant).await?;
    if !s.role().can_manage_locations() {
        return Err(CoreError::Forbidden.into());
    }
    check_location(&mut s, location).await?;
    if version < 0 {
        return Err(PublicBookingError::Invalid);
    }
    for field in &mut form.fields {
        field.label = field.label.trim().into();
        for option in &mut field.options {
            *option = option.trim().into();
        }
    }
    let valid: bool = sqlx::query_scalar("SELECT bookings.form_valid($1)")
        .bind(Json(&form))
        .fetch_one(s.connection())
        .await?;
    if !valid {
        return Err(PublicBookingError::Invalid);
    }
    let row: Option<(Uuid, bool, Json<BookingForm>, i32)> = if version == 0 {
        sqlx::query_as("INSERT INTO bookings.public_settings(tenant_id,location_id,public_slug,enabled,form) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING RETURNING public_slug,enabled,form,version").bind(tenant).bind(location).bind(Uuid::new_v4()).bind(enabled).bind(Json(&form)).fetch_optional(s.connection()).await?
    } else {
        sqlx::query_as("UPDATE bookings.public_settings SET enabled=$3,form=$4,version=version+1 WHERE tenant_id=$1 AND location_id=$2 AND version=$5 RETURNING public_slug,enabled,form,version").bind(tenant).bind(location).bind(enabled).bind(Json(&form)).bind(version).fetch_optional(s.connection()).await?
    };
    let (slug, enabled, Json(form), version) = row.ok_or(CoreError::VersionConflict)?;
    s.commit().await?;
    Ok(BookingPublicSettings {
        public_slug: Some(slug),
        enabled,
        form,
        version,
    })
}
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct PublicBookingSpecialist {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub version: i32,
}
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct PublicBookingProfile {
    pub public_slug: Uuid,
    pub organization_name: String,
    pub location_name: String,
    pub time_zone: String,
    pub today: String,
    pub enabled: bool,
    pub version: i32,
    pub form: Option<BookingForm>,
    pub specialists: Vec<PublicBookingSpecialist>,
}
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct PublicBookingOffer {
    pub id: Uuid,
    pub specialist_id: Uuid,
    pub specialist_name: String,
    pub specialist_version: i32,
    pub title: String,
    pub version: i32,
    pub mode: String,
    pub date: Option<String>,
    pub starts_local: Option<String>,
    pub ends_local: Option<String>,
    pub time_zone: String,
    pub remaining: i64,
    pub starts_at: Option<String>,
    pub ends_at: Option<String>,
}
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct BookingCalendarDay {
    pub date: String,
    pub count: i64,
}
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct BookingAvailability {
    pub profile: PublicBookingProfile,
    pub month: String,
    pub date: String,
    pub version: String,
    pub days: Vec<BookingCalendarDay>,
    pub items: Vec<PublicBookingOffer>,
    pub next_cursor: Option<Uuid>,
}
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct PublicBookingReceipt {
    pub reference: Uuid,
    pub status: String,
    pub title: String,
    pub specialist_name: String,
    pub time_zone: String,
    pub organization_name: String,
    pub location_name: String,
    pub version: i32,
    pub date: Option<String>,
    pub starts_local: Option<String>,
    pub ends_local: Option<String>,
    pub starts_at: Option<String>,
    pub ends_at: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PublicBookingInput {
    pub offer_id: Uuid,
    pub offer_version: i32,
    pub specialist_version: i32,
    pub form_version: i32,
    pub time_zone: String,
    pub answers: BTreeMap<String, String>,
}
pub struct AvailabilityQuery<'a> {
    pub month: &'a str,
    pub date: &'a str,
    pub specialist: Option<Uuid>,
    pub after: Option<Uuid>,
    pub version: Option<&'a str>,
    pub limit: i32,
}
pub async fn profile(
    pool: &PgPool,
    slug: Uuid,
) -> Result<PublicBookingProfile, PublicBookingError> {
    let value: Option<Json<PublicBookingProfile>> =
        sqlx::query_scalar("SELECT bookings.public_profile($1)")
            .bind(slug)
            .fetch_one(pool)
            .await?;
    value
        .map(|Json(profile)| profile)
        .ok_or(PublicBookingError::NotFound)
}
pub async fn availability(
    pool: &PgPool,
    slug: Uuid,
    q: AvailabilityQuery<'_>,
) -> Result<BookingAvailability, PublicBookingError> {
    if q.month.len() != 7
        || q.date.len() != 10
        || !(1..=100).contains(&q.limit)
        || q.version
            .is_some_and(|v| v.len() != 32 || !v.bytes().all(|c| c.is_ascii_hexdigit()))
    {
        return Err(PublicBookingError::Invalid);
    }
    let Json(value) =
        sqlx::query_scalar("SELECT bookings.public_availability($1,$2,$3,$4,$5,$6,$7)")
            .bind(slug)
            .bind(q.month)
            .bind(q.date)
            .bind(q.specialist)
            .bind(q.after)
            .bind(q.version)
            .bind(q.limit)
            .fetch_one(pool)
            .await?;
    Ok(value)
}
pub async fn book(
    pool: &PgPool,
    slug: Uuid,
    hash: &[u8; 32],
    input: PublicBookingInput,
) -> Result<PublicBookingReceipt, PublicBookingError> {
    if let Some(email) = input
        .answers
        .get("email")
        .filter(|value| !value.trim().is_empty())
    {
        crate::auth::login::Email::parse(email).map_err(|_| PublicBookingError::Invalid)?;
    }
    if input.offer_version < 1
        || input.specialist_version < 1
        || input.form_version < 1
        || input.time_zone.len() > 128
        || input.answers.len() > 14
    {
        return Err(PublicBookingError::Invalid);
    }
    let Json(value) = sqlx::query_scalar("SELECT bookings.public_book($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(slug)
        .bind(hash.as_slice())
        .bind(input.offer_id)
        .bind(input.offer_version)
        .bind(input.specialist_version)
        .bind(input.form_version)
        .bind(input.time_zone)
        .bind(Json(input.answers))
        .fetch_one(pool)
        .await?;
    Ok(value)
}
pub async fn receipt(
    pool: &PgPool,
    slug: Uuid,
    hash: &[u8; 32],
) -> Result<PublicBookingReceipt, PublicBookingError> {
    let Json(value) = sqlx::query_scalar("SELECT bookings.public_receipt($1,$2)")
        .bind(slug)
        .bind(hash.as_slice())
        .fetch_one(pool)
        .await?;
    Ok(value)
}
