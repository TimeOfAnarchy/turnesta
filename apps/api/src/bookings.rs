use jiff::{SignedDuration, Zoned, civil::DateTime, tz::AmbiguousOffset};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, types::Json};
use thiserror::Error;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::core::{CoreError, LocationId, Name, TenantId, TenantSession, UserId};

#[derive(Debug, Error)]
pub enum BookingError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("booking database operation failed")]
    Database(sqlx::Error),
    #[error("booking resource not found")]
    NotFound,
    #[error("invalid booking fields")]
    Invalid,
    #[error("local time does not exist")]
    TimeGap,
    #[error("local time is ambiguous")]
    TimeFold,
    #[error("timezone changed since the form was opened")]
    TimeZoneChanged,
    #[error("specialist disabled")]
    SpecialistDisabled,
    #[error("idempotency key has different payload")]
    IdempotencyConflict,
    #[error("overlap needs acknowledgement")]
    Overlap(ConflictNotice),
    #[error("invalid client contact")]
    InvalidClient,
    #[error("offer unavailable for booking")]
    OfferUnavailable,
    #[error("offer has no remaining places")]
    OfferFull,
    #[error("specialist already booked")]
    SpecialistBusy,
    #[error("occupied offer requires explicit rescheduling")]
    OfferOccupied,
    #[error("appointment is no longer active")]
    AppointmentClosed,
}
impl From<sqlx::Error> for BookingError {
    fn from(error: sqlx::Error) -> Self {
        match error.as_database_error().and_then(|e| e.code()).as_deref() {
            Some("B2001") => Self::Invalid,
            Some("B2002") => Self::TimeGap,
            Some("B2003") => Self::TimeFold,
            Some("B2101") => Self::NotFound,
            Some("B2102") => Self::AppointmentClosed,
            Some("B2103") => Self::OfferUnavailable,
            Some("B2104") => Self::OfferFull,
            Some("B2105") => Self::SpecialistBusy,
            Some("B2106") => Self::OfferOccupied,
            _ => Self::Database(error),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct Specialist {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub version: i32,
}
#[derive(Debug, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct Preset {
    pub id: Uuid,
    pub name: String,
    pub duration_minutes: Option<i32>,
    pub enabled: bool,
    pub version: i32,
}
#[derive(Debug, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct Offer {
    pub id: Uuid,
    pub specialist_id: Uuid,
    pub title: String,
    pub mode: String,
    pub date: Option<String>,
    pub starts_local: Option<String>,
    pub ends_local: Option<String>,
    pub starts_at: Option<String>,
    pub ends_at: Option<String>,
    pub time_zone: String,
    pub places: i32,
    pub state: OfferState,
    pub version: i32,
    #[serde(default)]
    pub reserved_places: Option<i64>,
    #[serde(default)]
    pub blocked_by_booking: Option<bool>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum OfferState {
    Draft,
    Ready,
    Withdrawn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Timing {
    Interval {
        starts_local: String,
        ends_local: String,
    },
    StartOnly {
        starts_local: String,
    },
    ByAgreement {
        date: Option<String>,
        places: i32,
    },
}
struct TimingFields<'a> {
    mode: &'a str,
    date: Option<&'a str>,
    start: Option<&'a str>,
    end: Option<&'a str>,
    places: i32,
}
impl Timing {
    fn fields(&self) -> Result<TimingFields<'_>, BookingError> {
        Ok(match self {
            Self::Interval {
                starts_local,
                ends_local,
            } => TimingFields {
                mode: "interval",
                date: None,
                start: Some(starts_local),
                end: Some(ends_local),
                places: 1,
            },
            Self::StartOnly { starts_local } => TimingFields {
                mode: "start_only",
                date: None,
                start: Some(starts_local),
                end: None,
                places: 1,
            },
            Self::ByAgreement { date, places } if (1..=1000).contains(places) => TimingFields {
                mode: "by_agreement",
                date: date.as_deref(),
                start: None,
                end: None,
                places: *places,
            },
            _ => return Err(BookingError::Invalid),
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BookingCommand {
    SaveSpecialist {
        id: Uuid,
        name: String,
        description: String,
        enabled: bool,
        expected_version: i32,
    },
    SavePreset {
        id: Uuid,
        name: String,
        duration_minutes: Option<i32>,
        enabled: bool,
        expected_version: i32,
    },
    SaveOffer {
        id: Uuid,
        specialist_id: Uuid,
        time_zone: String,
        title: String,
        timing: Timing,
        state: OfferState,
        expected_version: i32,
        conflict_token: Option<String>,
    },
}
impl BookingCommand {
    fn normalize(&mut self) -> Result<(), BookingError> {
        let version = match self {
            Self::SaveSpecialist {
                name,
                description,
                expected_version,
                ..
            } => {
                *name = Name::parse(name)?.as_str().to_owned();
                *description = clean_text(description, 500)?;
                *expected_version
            }
            Self::SavePreset {
                name,
                duration_minutes,
                expected_version,
                ..
            } => {
                *name = Name::parse(name)?.as_str().to_owned();
                if duration_minutes.is_some_and(|v| !(1..=1440).contains(&v)) {
                    return Err(BookingError::Invalid);
                }
                *expected_version
            }
            Self::SaveOffer {
                time_zone,
                title,
                timing,
                expected_version,
                conflict_token,
                ..
            } => {
                *time_zone = clean_text(time_zone, 128)?;
                *title = clean_text(title, 120)?;
                timing.fields()?;
                if conflict_token
                    .as_ref()
                    .is_some_and(|v| v.len() != 64 || !v.bytes().all(|c| c.is_ascii_hexdigit()))
                {
                    return Err(BookingError::Invalid);
                }
                *expected_version
            }
        };
        if version < 0 {
            return Err(BookingError::Invalid);
        }
        Ok(())
    }
}
fn clean_text(value: &str, max: usize) -> Result<String, BookingError> {
    let value = value.trim();
    if value.chars().count() > max || value.chars().any(char::is_control) {
        return Err(BookingError::Invalid);
    }
    Ok(value.to_owned())
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "item", rename_all = "snake_case")]
pub enum BookingResource {
    Specialist(Specialist),
    Preset(Preset),
    Offer(Offer),
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BookingReceipt {
    pub resource: BookingResource,
}
#[derive(Debug, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct Conflict {
    pub id: Uuid,
    pub title: String,
    pub starts_local: Option<String>,
    pub ends_local: Option<String>,
    pub time_zone: String,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct ConflictNotice {
    pub token: String,
    pub total: i64,
    pub conflicts: Vec<Conflict>,
}

async fn location_zone(
    session: &mut TenantSession,
    location: LocationId,
    lock: bool,
) -> Result<String, BookingError> {
    let tenant = session.tenant_id();
    // Concurrent editors share this lock. A timezone edit waits until this
    // command has captured a consistent timezone snapshot.
    sqlx::query_scalar(if lock {
        "SELECT time_zone FROM core.locations WHERE tenant_id=$1 AND id=$2 FOR SHARE"
    } else {
        "SELECT time_zone FROM core.locations WHERE tenant_id=$1 AND id=$2"
    })
    .bind(tenant)
    .bind(location)
    .fetch_optional(session.connection())
    .await?
    .ok_or(BookingError::NotFound)
}

pub(crate) async fn lock_calendar(
    session: &mut TenantSession,
    location: LocationId,
    specialist: Uuid,
) -> Result<(), BookingError> {
    let tenant = session.tenant_id();
    sqlx::query_scalar::<_, i64>("SELECT revision FROM bookings.calendars WHERE tenant_id=$1 AND location_id=$2 AND specialist_id=$3 FOR UPDATE")
        .bind(tenant).bind(location).bind(specialist).fetch_optional(session.connection()).await?.ok_or(BookingError::NotFound)?;
    Ok(())
}

// No contacts enter the schedule projection. Employee sees unknown occupancy,
// rather than a false zero caused by the private appointment RLS policy.
const OFFER_PROJECTION: &str = "SELECT v.*, CASE WHEN $8 THEN (SELECT count(*) FROM bookings.appointments a WHERE a.tenant_id=v.tenant_id AND a.offer_id=v.id AND a.status='confirmed') END AS reserved_places, CASE WHEN $8 THEN EXISTS(SELECT 1 FROM bookings.appointments a WHERE a.tenant_id=v.tenant_id AND a.specialist_id=v.specialist_id AND a.offer_id<>v.id AND a.status='confirmed' AND bookings.times_conflict(a.starts_at,a.ends_at,v.starts_at::timestamptz,v.ends_at::timestamptz)) END AS blocked_by_booking FROM bookings.offer_details v WHERE tenant_id=$1 AND location_id=$2 AND specialist_id=$3 AND (date=to_char(bookings.valid_date($4),'YYYY-MM-DD') OR date IS NULL) AND ($5::uuid IS NULL OR id>$5) AND ($6 OR state<>'withdrawn' OR EXISTS(SELECT 1 FROM bookings.appointments a WHERE a.tenant_id=v.tenant_id AND a.offer_id=v.id AND a.status='confirmed')) ORDER BY id LIMIT $7";

pub async fn specialists(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    location: LocationId,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<Specialist>, BookingError> {
    let mut s = TenantSession::begin(pool, user, tenant).await?;
    location_zone(&mut s, location, false).await?;
    let rows=sqlx::query_as("SELECT id,name,description,enabled,version FROM bookings.specialists WHERE tenant_id=$1 AND location_id=$2 AND ($3::uuid IS NULL OR id>$3) ORDER BY id LIMIT $4")
        .bind(tenant).bind(location).bind(after).bind(limit.clamp(1,100)+1).fetch_all(s.connection()).await?;
    s.commit().await?;
    Ok(rows)
}
pub async fn presets(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    location: LocationId,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<Preset>, BookingError> {
    let mut s = TenantSession::begin(pool, user, tenant).await?;
    location_zone(&mut s, location, false).await?;
    let rows=sqlx::query_as("SELECT id,name,duration_minutes,enabled,version FROM bookings.presets WHERE tenant_id=$1 AND location_id=$2 AND ($3::uuid IS NULL OR id>$3) ORDER BY id LIMIT $4")
        .bind(tenant).bind(location).bind(after).bind(limit.clamp(1,100)+1).fetch_all(s.connection()).await?;
    s.commit().await?;
    Ok(rows)
}
pub struct OfferPage<'a> {
    pub specialist: Uuid,
    pub date: &'a str,
    pub after: Option<Uuid>,
    pub limit: i64,
    pub include_withdrawn: bool,
}
pub async fn offers(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    location: LocationId,
    page: OfferPage<'_>,
) -> Result<Vec<Offer>, BookingError> {
    let mut s = TenantSession::begin(pool, user, tenant).await?;
    location_zone(&mut s, location, false).await?;
    let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM bookings.specialists WHERE tenant_id=$1 AND location_id=$2 AND id=$3)")
        .bind(tenant).bind(location).bind(page.specialist).fetch_one(s.connection()).await?;
    if !exists {
        return Err(BookingError::NotFound);
    }
    let staff = s.role() != crate::core::Role::Employee;
    let rows = sqlx::query_as(OFFER_PROJECTION)
        .bind(tenant)
        .bind(location)
        .bind(page.specialist)
        .bind(page.date)
        .bind(page.after)
        .bind(page.include_withdrawn)
        .bind(page.limit.clamp(1, 100) + 1)
        .bind(staff)
        .fetch_all(s.connection())
        .await?;
    // Validate even when an empty relation lets the planner skip the predicate.
    sqlx::query("SELECT bookings.valid_date($1)")
        .bind(page.date)
        .execute(s.connection())
        .await?;
    s.commit().await?;
    Ok(rows)
}

pub async fn execute(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    location: LocationId,
    key: Uuid,
    mut command: BookingCommand,
) -> Result<BookingReceipt, BookingError> {
    command.normalize()?;
    let mut s = TenantSession::begin(pool, user, tenant).await?;
    if !s.role().can_manage_locations() {
        return Err(CoreError::Forbidden.into());
    }
    sqlx::query("SET LOCAL lock_timeout='2s'")
        .execute(s.connection())
        .await?;
    let zone = location_zone(&mut s, location, true).await?;
    let claimed=sqlx::query("INSERT INTO bookings.commands(tenant_id,location_id,actor_id,idempotency_key,command) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
        .bind(tenant).bind(location).bind(user).bind(key).bind(Json(&command)).execute(s.connection()).await?.rows_affected();
    if claimed == 0 {
        let (Json(previous),Json(result)):(Json<BookingCommand>,Json<BookingReceipt>)=sqlx::query_as("SELECT command,result FROM bookings.commands WHERE tenant_id=$1 AND location_id=$2 AND actor_id=$3 AND idempotency_key=$4")
            .bind(tenant).bind(location).bind(user).bind(key).fetch_one(s.connection()).await?;
        if previous != command {
            return Err(BookingError::IdempotencyConflict);
        }
        s.commit().await?;
        return Ok(result);
    }
    match &command {
        BookingCommand::SaveSpecialist {
            id,
            expected_version,
            ..
        } if *expected_version > 0 => lock_calendar(&mut s, location, *id).await?,
        BookingCommand::SaveOffer { specialist_id, .. } => {
            lock_calendar(&mut s, location, *specialist_id).await?
        }
        _ => {}
    }
    let resource = match &command {
        BookingCommand::SaveSpecialist {
            id,
            name,
            description,
            enabled,
            expected_version,
        } => {
            let row=if *expected_version==0 {
                sqlx::query_as("INSERT INTO bookings.specialists(tenant_id,location_id,id,name,description,enabled) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING RETURNING id,name,description,enabled,version")
                    .bind(tenant).bind(location).bind(id).bind(name).bind(description).bind(enabled).fetch_optional(s.connection()).await?
            } else {
                sqlx::query_as("UPDATE bookings.specialists SET name=$4,description=$5,enabled=$6,version=version+1 WHERE tenant_id=$1 AND location_id=$2 AND id=$3 AND version=$7 RETURNING id,name,description,enabled,version")
                    .bind(tenant).bind(location).bind(id).bind(name).bind(description).bind(enabled).bind(expected_version).fetch_optional(s.connection()).await?
            }.ok_or(CoreError::VersionConflict)?;
            BookingResource::Specialist(row)
        }
        BookingCommand::SavePreset {
            id,
            name,
            duration_minutes,
            enabled,
            expected_version,
        } => {
            let row=if *expected_version==0 {
                sqlx::query_as("INSERT INTO bookings.presets(tenant_id,location_id,id,name,duration_minutes,enabled) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING RETURNING id,name,duration_minutes,enabled,version")
                    .bind(tenant).bind(location).bind(id).bind(name).bind(duration_minutes).bind(enabled).fetch_optional(s.connection()).await?
            } else {
                sqlx::query_as("UPDATE bookings.presets SET name=$4,duration_minutes=$5,enabled=$6,version=version+1 WHERE tenant_id=$1 AND location_id=$2 AND id=$3 AND version=$7 RETURNING id,name,duration_minutes,enabled,version")
                    .bind(tenant).bind(location).bind(id).bind(name).bind(duration_minutes).bind(enabled).bind(expected_version).fetch_optional(s.connection()).await?
            }.ok_or(CoreError::VersionConflict)?;
            BookingResource::Preset(row)
        }
        BookingCommand::SaveOffer { .. } => {
            BookingResource::Offer(save_offer(&mut s, location, &zone, &command).await?)
        }
    };
    let receipt = BookingReceipt { resource };
    sqlx::query("UPDATE bookings.commands SET result=$5 WHERE tenant_id=$1 AND location_id=$2 AND actor_id=$3 AND idempotency_key=$4")
        .bind(tenant).bind(location).bind(user).bind(key).bind(Json(&receipt)).execute(s.connection()).await?;
    s.commit().await?;
    Ok(receipt)
}

async fn save_offer(
    s: &mut TenantSession,
    location: LocationId,
    current_zone: &str,
    command: &BookingCommand,
) -> Result<Offer, BookingError> {
    let BookingCommand::SaveOffer {
        id,
        specialist_id,
        time_zone,
        title,
        timing,
        state,
        expected_version,
        conflict_token,
    } = command
    else {
        return Err(BookingError::Invalid);
    };
    let tenant = s.tenant_id();
    let (enabled,generation):(bool,i64)=sqlx::query_as("SELECT enabled,schedule_version FROM bookings.specialists WHERE tenant_id=$1 AND location_id=$2 AND id=$3 FOR UPDATE")
        .bind(tenant).bind(location).bind(specialist_id).fetch_optional(s.connection()).await?.ok_or(BookingError::NotFound)?;
    if !enabled && *state == OfferState::Ready {
        return Err(BookingError::SpecialistDisabled);
    }
    let zone = if *expected_version > 0 {
        let (zone,version):(String,i32)=sqlx::query_as("SELECT time_zone,version FROM bookings.offers WHERE tenant_id=$1 AND location_id=$2 AND specialist_id=$3 AND id=$4")
            .bind(tenant).bind(location).bind(specialist_id).bind(id).fetch_optional(s.connection()).await?.ok_or(BookingError::NotFound)?;
        if version != *expected_version {
            return Err(CoreError::VersionConflict.into());
        }
        zone
    } else {
        current_zone.to_owned()
    };
    if time_zone != &zone {
        return Err(BookingError::TimeZoneChanged);
    }
    let TimingFields {
        mode,
        date,
        start,
        end,
        places,
    } = timing.fields()?;
    let start_instant = start
        .map(|wall| resolve_time(&zone, wall).map(|v| v.timestamp()))
        .transpose()?;
    let end_instant = end
        .map(|wall| resolve_time(&zone, wall).map(|v| v.timestamp()))
        .transpose()?;
    if start_instant.is_some() && end_instant.is_some() && end_instant <= start_instant {
        return Err(BookingError::Invalid);
    }
    let start_utc = start_instant.map(|v| v.to_string());
    let end_utc = end_instant.map(|v| v.to_string());
    if *state != OfferState::Withdrawn && start_utc.is_some() {
        let conflicts:Vec<(Json<Conflict>,i64)>=sqlx::query_as("SELECT jsonb_build_object('id',o.id,'title',o.title,'starts_local',v.starts_local,'ends_local',v.ends_local,'time_zone',v.time_zone),count(*) OVER() FROM bookings.offers o JOIN bookings.offer_details v ON v.tenant_id=o.tenant_id AND v.id=o.id WHERE o.tenant_id=$1 AND o.specialist_id=$2 AND o.id<>$3 AND o.state<>'withdrawn' AND bookings.times_conflict(o.starts_at,o.ends_at,$4::text::timestamptz,$5::text::timestamptz) ORDER BY o.starts_at,o.id LIMIT 20")
            .bind(tenant).bind(specialist_id).bind(id).bind(&start_utc).bind(&end_utc).fetch_all(s.connection()).await?;
        if !conflicts.is_empty() {
            let mut payload = serde_json::to_value(command).map_err(|_| BookingError::Invalid)?;
            payload
                .as_object_mut()
                .ok_or(BookingError::Invalid)?
                .remove("conflict_token");
            let token = hex::encode(Sha256::digest(format!("{generation}:{zone}:{payload}")));
            if conflict_token.as_deref() != Some(token.as_str()) {
                let total = conflicts[0].1;
                return Err(BookingError::Overlap(ConflictNotice {
                    token,
                    total,
                    conflicts: conflicts.into_iter().map(|(Json(v), _)| v).collect(),
                }));
            }
        }
    }
    let changed = if *expected_version == 0 {
        sqlx::query("INSERT INTO bookings.offers(tenant_id,location_id,specialist_id,id,title,mode,local_date,starts_local,ends_local,starts_at,ends_at,time_zone,places,state) VALUES($1,$2,$3,$4,$5,$6,coalesce(bookings.valid_date($7),$8::text::timestamp::date),$8::text::timestamp,$9::text::timestamp,$10::text::timestamptz,$11::text::timestamptz,$12,$13,$14) ON CONFLICT DO NOTHING")
            .bind(tenant).bind(location).bind(specialist_id).bind(id).bind(title).bind(mode).bind(date).bind(start).bind(end).bind(&start_utc).bind(&end_utc).bind(&zone).bind(places).bind(state).execute(s.connection()).await?.rows_affected()
    } else {
        sqlx::query("UPDATE bookings.offers SET title=$5,mode=$6,local_date=coalesce(bookings.valid_date($7),$8::text::timestamp::date),starts_local=$8::text::timestamp,ends_local=$9::text::timestamp,starts_at=$10::text::timestamptz,ends_at=$11::text::timestamptz,places=$12,state=$13,version=version+1 WHERE tenant_id=$1 AND location_id=$2 AND specialist_id=$3 AND id=$4 AND version=$14")
            .bind(tenant).bind(location).bind(specialist_id).bind(id).bind(title).bind(mode).bind(date).bind(start).bind(end).bind(&start_utc).bind(&end_utc).bind(places).bind(state).bind(expected_version).execute(s.connection()).await?.rows_affected()
    };
    if changed != 1 {
        return Err(CoreError::VersionConflict.into());
    }
    sqlx::query("UPDATE bookings.specialists SET schedule_version=schedule_version+1 WHERE tenant_id=$1 AND id=$2")
        .bind(tenant).bind(specialist_id).execute(s.connection()).await?;
    Ok(
        sqlx::query_as("SELECT *,NULL::bigint AS reserved_places,NULL::boolean AS blocked_by_booking FROM bookings.offer_details WHERE tenant_id=$1 AND id=$2")
            .bind(tenant)
            .bind(id)
            .fetch_one(s.connection())
            .await?,
    )
}

#[derive(Serialize, ToSchema)]
pub struct TimeSuggestion {
    pub ends_local: String,
    pub time_zone: String,
}
pub struct TimeRequest<'a> {
    pub starts_local: &'a str,
    pub minutes: i32,
    pub time_zone: &'a str,
    pub offer_id: Option<Uuid>,
}
pub async fn suggest_time(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    location: LocationId,
    request: TimeRequest<'_>,
) -> Result<TimeSuggestion, BookingError> {
    if !(1..=1440).contains(&request.minutes) {
        return Err(BookingError::Invalid);
    }
    let mut s = TenantSession::begin(pool, user, tenant).await?;
    let current = location_zone(&mut s, location, false).await?;
    let zone = if let Some(id) = request.offer_id {
        sqlx::query_scalar(
            "SELECT time_zone FROM bookings.offers WHERE tenant_id=$1 AND location_id=$2 AND id=$3",
        )
        .bind(tenant)
        .bind(location)
        .bind(id)
        .fetch_optional(s.connection())
        .await?
        .ok_or(BookingError::NotFound)?
    } else {
        current
    };
    if request.time_zone != zone {
        return Err(BookingError::TimeZoneChanged);
    }
    let end = duration_end(&zone, request.starts_local, request.minutes)?;
    s.commit().await?;
    Ok(TimeSuggestion {
        ends_local: end,
        time_zone: zone,
    })
}

fn resolve_time(zone: &str, wall: &str) -> Result<Zoned, BookingError> {
    let local: DateTime = wall.parse().map_err(|_| BookingError::Invalid)?;
    if wall.len() != 16 || local.strftime("%Y-%m-%dT%H:%M").to_string() != wall {
        return Err(BookingError::Invalid);
    }
    let tz = jiff::tz::db()
        .get(zone)
        .map_err(|_| BookingError::Invalid)?;
    let candidate = tz.to_ambiguous_zoned(local);
    match candidate.offset() {
        AmbiguousOffset::Gap { .. } => Err(BookingError::TimeGap),
        AmbiguousOffset::Fold { .. } => Err(BookingError::TimeFold),
        AmbiguousOffset::Unambiguous { .. } => {
            candidate.unambiguous().map_err(|_| BookingError::Invalid)
        }
    }
}

fn duration_end(zone: &str, wall: &str, minutes: i32) -> Result<String, BookingError> {
    let start = resolve_time(zone, wall)?;
    let end = start
        .timestamp()
        .checked_add(SignedDuration::from_mins(i64::from(minutes)))
        .map_err(|_| BookingError::Invalid)?
        .to_zoned(start.time_zone().clone())
        .strftime("%Y-%m-%dT%H:%M")
        .to_string();
    resolve_time(zone, &end)?;
    Ok(end)
}

#[cfg(test)]
mod time_tests {
    use super::*;

    #[test]
    fn rejects_gaps_folds_and_noncanonical_wall_times() {
        for (zone, gap, fold) in [
            ("America/New_York", "2026-03-08T02:30", "2026-11-01T01:30"),
            (
                "Australia/Lord_Howe",
                "2026-10-04T02:15",
                "2026-04-05T01:45",
            ),
        ] {
            assert!(matches!(
                resolve_time(zone, gap),
                Err(BookingError::TimeGap)
            ));
            assert!(matches!(
                resolve_time(zone, fold),
                Err(BookingError::TimeFold)
            ));
        }
        for wall in [
            "2026-02-30T10:00",
            "2026-01-01 10:00",
            "2026-01-01T10:00:00",
            "2026-01-01T24:00",
        ] {
            assert!(matches!(
                resolve_time("UTC", wall),
                Err(BookingError::Invalid)
            ));
        }
    }

    #[test]
    fn preset_duration_uses_elapsed_minutes_and_rechecks_ambiguous_end() {
        assert_eq!(
            duration_end("America/New_York", "2026-03-08T01:30", 60).unwrap(),
            "2026-03-08T03:30"
        );
        assert_eq!(
            duration_end("Europe/Moscow", "2026-11-01T23:30", 90).unwrap(),
            "2026-11-02T01:00"
        );
        assert!(matches!(
            duration_end("America/New_York", "2026-11-01T00:30", 60),
            Err(BookingError::TimeFold)
        ));
    }
}
