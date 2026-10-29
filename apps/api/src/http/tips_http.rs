use std::{future::Future, time::Duration};

use askama::Template;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{
        DefaultBodyLimit, Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode, header::CONTENT_TYPE},
    response::Html,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::{
    AppState,
    api::{ApiError, SignedIn, check_mutation, check_origin, single_header},
};
use crate::{
    core::{LocationId, Name, TenantId},
    tips::{self, Attempt, MockEvent, Recipient, TipsError},
};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/organizations/{tenant_id}/tips/recipients",
            get(recipients).post(create_recipient),
        )
        .route(
            "/api/organizations/{tenant_id}/tips/attempts",
            get(attempts),
        )
        .route(
            "/api/organizations/{tenant_id}/tips/attempts/{attempt_id}/reconcile",
            post(reconcile),
        )
        .route("/tips/{slug}", get(profile_page))
        .route("/mock-payments/{token}", get(checkout_page))
        .route("/tips/status/{token}", get(status_page))
        .route("/api/public/tips/{slug}/attempts", post(create_attempt))
        .route("/api/public/tips/status/{token}", get(public_status))
        .route(
            "/api/mock-payments/{token}/complete",
            post(complete_checkout),
        )
        .route(
            "/hooks/tips/mock",
            post(webhook).layer(DefaultBodyLimit::max(8192)),
        )
        .route(
            "/assets/tips.css",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../../web/tips.css"),
                )
            }),
        )
        .route(
            "/assets/tips.js",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../web/tips.js"),
                )
            }),
        )
}

impl From<TipsError> for ApiError {
    fn from(error: TipsError) -> Self {
        match error {
            TipsError::Core(error) => error.into(),
            TipsError::Database(error) => error.into(),
            TipsError::Forbidden => Self::forbidden(),
            TipsError::NotFound => not_found(),
            TipsError::InvalidAmount => Self::new(
                StatusCode::BAD_REQUEST,
                "invalid_amount",
                "Amount must be 100..1000000 minor RUB units",
            ),
            TipsError::IdempotencyConflict => Self::new(
                StatusCode::CONFLICT,
                "idempotency_conflict",
                "The same request key has different payment details",
            ),
            TipsError::StateConflict => Self::new(
                StatusCode::CONFLICT,
                "state_conflict",
                "The provider has already recorded a different outcome",
            ),
            TipsError::EventMismatch => Self::new(
                StatusCode::CONFLICT,
                "event_mismatch",
                "Payment event does not match the recorded attempt",
            ),
        }
    }
}

fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", "Tips record not found")
}
pub(super) fn enabled(state: &AppState) -> Result<(), ApiError> {
    if state.payments.enabled() {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "tips_disabled",
            "Tips mock mode is disabled",
        ))
    }
}
async fn bounded<T>(future: impl Future<Output = Result<T, TipsError>>) -> Result<T, ApiError> {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .map_err(|_| ApiError::unavailable())?
        .map_err(Into::into)
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateRecipientRequest {
    display_name: String,
    location_id: LocationId,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Pagination {
    after: Option<Uuid>,
    limit: Option<i64>,
}
impl Pagination {
    fn limit(&self) -> Result<i64, ApiError> {
        let value = self.limit.unwrap_or(50);
        if (1..=100).contains(&value) {
            Ok(value)
        } else {
            Err(ApiError::bad_request())
        }
    }
}
#[derive(Serialize, ToSchema)]
pub(super) struct RecipientPage {
    items: Vec<Recipient>,
    next_cursor: Option<Uuid>,
}
#[derive(Serialize, ToSchema)]
pub(super) struct AttemptPage {
    items: Vec<Attempt>,
    next_cursor: Option<Uuid>,
}

#[utoipa::path(get, path="/api/organizations/{tenant_id}/tips/recipients", tag="tips", security(("sessionCookie"=[])), params(("tenant_id"=Uuid,Path), ("after"=Option<Uuid>,Query), ("limit"=Option<i64>,Query)), responses((status=200,body=RecipientPage),(status=403,description="Owner/manager required")))]
pub(super) async fn recipients(
    State(state): State<AppState>,
    signed: SignedIn,
    path: Result<Path<TenantId>, PathRejection>,
    query: Result<Query<Pagination>, QueryRejection>,
) -> Result<Json<RecipientPage>, ApiError> {
    enabled(&state)?;
    let Path(tenant) = path.map_err(|_| ApiError::bad_request())?;
    let Query(page) = query.map_err(|_| ApiError::bad_request())?;
    let limit = page.limit()?;
    let mut items = bounded(tips::recipients(
        &state.pool,
        signed.user_id,
        tenant,
        page.after,
        limit,
    ))
    .await?;
    let next_cursor = if items.len() > limit as usize {
        items.truncate(limit as usize);
        items.last().map(|item| item.id)
    } else {
        None
    };
    Ok(Json(RecipientPage { items, next_cursor }))
}

#[utoipa::path(post, path="/api/organizations/{tenant_id}/tips/recipients", tag="tips", security(("sessionCookie"=[])), params(("tenant_id"=Uuid,Path)), request_body=CreateRecipientRequest, responses((status=201,body=Recipient),(status=403,description="Owner/manager and CSRF required")))]
pub(super) async fn create_recipient(
    State(state): State<AppState>,
    signed: SignedIn,
    headers: HeaderMap,
    path: Result<Path<TenantId>, PathRejection>,
    body: Result<Json<CreateRecipientRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Recipient>), ApiError> {
    enabled(&state)?;
    check_mutation(&headers, &state.auth, &signed)?;
    let Path(tenant) = path.map_err(|_| ApiError::bad_request())?;
    let Json(body) = body.map_err(|_| ApiError::bad_request())?;
    let name = Name::parse(&body.display_name).map_err(ApiError::from)?;
    let recipient = bounded(tips::create_recipient(
        &state.pool,
        signed.user_id,
        tenant,
        body.location_id,
        &name,
    ))
    .await?;
    Ok((StatusCode::CREATED, Json(recipient)))
}

#[utoipa::path(get, path="/api/organizations/{tenant_id}/tips/attempts", tag="tips", security(("sessionCookie"=[])), params(("tenant_id"=Uuid,Path), ("after"=Option<Uuid>,Query), ("limit"=Option<i64>,Query)), responses((status=200,body=AttemptPage),(status=403,description="Owner/manager required")))]
pub(super) async fn attempts(
    State(state): State<AppState>,
    signed: SignedIn,
    path: Result<Path<TenantId>, PathRejection>,
    query: Result<Query<Pagination>, QueryRejection>,
) -> Result<Json<AttemptPage>, ApiError> {
    enabled(&state)?;
    let Path(tenant) = path.map_err(|_| ApiError::bad_request())?;
    let Query(page) = query.map_err(|_| ApiError::bad_request())?;
    let limit = page.limit()?;
    let mut items = bounded(tips::attempts(
        &state.pool,
        signed.user_id,
        tenant,
        page.after,
        limit,
    ))
    .await?;
    let next_cursor = if items.len() > limit as usize {
        items.truncate(limit as usize);
        items.last().map(|item| item.id)
    } else {
        None
    };
    Ok(Json(AttemptPage { items, next_cursor }))
}

#[utoipa::path(post, path="/api/organizations/{tenant_id}/tips/attempts/{attempt_id}/reconcile", tag="tips", security(("sessionCookie"=[])), params(("tenant_id"=Uuid,Path),("attempt_id"=Uuid,Path)), responses((status=200,body=Attempt),(status=403,description="Owner/manager and CSRF required")))]
pub(super) async fn reconcile(
    State(state): State<AppState>,
    signed: SignedIn,
    headers: HeaderMap,
    path: Result<Path<(TenantId, Uuid)>, PathRejection>,
) -> Result<Json<Attempt>, ApiError> {
    enabled(&state)?;
    check_mutation(&headers, &state.auth, &signed)?;
    let Path((tenant, attempt)) = path.map_err(|_| ApiError::bad_request())?;
    Ok(Json(
        bounded(tips::reconcile_attempt(
            &state.pool,
            signed.user_id,
            tenant,
            attempt,
        ))
        .await?,
    ))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateAttemptRequest {
    amount_minor: i64,
    idempotency_key: Uuid,
}
#[derive(Serialize, ToSchema)]
pub(super) struct CheckoutLinks {
    checkout_url: String,
    status_url: String,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CompleteCheckoutRequest {
    outcome: String,
}
#[derive(Serialize, ToSchema)]
pub(super) struct StatusLink {
    status_url: String,
}

#[utoipa::path(post, path="/api/public/tips/{slug}/attempts", tag="tips-public", params(("slug"=Uuid,Path)), request_body=CreateAttemptRequest, responses((status=201,body=CheckoutLinks),(status=409,description="Idempotency conflict")))]
pub(super) async fn create_attempt(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
    body: Result<Json<CreateAttemptRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<CheckoutLinks>), ApiError> {
    enabled(&state)?;
    check_origin(&headers, &state.auth)?;
    let _permit = state.public_gate.try_acquire().map_err(|_| {
        ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_requests",
            "Please retry later",
        )
    })?;
    let Path(slug) = path.map_err(|_| ApiError::bad_request())?;
    let Json(body) = body.map_err(|_| ApiError::bad_request())?;
    let checkout = bounded(tips::create_attempt(
        &state.pool,
        slug,
        body.idempotency_key,
        body.amount_minor,
    ))
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(CheckoutLinks {
            checkout_url: format!("/mock-payments/{}", checkout.checkout_token),
            status_url: format!("/tips/status/{}", checkout.status_token),
        }),
    ))
}

#[utoipa::path(post, path="/api/mock-payments/{token}/complete", tag="tips-public", params(("token"=Uuid,Path)), request_body=CompleteCheckoutRequest, responses((status=200,body=StatusLink),(status=409,description="Provider already finalized")))]
pub(super) async fn complete_checkout(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
    body: Result<Json<CompleteCheckoutRequest>, JsonRejection>,
) -> Result<Json<StatusLink>, ApiError> {
    enabled(&state)?;
    check_origin(&headers, &state.auth)?;
    let _permit = state.public_gate.try_acquire().map_err(|_| {
        ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_requests",
            "Please retry later",
        )
    })?;
    let Path(token) = path.map_err(|_| ApiError::bad_request())?;
    let Json(body) = body.map_err(|_| ApiError::bad_request())?;
    if !matches!(body.outcome.as_str(), "succeeded" | "failed") {
        return Err(ApiError::bad_request());
    }
    let status = bounded(tips::complete_mock_checkout(
        &state.pool,
        token,
        &body.outcome,
    ))
    .await?;
    Ok(Json(StatusLink {
        status_url: format!("/tips/status/{status}"),
    }))
}

