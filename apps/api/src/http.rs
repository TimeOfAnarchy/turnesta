use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use crate::auth::AuthConfig;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, MatchedPath, Request, State},
    http::{HeaderValue, StatusCode, header::CACHE_CONTROL},
    middleware::{self, Next},
    response::Redirect,
    response::Response,
    routing::get,
};
use serde::Serialize;
use sqlx::PgPool;
use tracing::Instrument;
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

mod api;
mod bookings_http;
mod bookings_public_http;
mod bookings_widget_http;
mod catalog_http;
mod queue_events;
mod queue_http;
mod queue_public_http;
mod queue_stations_http;
mod queue_tips_http;
mod tips_http;

#[derive(Clone)]
pub struct AppState {
    pool: PgPool,
    ready_timeout: Duration,
    draining: Arc<AtomicBool>,
    auth: AuthConfig,
    login_gate: Arc<tokio::sync::Semaphore>,
    web_dist: PathBuf,
    payments: crate::payments::PaymentConfig,
    public_gate: Arc<tokio::sync::Semaphore>,
    queue_live: Option<Arc<crate::queue_live::LiveHub>>,
}

impl AppState {
    pub fn new(pool: PgPool, ready_timeout: Duration) -> Self {
        Self {
            pool,
            ready_timeout,
            draining: Arc::new(AtomicBool::new(false)),
            auth: AuthConfig::default(),
            login_gate: Arc::new(tokio::sync::Semaphore::new(16)),
            web_dist: "apps/web/dist".into(),
            payments: crate::payments::PaymentConfig::default(),
            public_gate: Arc::new(tokio::sync::Semaphore::new(32)),
            queue_live: None,
        }
    }

    pub fn begin_shutdown(&self) {
        self.draining.store(true, Ordering::Release);
        if let Some(hub) = &self.queue_live {
            hub.shutdown();
        }
    }

    pub fn with_queue_live(mut self, hub: Arc<crate::queue_live::LiveHub>) -> Self {
        self.queue_live = Some(hub);
        self
    }

    pub fn with_auth(mut self, auth: AuthConfig) -> Self {
        self.auth = auth;
        self
    }

    pub fn with_web_dist(mut self, path: PathBuf) -> Self {
        self.web_dist = path;
        self
    }

