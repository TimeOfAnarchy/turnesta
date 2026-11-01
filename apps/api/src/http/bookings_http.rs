use super::{
    AppState,
    api::{ApiError, SignedIn, check_mutation, single_header},
};
use crate::{
    appointments::{self, Appointment, AppointmentCommand, AppointmentReceipt},
    bookings::{
        self, BookingCommand, BookingError, BookingReceipt, ConflictNotice, Offer, Preset,
        Specialist,
    },
    core::{LocationId, TenantId},
};
use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/organizations/{tenant_id}/locations/{location_id}/bookings/appointments/{appointment_id}",
            get(appointment),
        )
        .route(
            "/api/organizations/{tenant_id}/locations/{location_id}/bookings/appointments",
            get(appointments),
        )
        .route(
            "/api/organizations/{tenant_id}/locations/{location_id}/bookings/appointments/commands",
            post(appointment_command),
        )
        .route(
            "/api/organizations/{tenant_id}/locations/{location_id}/bookings/specialists",
            get(specialists),
        )
        .route(
            "/api/organizations/{tenant_id}/locations/{location_id}/bookings/presets",
            get(presets),
        )
        .route(
            "/api/organizations/{tenant_id}/locations/{location_id}/bookings/offers",
            get(offers),
        )
        .route(
            "/api/organizations/{tenant_id}/locations/{location_id}/bookings/commands",
            post(command),
        )
        .route(
            "/api/organizations/{tenant_id}/locations/{location_id}/bookings/suggest-time",
            get(suggest_time),
        )
}
pub(super) enum BookingHttpError {
    Api(ApiError),
    Overlap(ConflictNotice),
}
impl From<ApiError> for BookingHttpError {
    fn from(e: ApiError) -> Self {
        Self::Api(e)
    }
}
impl From<BookingError> for BookingHttpError {
    fn from(e: BookingError) -> Self {
        Self::Api(match e {
            BookingError::Core(e) => e.into(),
            BookingError::Database(e) => e.into(),
            BookingError::NotFound => ApiError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "Booking resource not found",
            ),
            BookingError::Invalid => ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_booking",
                "Invalid booking fields",
            ),
            BookingError::TimeGap => ApiError::new(
                StatusCode::BAD_REQUEST,
                "time_gap",
                "Local time does not exist in this timezone",
            ),
            BookingError::TimeFold => ApiError::new(
                StatusCode::BAD_REQUEST,
                "time_fold",
                "Local time is ambiguous in this timezone",
            ),
            BookingError::TimeZoneChanged => ApiError::new(
                StatusCode::CONFLICT,
                "booking_time_zone_changed",
                "Review changed timezone before saving",
            ),
            BookingError::SpecialistDisabled => ApiError::new(
                StatusCode::CONFLICT,
                "specialist_disabled",
                "Specialist is disabled",
            ),
            BookingError::IdempotencyConflict => ApiError::new(
                StatusCode::CONFLICT,
                "idempotency_conflict",
                "Key has different command",
            ),
            BookingError::InvalidClient => ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_client",
                "Check client contact fields",
            ),
            BookingError::OfferUnavailable => ApiError::new(
                StatusCode::CONFLICT,
                "offer_unavailable",
                "Offer is closed or specialist is disabled",
            ),
            BookingError::OfferFull => ApiError::new(
                StatusCode::CONFLICT,
                "offer_full",
                "Offer has no remaining places",
            ),
            BookingError::SpecialistBusy => ApiError::new(
                StatusCode::CONFLICT,
                "specialist_busy",
                "Specialist is already booked",
            ),
            BookingError::OfferOccupied => ApiError::new(
                StatusCode::CONFLICT,
                "offer_occupied",
                "Move or cancel appointments before changing occupied time or capacity",
            ),
            BookingError::AppointmentClosed => ApiError::new(
                StatusCode::CONFLICT,
                "appointment_closed",
                "Appointment is already cancelled",
            ),
            BookingError::Overlap(details) => return Self::Overlap(details),
        })
    }
}
impl IntoResponse for BookingHttpError {
    fn into_response(self) -> Response {
        match self {
            Self::Api(e) => e.into_response(),
            Self::Overlap(details) => (
                StatusCode::CONFLICT,
                Json(OverlapResponse {
                    code: "booking_overlap",
                    message: "Review overlapping alternatives before saving",
                    details,
                }),
            )
                .into_response(),
        }
    }
}
#[derive(Serialize, ToSchema)]
pub(super) struct OverlapResponse {
    code: &'static str,
    message: &'static str,
    details: ConflictNotice,
}
async fn bounded<T>(
    future: impl std::future::Future<Output = Result<T, BookingError>>,
) -> Result<T, BookingHttpError> {
    tokio::time::timeout(std::time::Duration::from_secs(5), future)
        .await
        .map_err(|_| ApiError::unavailable())?
        .map_err(Into::into)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CatalogQuery {
    after: Option<Uuid>,
    limit: Option<i32>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OffersQuery {
    specialist_id: Uuid,
    date: String,
    after: Option<Uuid>,
    limit: Option<i32>,
    include_withdrawn: Option<bool>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AppointmentsQuery {
    specialist_id: Uuid,
    date: String,
    after: Option<Uuid>,
    limit: Option<i32>,
    include_cancelled: Option<bool>,
}
#[utoipa::path(get,path="/api/organizations/{tenant_id}/locations/{location_id}/bookings/appointments/{appointment_id}",tag="bookings",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("location_id"=Uuid,Path),("appointment_id"=Uuid,Path)),responses((status=200,body=Appointment),(status=403,description="Role cannot access client contacts")))]
pub(super) async fn appointment(
    State(state): State<AppState>,
    signed: SignedIn,
    path: Result<Path<(TenantId, LocationId, Uuid)>, PathRejection>,
) -> Result<Json<Appointment>, BookingHttpError> {
    let Path((tenant, location, id)) = path.map_err(|_| ApiError::bad_request())?;
    Ok(Json(
        bounded(appointments::get(
            &state.pool,
            signed.user_id,
            tenant,
            location,
            id,
        ))
        .await?,
    ))
}
#[derive(Serialize, ToSchema)]
pub(super) struct AppointmentPage {
    items: Vec<Appointment>,
    next_cursor: Option<Uuid>,
}

#[utoipa::path(get,path="/api/organizations/{tenant_id}/locations/{location_id}/bookings/appointments",tag="bookings",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("location_id"=Uuid,Path),("specialist_id"=Uuid,Query),("date"=String,Query),("after"=Option<Uuid>,Query),("limit"=Option<i32>,Query),("include_cancelled"=Option<bool>,Query)),responses((status=200,body=AppointmentPage),(status=403,description="Role cannot access client contacts")))]
pub(super) async fn appointments(
    State(state): State<AppState>,
    signed: SignedIn,
    path: Result<Path<(TenantId, LocationId)>, PathRejection>,
    query: Result<Query<AppointmentsQuery>, QueryRejection>,
) -> Result<Json<AppointmentPage>, BookingHttpError> {
    let Path((tenant, location)) = path.map_err(|_| ApiError::bad_request())?;
    let Query(q) = query.map_err(|_| ApiError::bad_request())?;
    let limit = limit(q.limit)?;
    let mut items = bounded(appointments::list(
        &state.pool,
        signed.user_id,
        tenant,
        location,
        appointments::AppointmentPage {
            specialist: q.specialist_id,
            date: &q.date,
            after: q.after,
            limit: limit as i64,
            include_cancelled: q.include_cancelled.unwrap_or(false),
        },
    ))
    .await?;
    let next_cursor = cursor(&mut items, limit, |a| a.id);
    Ok(Json(AppointmentPage { items, next_cursor }))
}

#[utoipa::path(post,path="/api/organizations/{tenant_id}/locations/{location_id}/bookings/appointments/commands",tag="bookings",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("location_id"=Uuid,Path),("Idempotency-Key"=Uuid,Header)),request_body=AppointmentCommand,responses((status=200,body=AppointmentReceipt),(status=409,description="Occupancy, version or idempotency conflict")))]
pub(super) async fn appointment_command(
    State(state): State<AppState>,
    signed: SignedIn,
    headers: HeaderMap,
    path: Result<Path<(TenantId, LocationId)>, PathRejection>,
    body: Result<Json<AppointmentCommand>, JsonRejection>,
) -> Result<Json<AppointmentReceipt>, BookingHttpError> {
    check_mutation(&headers, &state.auth, &signed)?;
    let Path((tenant, location)) = path.map_err(|_| ApiError::bad_request())?;
    let Json(command) = body.map_err(|_| ApiError::bad_request())?;
    let key = single_header(&headers, "idempotency-key")?
        .parse()
        .map_err(|_| ApiError::bad_request())?;
    Ok(Json(
        bounded(appointments::execute(
            &state.pool,
            signed.user_id,
            tenant,
            location,
            key,
            command,
        ))
        .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TimeQuery {
    starts_local: String,
    minutes: i32,
    time_zone: String,
    offer_id: Option<Uuid>,
}
#[utoipa::path(get,path="/api/organizations/{tenant_id}/locations/{location_id}/bookings/suggest-time",tag="bookings",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("location_id"=Uuid,Path),("starts_local"=String,Query),("minutes"=i32,Query),("time_zone"=String,Query),("offer_id"=Option<Uuid>,Query)),responses((status=200,body=bookings::TimeSuggestion)))]
pub(super) async fn suggest_time(
    State(state): State<AppState>,
    signed: SignedIn,
    path: Result<Path<(TenantId, LocationId)>, PathRejection>,
    query: Result<Query<TimeQuery>, QueryRejection>,
) -> Result<Json<bookings::TimeSuggestion>, BookingHttpError> {
    let Path((tenant, location)) = path.map_err(|_| ApiError::bad_request())?;
    let Query(q) = query.map_err(|_| ApiError::bad_request())?;
    Ok(Json(
        bounded(bookings::suggest_time(
            &state.pool,
            signed.user_id,
            tenant,
            location,
            bookings::TimeRequest {
                starts_local: &q.starts_local,
                minutes: q.minutes,
                time_zone: &q.time_zone,
                offer_id: q.offer_id,
            },
        ))
        .await?,
    ))
}
fn limit(value: Option<i32>) -> Result<usize, ApiError> {
    match value.unwrap_or(50) {
        v @ 1..=100 => Ok(v as usize),
        _ => Err(ApiError::bad_request()),
    }
}
#[derive(Serialize, ToSchema)]
pub(super) struct BookingSpecialistPage {
    items: Vec<Specialist>,
    next_cursor: Option<Uuid>,
}
#[derive(Serialize, ToSchema)]
pub(super) struct BookingPresetPage {
    items: Vec<Preset>,
    next_cursor: Option<Uuid>,
}
#[derive(Serialize, ToSchema)]
pub(super) struct BookingOfferPage {
    items: Vec<Offer>,
    next_cursor: Option<Uuid>,
}
fn cursor<T>(items: &mut Vec<T>, limit: usize, id: impl Fn(&T) -> Uuid) -> Option<Uuid> {
    if items.len() > limit {
        items.truncate(limit);
        items.last().map(id)
    } else {
        None
    }
}

#[utoipa::path(get,path="/api/organizations/{tenant_id}/locations/{location_id}/bookings/specialists",tag="bookings",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("location_id"=Uuid,Path),("after"=Option<Uuid>,Query),("limit"=Option<i32>,Query)),responses((status=200,body=BookingSpecialistPage)))]
pub(super) async fn specialists(
    State(state): State<AppState>,
    signed: SignedIn,
    path: Result<Path<(TenantId, LocationId)>, PathRejection>,
    query: Result<Query<CatalogQuery>, QueryRejection>,
) -> Result<Json<BookingSpecialistPage>, BookingHttpError> {
    let Path((tenant, location)) = path.map_err(|_| ApiError::bad_request())?;
    let Query(query) = query.map_err(|_| ApiError::bad_request())?;
    let limit = limit(query.limit)?;
    let mut items = bounded(bookings::specialists(
        &state.pool,
        signed.user_id,
        tenant,
        location,
        query.after,
        limit as i64,
    ))
    .await?;
    let next_cursor = cursor(&mut items, limit, |s| s.id);
    Ok(Json(BookingSpecialistPage { items, next_cursor }))
}
#[utoipa::path(get,path="/api/organizations/{tenant_id}/locations/{location_id}/bookings/presets",tag="bookings",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("location_id"=Uuid,Path),("after"=Option<Uuid>,Query),("limit"=Option<i32>,Query)),responses((status=200,body=BookingPresetPage)))]
pub(super) async fn presets(
    State(state): State<AppState>,
    signed: SignedIn,
    path: Result<Path<(TenantId, LocationId)>, PathRejection>,
    query: Result<Query<CatalogQuery>, QueryRejection>,
) -> Result<Json<BookingPresetPage>, BookingHttpError> {
    let Path((tenant, location)) = path.map_err(|_| ApiError::bad_request())?;
    let Query(query) = query.map_err(|_| ApiError::bad_request())?;
    let limit = limit(query.limit)?;
    let mut items = bounded(bookings::presets(
        &state.pool,
        signed.user_id,
        tenant,
        location,
        query.after,
        limit as i64,
    ))
    .await?;
    let next_cursor = cursor(&mut items, limit, |s| s.id);
    Ok(Json(BookingPresetPage { items, next_cursor }))
}
#[utoipa::path(get,path="/api/organizations/{tenant_id}/locations/{location_id}/bookings/offers",tag="bookings",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("location_id"=Uuid,Path),("specialist_id"=Uuid,Query),("date"=String,Query),("after"=Option<Uuid>,Query),("limit"=Option<i32>,Query),("include_withdrawn"=Option<bool>,Query)),responses((status=200,body=BookingOfferPage)))]
pub(super) async fn offers(
    State(state): State<AppState>,
    signed: SignedIn,
    path: Result<Path<(TenantId, LocationId)>, PathRejection>,
    query: Result<Query<OffersQuery>, QueryRejection>,
) -> Result<Json<BookingOfferPage>, BookingHttpError> {
    let Path((tenant, location)) = path.map_err(|_| ApiError::bad_request())?;
    let Query(query) = query.map_err(|_| ApiError::bad_request())?;
    let limit = limit(query.limit)?;
    let mut items = bounded(bookings::offers(
        &state.pool,
        signed.user_id,
        tenant,
        location,
        bookings::OfferPage {
            specialist: query.specialist_id,
            date: &query.date,
            after: query.after,
            limit: limit as i64,
            include_withdrawn: query.include_withdrawn.unwrap_or(false),
        },
    ))
    .await?;
    let next_cursor = cursor(&mut items, limit, |s| s.id);
    Ok(Json(BookingOfferPage { items, next_cursor }))
}
#[utoipa::path(post,path="/api/organizations/{tenant_id}/locations/{location_id}/bookings/commands",tag="bookings",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("location_id"=Uuid,Path),("Idempotency-Key"=Uuid,Header)),request_body=BookingCommand,responses((status=200,body=BookingReceipt),(status=409,body=OverlapResponse,description="Overlap acknowledgement or version/idempotency conflict")))]
pub(super) async fn command(
    State(state): State<AppState>,
    signed: SignedIn,
    headers: HeaderMap,
    path: Result<Path<(TenantId, LocationId)>, PathRejection>,
    body: Result<Json<BookingCommand>, JsonRejection>,
) -> Result<Json<BookingReceipt>, BookingHttpError> {
    check_mutation(&headers, &state.auth, &signed)?;
    let Path((tenant, location)) = path.map_err(|_| ApiError::bad_request())?;
    let Json(command) = body.map_err(|_| ApiError::bad_request())?;
    let key = single_header(&headers, "idempotency-key")?
        .parse()
        .map_err(|_| ApiError::bad_request())?;
    Ok(Json(
        bounded(bookings::execute(
            &state.pool,
            signed.user_id,
            tenant,
            location,
            key,
            command,
        ))
        .await?,
    ))
}