#[utoipa::path(get, path="/api/public/tips/status/{token}", tag="tips-public", params(("token"=Uuid,Path)), responses((status=200,body=tips::PublicAttempt),(status=404,description="Not found")))]
pub(super) async fn public_status(
    State(state): State<AppState>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<tips::PublicAttempt>, ApiError> {
    enabled(&state)?;
    let Path(token) = path.map_err(|_| ApiError::bad_request())?;
    Ok(Json(
        bounded(tips::public_attempt(&state.pool, token))
            .await?
            .ok_or_else(not_found)?,
    ))
}

#[derive(Serialize, ToSchema)]
pub(super) struct WebhookReceipt {
    accepted: bool,
    processed: bool,
}

#[utoipa::path(post,path="/hooks/tips/mock",tag="tips-webhooks",request_body=MockEvent,responses((status=200,body=WebhookReceipt),(status=403,description="Invalid signature"),(status=409,description="Correlation mismatch")))]
pub(super) async fn webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<WebhookReceipt>, ApiError> {
    enabled(&state)?;
    if single_header(&headers, "content-type")?
        .split(';')
        .next()
        .map(str::trim)
        != Some("application/json")
    {
        return Err(ApiError::bad_request());
    }
    state
        .payments
        .verify(
            single_header(&headers, "x-turnesta-mock-timestamp")?,
            single_header(&headers, "x-turnesta-mock-signature")?,
            &body,
        )
        .map_err(|_| ApiError::forbidden())?;
    let event: MockEvent = serde_json::from_slice(&body).map_err(|_| ApiError::bad_request())?;
    let processed = bounded(tips::accept_event(&state.pool, &event)).await?;
    Ok(Json(WebhookReceipt {
        accepted: true,
        processed,
    }))
}

#[derive(Template)]
#[template(path = "tip_profile.html")]
struct ProfileTemplate<'a> {
    profile: &'a tips::PublicRecipient,
    key: Uuid,
}
#[derive(Template)]
#[template(path = "tip_checkout.html")]
struct CheckoutTemplate<'a> {
    checkout: &'a tips::PublicCheckout,
    token: Uuid,
    amount: String,
    pending: bool,
}
#[derive(Template)]
#[template(path = "tip_status.html")]
struct StatusTemplate<'a> {
    attempt: &'a tips::PublicAttempt,
    token: Uuid,
    amount: String,
    heading: &'static str,
    message: &'static str,
}

