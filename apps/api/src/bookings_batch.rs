use crate::{
    bookings::{self, BookingError, Offer, OfferState, Preset, Timing},
    core::{CoreError, LocationId, TenantId, TenantSession, UserId},
};
use jiff::{SignedDuration, tz::TimeZone};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, types::Json};
use utoipa::ToSchema;
use uuid::Uuid;

const MAX_ROWS: usize = 200;
const MAX_DATES: usize = 31;
const MAX_CONFLICTS: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchRequest {
    pub id: Uuid,
    pub specialist_id: Uuid,
    pub time_zone: String,
    pub state: OfferState,
    pub source: BatchSource,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BatchSource {
    CopyDay {
        source_date: String,
        target_dates: Vec<String>,
    },
    PresetSeries {
        preset_id: Uuid,
        dates: Vec<String>,
        start_time: String,
        end_time: String,
        gap_minutes: i32,
    },
}
impl BatchRequest {
    pub(crate) fn normalize(&mut self) -> Result<(), BookingError> {
        if self.time_zone.len() > 128 || self.state == OfferState::Withdrawn {
            return Err(BookingError::InvalidBatch);
        }
        let dates = match &mut self.source {
            BatchSource::CopyDay {
                source_date,
                target_dates,
            } => {
                valid_date(source_date)?;
                if target_dates.contains(source_date) {
                    return Err(BookingError::InvalidBatch);
                }
                target_dates
            }
            BatchSource::PresetSeries {
                dates,
                start_time,
                end_time,
                gap_minutes,
                ..
            } => {
                valid_clock(start_time)?;
                valid_clock(end_time)?;
                if end_time <= start_time || !(0..=1440).contains(gap_minutes) {
                    return Err(BookingError::InvalidBatch);
                }
                dates
            }
        };
        if dates.is_empty() || dates.len() > MAX_DATES {
            return Err(BookingError::InvalidBatch);
        }
        for date in dates.iter() {
            valid_date(date)?;
        }
        dates.sort();
        dates.dedup();
        Ok(())
    }
}
fn valid_date(date: &str) -> Result<(), BookingError> {
    if date.len() != 10 || date.starts_with("0000") {
        return Err(BookingError::InvalidBatch);
    }
    bookings::resolve_time("UTC", &format!("{date}T00:00"))
        .map_err(|_| BookingError::InvalidBatch)?;
    Ok(())
}
fn valid_clock(clock: &str) -> Result<(), BookingError> {
    if clock.len() != 5 {
        return Err(BookingError::InvalidBatch);
    }
    bookings::resolve_time("UTC", &format!("2026-01-01T{clock}"))
        .map_err(|_| BookingError::InvalidBatch)?;
    Ok(())
}
#[derive(Debug, Serialize, ToSchema)]
pub struct BatchTimeIssue {
    pub date: String,
    pub code: String,
}
fn at(zone: &str, wall: &str) -> Result<jiff::Zoned, BookingError> {
    bookings::resolve_time(zone, wall).map_err(|error| match error {
        BookingError::TimeGap => BookingError::BatchTime(BatchTimeIssue {
            date: wall[..10].into(),
            code: "time_gap".into(),
        }),
        BookingError::TimeFold => BookingError::BatchTime(BatchTimeIssue {
            date: wall[..10].into(),
            code: "time_fold".into(),
        }),
        other => other,
    })
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BatchRow {
    pub index: i32,
    pub title: String,
    pub mode: String,
    pub date: String,
    pub starts_local: Option<String>,
    pub ends_local: Option<String>,
    pub starts_at: Option<String>,
    pub ends_at: Option<String>,
    pub time_zone: String,
    pub places: i32,
    pub state: OfferState,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct BatchConflict {
    pub row: i32,
    pub kind: String,
    pub other_row: Option<i32>,
    pub title: String,
    pub starts_local: Option<String>,
    pub ends_local: Option<String>,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct BatchPreview {
    pub token: String,
    pub rows: Vec<BatchRow>,
    pub conflicts: Vec<BatchConflict>,
    pub conflicts_truncated: bool,
    pub has_duplicates: bool,
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BatchReceipt {
    pub id: Uuid,
    pub version: i32,
    pub specialist_id: Uuid,
    pub dates: Vec<String>,
    pub offer_ids: Vec<Uuid>,
}
fn row(
    title: String,
    timing: Timing,
    request: &BatchRequest,
    rows: &mut Vec<BatchRow>,
) -> Result<(), BookingError> {
    if rows.len() >= MAX_ROWS {
        return Err(BookingError::InvalidBatch);
    }
    let (mode, date, start, end, places) = match timing {
        Timing::Interval {
            starts_local,
            ends_local,
        } => (
            "interval",
            starts_local[..10].into(),
            Some(starts_local),
            Some(ends_local),
            1,
        ),
        Timing::StartOnly { starts_local } => (
            "start_only",
            starts_local[..10].into(),
            Some(starts_local),
            None,
            1,
        ),
        Timing::ByAgreement {
            date: Some(date),
            places,
        } => ("by_agreement", date, None, None, places),
        _ => return Err(BookingError::InvalidBatch),
    };
    let starts_at = start
        .as_deref()
        .map(|v| at(&request.time_zone, v).map(|z| z.timestamp().to_string()))
        .transpose()?;
    let ends_at = end
        .as_deref()
        .map(|v| at(&request.time_zone, v).map(|z| z.timestamp().to_string()))
        .transpose()?;
    if starts_at.is_some() && ends_at.is_some() && ends_at <= starts_at {
        return Err(BookingError::InvalidBatch);
    }
    rows.push(BatchRow {
        index: rows.len() as i32 + 1,
        title,
        mode: mode.into(),
        date,
        starts_local: start,
        ends_local: end,
        starts_at,
        ends_at,
        time_zone: request.time_zone.clone(),
        places,
        state: request.state,
    });
    Ok(())
}
fn shift_wall(wall: &str, source: &str, target: &str) -> Result<String, BookingError> {
    // UTC is used only as a calendar arithmetic frame here, not as the appointment zone.
    let delta = bookings::resolve_time("UTC", &format!("{target}T00:00"))?
        .timestamp()
        .as_second()
        - bookings::resolve_time("UTC", &format!("{source}T00:00"))?
            .timestamp()
            .as_second();
    Ok(bookings::resolve_time("UTC", wall)?
        .timestamp()
        .checked_add(SignedDuration::from_secs(delta))
        .map_err(|_| BookingError::InvalidBatch)?
        .to_zoned(TimeZone::UTC)
        .strftime("%Y-%m-%dT%H:%M")
        .to_string())
}
fn conflict(a: &BatchRow, b: &BatchRow) -> bool {
    let (Some(start), Some(other)) = (&a.starts_at, &b.starts_at) else {
        return false;
    };
    match (&a.ends_at, &b.ends_at) {
        (None, None) => start == other,
        (None, Some(end)) => start >= other && start < end,
        (Some(end), None) => other >= start && other < end,
        (Some(end), Some(other_end)) => start < other_end && other < end,
    }
}
fn duplicate(a: &BatchRow, b: &BatchRow) -> bool {
    a.title == b.title
        && a.mode == b.mode
        && a.date == b.date
        && a.starts_at == b.starts_at
        && a.ends_at == b.ends_at
        && a.places == b.places
}
pub async fn preview(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    point: LocationId,
    mut request: BatchRequest,
) -> Result<BatchPreview, BookingError> {
    request.normalize()?;
    let mut s = TenantSession::begin(pool, user, tenant).await?;
    if !s.role().can_manage_locations() {
        return Err(CoreError::Forbidden.into());
    }
    sqlx::query("SET LOCAL lock_timeout='2s'")
        .execute(s.connection())
        .await?;
    let zone = bookings::location_zone(&mut s, point, true).await?;
    bookings::lock_calendar(&mut s, point, request.specialist_id).await?;
    let result = plan(&mut s, point, &zone, &request).await?;
    s.commit().await?;
    Ok(result)
}
async fn plan(
    s: &mut TenantSession,
    point: LocationId,
    zone: &str,
    request: &BatchRequest,
) -> Result<BatchPreview, BookingError> {
    let tenant = s.tenant_id();
    if request.time_zone != zone {
        return Err(BookingError::TimeZoneChanged);
    }
    let (enabled, version, generation, revision): (bool, i32, i64, i64) = sqlx::query_as(
        r#"
        SELECT enabled,version,schedule_version,(SELECT revision FROM bookings.calendars c
            WHERE c.tenant_id=s.tenant_id AND c.specialist_id=s.id)
        FROM bookings.specialists s WHERE tenant_id=$1 AND location_id=$2 AND id=$3 FOR UPDATE
    "#,
    )
    .bind(tenant)
    .bind(point)
    .bind(request.specialist_id)
    .fetch_optional(s.connection())
    .await?
    .ok_or(BookingError::NotFound)?;
    if !enabled && request.state == OfferState::Ready {
        return Err(BookingError::SpecialistDisabled);
    }
    let mut rows = Vec::new();
    let mut preset_version = None;
    match &request.source {
        BatchSource::CopyDay {
            source_date,
            target_dates,
        } => {
            let source:Vec<Offer>=sqlx::query_as(r#"
                SELECT *,NULL::bigint AS reserved_places,NULL::boolean AS blocked_by_booking
                FROM bookings.offer_details WHERE tenant_id=$1 AND location_id=$2 AND specialist_id=$3
                    AND date=$4 AND state<>'withdrawn' ORDER BY starts_local NULLS LAST,id LIMIT 201
            "#).bind(tenant).bind(point).bind(request.specialist_id).bind(source_date).fetch_all(s.connection()).await?;
            if source.is_empty() || source.len() * target_dates.len() > MAX_ROWS {
                return Err(BookingError::InvalidBatch);
            }
            if source.iter().any(|offer| offer.time_zone != zone) {
                return Err(BookingError::TimeZoneChanged);
            }
            for date in target_dates {
                for offer in &source {
                    let timing = match (offer.mode.as_str(), &offer.starts_local, &offer.ends_local)
                    {
                        ("interval", Some(start), Some(end)) => Timing::Interval {
                            starts_local: shift_wall(start, source_date, date)?,
                            ends_local: shift_wall(end, source_date, date)?,
                        },
                        ("start_only", Some(start), _) => Timing::StartOnly {
                            starts_local: shift_wall(start, source_date, date)?,
                        },
                        ("by_agreement", _, _) => Timing::ByAgreement {
                            date: Some(date.clone()),
                            places: offer.places,
                        },
                        _ => return Err(BookingError::InvalidBatch),
                    };
                    row(offer.title.clone(), timing, request, &mut rows)?;
                }
            }
        }
        BatchSource::PresetSeries {
            preset_id,
            dates,
            start_time,
            end_time,
            gap_minutes,
        } => {
            let preset:Preset=sqlx::query_as("SELECT id,name,duration_minutes,enabled,version FROM bookings.presets WHERE tenant_id=$1 AND location_id=$2 AND id=$3 FOR SHARE")
                .bind(tenant).bind(point).bind(preset_id).fetch_optional(s.connection()).await?.ok_or(BookingError::NotFound)?;
            let duration = preset
                .duration_minutes
                .filter(|d| *d > 0)
                .ok_or(BookingError::InvalidBatch)?;
            if !preset.enabled {
                return Err(BookingError::InvalidBatch);
            }
            preset_version = Some(preset.version);
            for date in dates {
                let mut start = at(zone, &format!("{date}T{start_time}"))?;
                let limit = at(zone, &format!("{date}T{end_time}"))?.timestamp();
                let before = rows.len();
                loop {
                    let end = start
                        .timestamp()
                        .checked_add(SignedDuration::from_mins(i64::from(duration)))
                        .map_err(|_| BookingError::InvalidBatch)?;
                    if end > limit {
                        break;
                    }
                    let end_wall = end
                        .to_zoned(start.time_zone().clone())
                        .strftime("%Y-%m-%dT%H:%M")
                        .to_string();
                    row(
                        preset.name.clone(),
                        Timing::Interval {
                            starts_local: start.strftime("%Y-%m-%dT%H:%M").to_string(),
                            ends_local: end_wall,
                        },
                        request,
                        &mut rows,
                    )?;
                    let next = end
                        .checked_add(SignedDuration::from_mins(i64::from(*gap_minutes)))
                        .map_err(|_| BookingError::InvalidBatch)?;
                    start = next.to_zoned(start.time_zone().clone());
                    if start.timestamp() >= limit {
                        break;
                    }
                }
                if rows.len() == before {
                    return Err(BookingError::InvalidBatch);
                }
            }
        }
    }
    let mut conflicts:Vec<BatchConflict>=sqlx::query_as(r#"
        WITH p AS (SELECT * FROM jsonb_to_recordset($4::jsonb) AS p(index integer,title text,mode text,date text,
            starts_at text,ends_at text,places integer)), hits AS (
            SELECT p.index AS row,CASE WHEN o.title=p.title AND o.mode=p.mode AND o.local_date=p.date::date
                AND o.starts_at IS NOT DISTINCT FROM p.starts_at::timestamptz
                AND o.ends_at IS NOT DISTINCT FROM p.ends_at::timestamptz AND o.places=p.places
                THEN 'duplicate' ELSE 'overlap' END AS kind,NULL::integer AS other_row,o.title,
                to_char(o.starts_local,'YYYY-MM-DD"T"HH24:MI') AS starts_local,
                to_char(o.ends_local,'YYYY-MM-DD"T"HH24:MI') AS ends_local
            FROM p JOIN bookings.offers o ON o.tenant_id=$1 AND o.location_id=$2 AND o.specialist_id=$3 AND o.state<>'withdrawn'
            WHERE bookings.times_conflict(o.starts_at,o.ends_at,p.starts_at::timestamptz,p.ends_at::timestamptz)
                OR (o.mode='by_agreement' AND p.mode='by_agreement' AND o.local_date=p.date::date AND o.title=p.title AND o.places=p.places)
            UNION ALL
            SELECT p.index,'booked',NULL::integer,'Занятый приём',
                to_char(a.starts_local,'YYYY-MM-DD"T"HH24:MI'),to_char(a.ends_local,'YYYY-MM-DD"T"HH24:MI')
            FROM p JOIN bookings.appointments a ON a.tenant_id=$1 AND a.location_id=$2 AND a.specialist_id=$3 AND a.status='confirmed'
            WHERE bookings.times_conflict(a.starts_at,a.ends_at,p.starts_at::timestamptz,p.ends_at::timestamptz)
        ) SELECT * FROM hits ORDER BY (kind='duplicate') DESC,row,kind,starts_local NULLS LAST,ends_local NULLS LAST,title LIMIT 101
    "#).bind(tenant).bind(point).bind(request.specialist_id).bind(Json(&rows)).fetch_all(s.connection()).await?;
    let mut has_duplicates = conflicts.iter().any(|c| c.kind == "duplicate");
    let mut truncated = conflicts.len() > MAX_CONFLICTS;
    conflicts.truncate(MAX_CONFLICTS);
    for (i, a) in rows.iter().enumerate() {
        for b in rows.iter().skip(i + 1) {
            let is_duplicate = duplicate(a, b);
            has_duplicates |= is_duplicate;
            if is_duplicate || conflict(a, b) {
                if conflicts.len() < MAX_CONFLICTS {
                    conflicts.push(BatchConflict {
                        row: b.index,
                        kind: if is_duplicate {
                            "duplicate"
                        } else {
                            "internal"
                        }
                        .into(),
                        other_row: Some(a.index),
                        title: a.title.clone(),
                        starts_local: a.starts_local.clone(),
                        ends_local: a.ends_local.clone(),
                    });
                } else {
                    truncated = true;
                }
            }
        }
    }
    let payload = serde_json::to_vec(&(
        "turnesta-batch-v1",
        tenant,
        point,
        request,
        version,
        generation,
        revision,
        preset_version,
        &rows,
        &conflicts,
        truncated,
        has_duplicates,
    ))
    .map_err(|_| BookingError::InvalidBatch)?;
    Ok(BatchPreview {
        token: hex::encode(Sha256::digest(payload)),
        rows,
        conflicts,
        conflicts_truncated: truncated,
        has_duplicates,
    })
}
pub(crate) async fn apply(
    s: &mut TenantSession,
    point: LocationId,
    zone: &str,
    request: &BatchRequest,
    token: &str,
    allow: bool,
) -> Result<BatchReceipt, BookingError> {
    let preview = plan(s, point, zone, request).await?;
    if preview.token != token {
        return Err(BookingError::BatchChanged);
    }
    if preview.conflicts_truncated {
        return Err(BookingError::InvalidBatch);
    }
    if preview.has_duplicates {
        return Err(BookingError::BatchDuplicate);
    }
    if !preview.conflicts.is_empty() && !allow {
        return Err(BookingError::BatchConsent);
    }
    let tenant = s.tenant_id();
    let ids:Vec<Uuid>=sqlx::query_scalar(r#"
        INSERT INTO bookings.offers(tenant_id,location_id,specialist_id,id,title,mode,local_date,starts_local,ends_local,starts_at,ends_at,time_zone,places,state)
        SELECT $1,$2,$3,gen_random_uuid(),p.title,p.mode,p.date::date,p.starts_local::timestamp,p.ends_local::timestamp,
            p.starts_at::timestamptz,p.ends_at::timestamptz,p.time_zone,p.places,p.state
        FROM jsonb_to_recordset($4::jsonb) AS p(index integer,title text,mode text,date text,starts_local text,ends_local text,
            starts_at text,ends_at text,time_zone text,places integer,state text) ORDER BY p.index RETURNING id
    "#).bind(tenant).bind(point).bind(request.specialist_id).bind(Json(&preview.rows)).fetch_all(s.connection()).await?;
    sqlx::query("UPDATE bookings.specialists SET schedule_version=schedule_version+1 WHERE tenant_id=$1 AND location_id=$2 AND id=$3")
        .bind(tenant).bind(point).bind(request.specialist_id).execute(s.connection()).await?;
    let mut dates: Vec<_> = preview.rows.into_iter().map(|r| r.date).collect();
    dates.sort();
    dates.dedup();
    Ok(BatchReceipt {
        id: request.id,
        version: 1,
        specialist_id: request.specialist_id,
        dates,
        offer_ids: ids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> BatchRequest {
        BatchRequest {
            id: Uuid::new_v4(),
            specialist_id: Uuid::new_v4(),
            time_zone: "Europe/Moscow".into(),
            state: OfferState::Ready,
            source: BatchSource::CopyDay {
                source_date: "2026-11-02".into(),
                target_dates: vec![
                    "2026-11-04".into(),
                    "2026-11-03".into(),
                    "2026-11-03".into(),
                ],
            },
        }
    }
    #[test]
    fn batch_dates_are_bounded_canonical_and_do_not_include_source() {
        let mut input = request();
        input.normalize().unwrap();
        let BatchSource::CopyDay { target_dates, .. } = input.source else {
            panic!()
        };
        assert_eq!(target_dates, vec!["2026-11-03", "2026-11-04"]);
        for date in ["2026-11-02", "2026-02-30", "0000-01-01", "2026-1-01"] {
            let mut input = request();
            input.source = BatchSource::CopyDay {
                source_date: "2026-11-02".into(),
                target_dates: vec![date.into()],
            };
            assert!(input.normalize().is_err());
        }
        let mut input = request();
        input.source = BatchSource::CopyDay {
            source_date: "2026-11-02".into(),
            target_dates: vec!["2026-11-03".into(); 32],
        };
        assert!(input.normalize().is_err());
    }
    #[test]
    fn copying_preserves_wall_clock_day_span_and_rejects_target_dst_gap() {
        assert_eq!(
            shift_wall("2024-02-29T01:15", "2024-02-28", "2026-11-02").unwrap(),
            "2026-11-03T01:15"
        );
        assert_eq!(
            shift_wall("2026-11-02T23:30", "2026-11-02", "2026-12-31").unwrap(),
            "2026-12-31T23:30"
        );
        let mut input = request();
        input.time_zone = "America/New_York".into();
        let mut rows = Vec::new();
        let error = row(
            "Visit".into(),
            Timing::StartOnly {
                starts_local: "2026-03-08T02:30".into(),
            },
            &input,
            &mut rows,
        )
        .unwrap_err();
        assert!(
            matches!(error,BookingError::BatchTime(BatchTimeIssue{date,code}) if date=="2026-03-08"&&code=="time_gap")
        );
        assert!(rows.is_empty());
    }
    #[test]
    fn batch_conflicts_keep_half_open_boundaries_and_unknown_ends() {
        let input = request();
        let mut rows = Vec::new();
        for timing in [
            Timing::Interval {
                starts_local: "2026-11-03T10:00".into(),
                ends_local: "2026-11-03T11:00".into(),
            },
            Timing::StartOnly {
                starts_local: "2026-11-03T11:00".into(),
            },
            Timing::StartOnly {
                starts_local: "2026-11-03T10:30".into(),
            },
            Timing::StartOnly {
                starts_local: "2026-11-03T11:00".into(),
            },
        ] {
            row("Visit".into(), timing, &input, &mut rows).unwrap();
        }
        assert!(!conflict(&rows[0], &rows[1]));
        assert!(conflict(&rows[0], &rows[2]));
        assert!(conflict(&rows[2], &rows[0]));
        assert!(duplicate(&rows[1], &rows[3]));
    }
}
