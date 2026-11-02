use super::{
    AppState,
    api::{ApiError, SignedIn, check_mutation, check_origin, single_header},
    queue_public_http::permit,
};
use crate::{
    auth::token::SecretToken,
    bookings_public::{
        self, BookingAvailability, BookingForm, BookingPublicSettings, PublicBookingError,
        PublicBookingInput, PublicBookingProfile, PublicBookingReceipt,
    },
    core::{LocationId, TenantId},
};
use askama::Template;
use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, FromRequestParts, Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode, header::CONTENT_TYPE, request::Parts},
    response::Html,
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::{future::Future, time::Duration};
use utoipa::ToSchema;
use uuid::Uuid;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/organizations/{tenant_id}/locations/{location_id}/bookings/public-access",
            get(settings).put(configure),
        )
        .route("/book/{slug}", get(page))
        .route("/api/public/bookings/{slug}", get(profile))
        .route(
            "/api/public/bookings/{slug}/availability",
            get(availability),
        )
        .route(
            "/api/public/bookings/{slug}/receipt",
            get(receipt)
                .post(book)
                .layer(DefaultBodyLimit::max(32 * 1024)),
        )
        .route(
            "/assets/bookings.css",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../../web/bookings.css"),
                )
            }),
        )
        .route(
            "/assets/bookings.js",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../web/bookings.js"),
                )
            }),
        )
        .route(
            "/assets/bookings-client.js",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../web/bookings-client.mjs"),
                )
            }),
        )
}
impl From<PublicBookingError> for ApiError {
    fn from(e: PublicBookingError) -> Self {
        match e {
            PublicBookingError::Core(e) => e.into(),
            PublicBookingError::Database(e) => e.into(),
            PublicBookingError::NotFound => Self::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "Booking page or receipt not found",
            ),
            PublicBookingError::Closed => Self::new(
                StatusCode::CONFLICT,
                "booking_closed",
                "Public booking paused",
            ),
            PublicBookingError::RateLimit => Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                "booking_rate_limit",
                "Please retry later",
            ),
            PublicBookingError::Invalid => Self::new(
                StatusCode::BAD_REQUEST,
                "invalid_booking_form",
                "Check booking form and answers",
            ),
            PublicBookingError::Changed => Self::new(
                StatusCode::CONFLICT,
                "booking_changed",
                "Review updated availability or form",
            ),
            PublicBookingError::IdempotencyConflict => Self::new(
                StatusCode::CONFLICT,
                "booking_request_conflict",
                "Capability has a different request",
            ),
            PublicBookingError::Expired => Self::new(
                StatusCode::GONE,
                "booking_access_expired",
                "Receipt access expired",
            ),
            PublicBookingError::Full => {
                Self::new(StatusCode::CONFLICT, "offer_full", "No remaining places")
            }
            PublicBookingError::Busy => Self::new(
                StatusCode::CONFLICT,
                "specialist_busy",
                "Specialist is already booked",
            ),
            PublicBookingError::Unavailable => Self::new(
                StatusCode::CONFLICT,
                "offer_unavailable",
                "Offer unavailable",
            ),
        }
    }
}
async fn bounded<T>(
    future: impl Future<Output = Result<T, PublicBookingError>>,
) -> Result<T, ApiError> {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .map_err(|_| ApiError::unavailable())?
        .map_err(Into::into)
}
pub(super) struct BookingAccess([u8; 32]);
impl FromRequestParts<AppState> for BookingAccess {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, _: &AppState) -> Result<Self, ApiError> {
        if !parts.headers.contains_key("x-booking-access") {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "booking_access_required",
                "Booking access token required",
            ));
        }
        let token = SecretToken::parse(single_header(&parts.headers, "x-booking-access")?)
            .map_err(|_| ApiError::bad_request())?;
        Ok(Self(token.hash()))
    }
}
#[derive(Serialize, ToSchema)]
pub(super) struct BookingAccessResponse {
    #[serde(flatten)]
    settings: BookingPublicSettings,
    public_url: Option<String>,
}
fn links(state: &AppState, settings: BookingPublicSettings) -> BookingAccessResponse {
    let public_url = settings
        .public_slug
        .map(|slug| format!("{}/book/{slug}", state.auth.public_origin));
    BookingAccessResponse {
        settings,
        public_url,
    }
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct BookingPublicConfigureInput {
    enabled: bool,
    form: BookingForm,
    expected_version: i32,
}
#[utoipa::path(get,path="/api/organizations/{tenant_id}/locations/{location_id}/bookings/public-access",tag="bookings",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("location_id"=Uuid,Path)),responses((status=200,body=BookingAccessResponse)))]
pub(super) async fn settings(
    State(state): State<AppState>,
    signed: SignedIn,
    path: Result<Path<(TenantId, LocationId)>, PathRejection>,
) -> Result<Json<BookingAccessResponse>, ApiError> {
    let Path((tenant, location)) = path.map_err(|_| ApiError::bad_request())?;
    let settings = bounded(bookings_public::settings(
        &state.pool,
        signed.user_id,
        tenant,
        location,
    ))
    .await?;
    Ok(Json(links(&state, settings)))
}
#[utoipa::path(put,path="/api/organizations/{tenant_id}/locations/{location_id}/bookings/public-access",tag="bookings",security(("sessionCookie"=[])),params(("tenant_id"=Uuid,Path),("location_id"=Uuid,Path)),request_body=BookingPublicConfigureInput,responses((status=200,body=BookingAccessResponse)))]
pub(super) async fn configure(
    State(state): State<AppState>,
    signed: SignedIn,
    headers: HeaderMap,
    path: Result<Path<(TenantId, LocationId)>, PathRejection>,
    body: Result<Json<BookingPublicConfigureInput>, JsonRejection>,
) -> Result<Json<BookingAccessResponse>, ApiError> {
    check_mutation(&headers, &state.auth, &signed)?;
    let Path((tenant, location)) = path.map_err(|_| ApiError::bad_request())?;
    let Json(v) = body.map_err(|_| ApiError::bad_request())?;
    let result = bounded(bookings_public::configure(
        &state.pool,
        signed.user_id,
        tenant,
        location,
        v.enabled,
        v.form,
        v.expected_version,
    ))
    .await?;
    Ok(Json(links(&state, result)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CalendarQuery {
    month: String,
    date: String,
    specialist_id: Option<Uuid>,
    after: Option<Uuid>,
    version: Option<String>,
    limit: Option<i32>,
}
#[utoipa::path(get,path="/api/public/bookings/{slug}",tag="bookings-public",params(("slug"=Uuid,Path)),responses((status=200,body=PublicBookingProfile)))]
pub(super) async fn profile(
    State(state): State<AppState>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<PublicBookingProfile>, ApiError> {
    let _permit = permit(&state)?;
    let Path(slug) = path.map_err(|_| ApiError::bad_request())?;
    Ok(Json(
        bounded(bookings_public::profile(&state.pool, slug)).await?,
    ))
}
#[utoipa::path(get,path="/api/public/bookings/{slug}/availability",tag="bookings-public",params(("slug"=Uuid,Path),("month"=String,Query),("date"=String,Query),("specialist_id"=Option<Uuid>,Query),("after"=Option<Uuid>,Query),("version"=Option<String>,Query),("limit"=Option<i32>,Query)),responses((status=200,body=BookingAvailability)))]
pub(super) async fn availability(
    State(state): State<AppState>,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<CalendarQuery>, QueryRejection>,
) -> Result<Json<BookingAvailability>, ApiError> {
    let _permit = permit(&state)?;
    let Path(slug) = path.map_err(|_| ApiError::bad_request())?;
    let Query(q) = query.map_err(|_| ApiError::bad_request())?;
    Ok(Json(
        bounded(bookings_public::availability(
            &state.pool,
            slug,
            bookings_public::AvailabilityQuery {
                month: &q.month,
                date: &q.date,
                specialist: q.specialist_id,
                after: q.after,
                version: q.version.as_deref(),
                limit: q.limit.unwrap_or(50),
            },
        ))
        .await?,
    ))
}
#[utoipa::path(get,path="/api/public/bookings/{slug}/receipt",tag="bookings-public",params(("slug"=Uuid,Path),("X-Booking-Access"=String,Header)),responses((status=200,body=PublicBookingReceipt),(status=401,description="Capability required")))]
pub(super) async fn receipt(
    State(state): State<AppState>,
    path: Result<Path<Uuid>, PathRejection>,
    access: BookingAccess,
) -> Result<Json<PublicBookingReceipt>, ApiError> {
    let _permit = permit(&state)?;
    let Path(slug) = path.map_err(|_| ApiError::bad_request())?;
    Ok(Json(
        bounded(bookings_public::receipt(&state.pool, slug, &access.0)).await?,
    ))
}
#[utoipa::path(post,path="/api/public/bookings/{slug}/receipt",tag="bookings-public",params(("slug"=Uuid,Path),("X-Booking-Access"=String,Header)),request_body=PublicBookingInput,responses((status=200,body=PublicBookingReceipt),(status=409,description="Availability or form changed")))]
pub(super) async fn book(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
    access: BookingAccess,
    body: Result<Json<PublicBookingInput>, JsonRejection>,
) -> Result<Json<PublicBookingReceipt>, ApiError> {
    check_origin(&headers, &state.auth)?;
    let _permit = permit(&state)?;
    let Path(slug) = path.map_err(|_| ApiError::bad_request())?;
    let Json(input) = body.map_err(|_| ApiError::bad_request())?;
    Ok(Json(
        bounded(bookings_public::book(&state.pool, slug, &access.0, input)).await?,
    ))
}
#[derive(Template)]
#[template(path = "bookings.html")]
struct BookingPage {
    slug: Uuid,
    profile: Option<PublicBookingProfile>,
    parent_origin: String,
    channel: String,
}
pub(super) fn render_page(
    slug: Uuid,
    profile: Option<PublicBookingProfile>,
    parent_origin: String,
    channel: String,
) -> Result<String, ApiError> {
    BookingPage {
        slug,
        profile,
        parent_origin,
        channel,
    }
    .render()
    .map_err(|_| ApiError::unavailable())
}
pub(super) async fn page(
    State(state): State<AppState>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<(StatusCode, Html<String>), ApiError> {
    let _permit = permit(&state)?;
    let Path(slug) = path.map_err(|_| ApiError::bad_request())?;
    let profile = match tokio::time::timeout(
        Duration::from_secs(5),
        bookings_public::profile(&state.pool, slug),
    )
    .await
    {
        Ok(Ok(p)) => Some(p),
        Ok(Err(PublicBookingError::NotFound)) => None,
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => return Err(ApiError::unavailable()),
    };
    let status = if profile.is_some() {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    };
    let html = render_page(slug, profile, String::new(), String::new())?;
    Ok((status, Html(html)))
}