    pub fn with_payments(mut self, config: crate::payments::PaymentConfig) -> Self {
        self.payments = config;
        self
    }
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum HealthStatus {
    Alive,
    Ready,
    NotReady,
}

#[derive(Serialize, ToSchema)]
struct HealthResponse {
    status: HealthStatus,
    service: &'static str,
    version: &'static str,
}

impl HealthResponse {
    fn new(status: HealthStatus) -> Self {
        Self {
            status,
            service: "turnesta-api",
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

#[derive(Serialize, ToSchema)]
struct ErrorResponse {
    code: &'static str,
    message: &'static str,
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Turnesta API", description = "Turnesta operational endpoints"),
    paths(
        bookings_http::batch_preview,
        bookings_widget_http::settings,
        bookings_widget_http::configure,
        bookings_public_http::settings,
        bookings_public_http::configure,
        bookings_public_http::profile,
        bookings_public_http::availability,
        bookings_public_http::book,
        bookings_public_http::receipt,
        bookings_http::appointments,
        bookings_http::appointment,
        bookings_http::appointment_command,
        bookings_http::specialists,
        bookings_http::presets,
        bookings_http::offers,
        bookings_http::command,
        bookings_http::suggest_time,
        health,
        ready,
        api::login,
        api::session,
        api::logout,
        api::organizations,
        api::organization,
        api::locations,
        api::create_location,
        api::rename_organization,
        api::update_location,
        api::time_zones,
        tips_http::recipients,
        tips_http::create_recipient,
        tips_http::attempts,
        tips_http::reconcile,
        tips_http::create_attempt,
        tips_http::complete_checkout,
        tips_http::public_status,
        tips_http::webhook,
        queue_http::queues,
        queue_http::create_queue,
        queue_http::get_queue,
        queue_http::tickets,
        queue_http::command,
        queue_public_http::settings,
        queue_public_http::configure,
        queue_public_http::configure_display,
        queue_tips_http::settings,
        queue_tips_http::configure,
        queue_stations_http::stations,
        queue_stations_http::create,
        queue_stations_http::update,
        catalog_http::services,
        catalog_http::create,
        catalog_http::update,
        catalog_http::public_services,
        queue_public_http::display,
        queue_events::staff,
        queue_events::public,
        queue_public_http::qr,
        queue_public_http::profile,
        queue_public_http::ticket,
        queue_public_http::issue
    ),
    components(schemas(
        crate::bookings_batch::BatchRequest,
        crate::bookings_batch::BatchSource,
        crate::bookings_batch::BatchRow,
        crate::bookings_batch::BatchConflict,
        crate::bookings_batch::BatchPreview,
        crate::bookings_batch::BatchReceipt,
        crate::bookings_batch::BatchTimeIssue,
        crate::bookings_widget::WidgetSettings,
        crate::bookings_widget::WidgetConfigure,
        bookings_widget_http::WidgetResponse,
        crate::bookings_public::BookingForm,
        crate::bookings_public::BookingFormField,
        crate::bookings_public::BookingFieldKind,
        crate::bookings_public::PublicBookingProfile,
        crate::bookings_public::PublicBookingSpecialist,
        crate::bookings_public::PublicBookingOffer,
        crate::bookings_public::BookingCalendarDay,
        crate::bookings_public::BookingAvailability,
        crate::bookings_public::PublicBookingInput,
        crate::bookings_public::PublicBookingReceipt,
        bookings_public_http::BookingPublicConfigureInput,
        bookings_public_http::BookingAccessResponse,
        crate::appointments::Appointment,
        crate::appointments::ClientInput,
        crate::appointments::AppointmentCommand,
        crate::appointments::AppointmentReceipt,
        crate::appointments::AppointmentResource,
        bookings_http::AppointmentPage,
        crate::bookings::Specialist,
        crate::bookings::Preset,
        crate::bookings::Offer,
        crate::bookings::Timing,
        crate::bookings::TimeSuggestion,
        crate::bookings::OfferState,
        crate::bookings::BookingCommand,
        crate::bookings::BookingReceipt,
        crate::bookings::BookingResource,
        crate::bookings::Conflict,
        crate::bookings::ConflictNotice,
        bookings_http::BookingSpecialistPage,
        bookings_http::BookingPresetPage,
        bookings_http::BookingOfferPage,
        bookings_http::OverlapResponse,
        HealthResponse,
        HealthStatus,
        ErrorResponse,
        api::LoginRequest,
        api::SessionResponse,
        api::CreateLocationRequest,
        api::RenameOrganizationRequest,
        api::UpdateLocationRequest,
        api::TimeZoneList,
        api::OrganizationSummary,
        api::OrganizationList,
        api::LocationList,
        crate::core::Organization,
        crate::core::Location,
        crate::core::Role,
        crate::tips::Recipient,
        crate::tips::Attempt,
        crate::tips::PublicAttempt,
        crate::tips::MockEvent,
        tips_http::CreateRecipientRequest,
        tips_http::RecipientPage,
        tips_http::AttemptPage,
        tips_http::CreateAttemptRequest,
        tips_http::CheckoutLinks,
        tips_http::CompleteCheckoutRequest,
        tips_http::StatusLink,
        tips_http::WebhookReceipt,
        crate::queue::Queue,
        crate::queue::Ticket,
        crate::queue::TicketStatus,
        crate::queue::Command,
        crate::queue::CommandResult,
        queue_http::CreateQueueRequest,
        queue_http::QueuePage,
        queue_http::TicketList,
        queue_public_http::SettingsResponse,
        queue_public_http::ConfigureRequest,
        queue_public_http::ConfigureDisplayRequest,
        queue_public_http::IssueRequest,
        crate::queue_public::PublicProfile,
        crate::queue_public::PublicTicket,
        crate::queue_stations::Station,
        queue_stations_http::StationPage,
        queue_stations_http::StationInput,
        queue_stations_http::UpdateStationInput,
        crate::catalog::Service,
        crate::queue_public::PublicService,
        catalog_http::ServicePage,
        catalog_http::PublicServicePage,
        catalog_http::ServiceInput,
        catalog_http::UpdateInput,
        crate::queue_public::PublicTipLink,
        crate::queue_tips::TipSettings,
        queue_tips_http::ConfigureTipsRequest,
        crate::queue_public::DisplayTicket,
        crate::queue_public::PublicDisplay
    ))
)]
struct ApiDoc;

pub fn router(state: AppState) -> Router {
    use tower_http::services::{ServeDir, ServeFile};
    let web_root = state.web_dist.clone();
    // Assets have a strict 404; deep cabinet routes return the SPA document.
    let cabinet = Router::new()
        .route("/app", get(|| async { Redirect::permanent("/app/") }))
        .nest_service("/app/assets", ServeDir::new(web_root.join("assets")))
        .route_service(
            "/app/favicon.svg",
            ServeFile::new(web_root.join("favicon.svg")),
        )
        .route_service("/app/", ServeFile::new(web_root.join("index.html")))
        .route_service("/app/{*path}", ServeFile::new(web_root.join("index.html")));
    Router::new()
        .route("/health", get(health))
        .route("/health/ready", get(ready))
        .route("/openapi.json", get(openapi))
        .merge(api::routes())
        .merge(tips_http::routes())
        .merge(queue_http::routes())
        .merge(queue_events::routes())
        .merge(queue_public_http::routes())
        .merge(queue_tips_http::routes())
        .merge(catalog_http::routes())
        .merge(bookings_http::routes())
        .merge(bookings_public_http::routes())
        .merge(bookings_widget_http::routes())
        .merge(queue_stations_http::routes())
        .merge(cabinet)
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn(request_context))
        .with_state(state)
}

#[utoipa::path(
    get,
    path = "/health",
    tag = "operations",
    responses((status = 200, description = "HTTP process is alive", body = HealthResponse))
)]
async fn health() -> Json<HealthResponse> {
    Json(HealthResponse::new(HealthStatus::Alive))
}