fn render(template: &impl Template) -> Result<Html<String>, ApiError> {
    template
        .render()
        .map(Html)
        .map_err(|_| ApiError::unavailable())
}
fn money(minor: i64) -> String {
    format!("{},{:02} ₽", minor / 100, minor % 100)
}

async fn profile_page(
    State(state): State<AppState>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Html<String>, ApiError> {
    enabled(&state)?;
    let Path(slug) = path.map_err(|_| ApiError::bad_request())?;
    let profile = bounded(tips::public_recipient(&state.pool, slug))
        .await?
        .ok_or_else(not_found)?;
    render(&ProfileTemplate {
        profile: &profile,
        key: Uuid::new_v4(),
    })
}
async fn checkout_page(
    State(state): State<AppState>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Html<String>, ApiError> {
    enabled(&state)?;
    let Path(token) = path.map_err(|_| ApiError::bad_request())?;
    let checkout = bounded(tips::checkout(&state.pool, token))
        .await?
        .ok_or_else(not_found)?;
    render(&CheckoutTemplate {
        amount: money(checkout.amount_minor),
        pending: checkout.status == "awaiting_payment",
        checkout: &checkout,
        token,
    })
}
async fn status_page(
    State(state): State<AppState>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Html<String>, ApiError> {
    enabled(&state)?;
    let Path(token) = path.map_err(|_| ApiError::bad_request())?;
    let attempt = bounded(tips::public_attempt(&state.pool, token))
        .await?
        .ok_or_else(not_found)?;
    let (heading, message) = match attempt.status.as_str() {
        "succeeded" => (
            "Спасибо за добрые слова и чаевые!",
            "Тестовая оплата подтверждена сервером. Деньги не списывались.",
        ),
        "failed" => (
            "Тестовая оплата отклонена",
            "Эмулятор вернул отказ. Деньги не списывались.",
        ),
        "expired" => (
            "Время тестовой оплаты истекло",
            "Откройте ссылку получателя, чтобы начать заново. Деньги не списывались.",
        ),
        _ => (
            "Проверяем тестовую оплату",
            "Ждём серверное подтверждение от эмулятора. Возврат на эту страницу сам по себе не подтверждает оплату.",
        ),
    };
    render(&StatusTemplate {
        amount: money(attempt.amount_minor),
        attempt: &attempt,
        token,
        heading,
        message,
    })
}