#[utoipa::path(
    get,
    path = "/health/ready",
    tag = "operations",
    responses(
        (status = 200, description = "Database is reachable", body = HealthResponse),
        (status = 503, description = "Dependency unavailable or server draining", body = HealthResponse)
    )
)]
async fn ready(State(state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    if state.draining.load(Ordering::Acquire) {
        return not_ready();
    }

    let check = tokio::time::timeout(
        state.ready_timeout,
        sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&state.pool),
    )
    .await;

    match check {
        Ok(Ok(1)) if !state.draining.load(Ordering::Acquire) => (
            StatusCode::OK,
            Json(HealthResponse::new(HealthStatus::Ready)),
        ),
        Ok(Err(error)) => {
            tracing::warn!(error = %error, "database readiness check failed");
            not_ready()
        }
        _ => not_ready(),
    }
}

fn not_ready() -> (StatusCode, Json<HealthResponse>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(HealthResponse::new(HealthStatus::NotReady)),
    )
}

async fn openapi(State(state): State<AppState>) -> Json<utoipa::openapi::OpenApi> {
    use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityScheme};
    let mut document = ApiDoc::openapi();
    if let Some(components) = document.components.as_mut() {
        components.add_security_scheme(
            "sessionCookie",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new(
                state.auth.session_cookie_name(),
            ))),
        );
        components.add_security_scheme(
            "queueTicket",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("X-Queue-Ticket"))),
        );
    }
    Json(document)
}

async fn not_found() -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorResponse {
            code: "not_found",
            message: "Route not found",
        }),
    )
}

async fn method_not_allowed() -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(ErrorResponse {
            code: "method_not_allowed",
            message: "Method not allowed",
        }),
    )
}

async fn request_context(request: Request, next: Next) -> Response {
    let is_cabinet_asset = request.uri().path().starts_with("/app/assets/");
    let request_id = Uuid::new_v4().to_string();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or("<unmatched>")
        .to_owned();
    let span = tracing::info_span!(
        "http_request",
        request_id = %request_id,
        method = %request.method(),
        route = %route,
    );

    async move {
        let started = Instant::now();
        let mut response = next.run(request).await;
        response.headers_mut().insert(
            "x-request-id",
            HeaderValue::from_str(&request_id).expect("UUID is a valid header value"),
        );
        let cache_control = if is_cabinet_asset && response.status().is_success() {
            "public, max-age=31536000, immutable"
        } else { "no-store" };
        response.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static(cache_control));
        response.headers_mut().insert("x-content-type-options", HeaderValue::from_static("nosniff"));
        response.headers_mut().insert("referrer-policy", HeaderValue::from_static("no-referrer"));
        let ancestors = response.extensions_mut().remove::<WidgetFrameAncestor>();
        let policy = ancestors.map_or_else(
            || HeaderValue::from_static("default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'"),
            |policy| policy.0,
        );
        response.headers_mut().insert("content-security-policy", policy);
        tracing::info!(
            status = response.status().as_u16(),
            duration_ms = started.elapsed().as_secs_f64() * 1_000.0,
            "request completed"
        );
        response
    }
    .instrument(span)
    .await
}

// Only the validated widget HTML route can supply this response extension.
#[derive(Clone)]
struct WidgetFrameAncestor(HeaderValue);
