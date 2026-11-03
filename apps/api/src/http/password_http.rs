use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, Path, State,
        rejection::{JsonRejection, PathRejection},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::{future::Future, time::Duration};
use utoipa::ToSchema;
use zeroize::Zeroizing;

use super::{
    AppState,
    api::{ApiError, SignedIn, append_cookie, check_mutation, check_origin, cookie_token},
};
use crate::{
    auth::{
        login::Email,
        password::{self, PasswordError},
        store,
        token::SecretToken,
    },
    core::{Name, Role, TenantId, TenantSession, UserId},
};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/assets/password-login.js",
            get(|| async {
                (
                    [(
                        axum::http::header::CONTENT_TYPE,
                        "text/javascript; charset=utf-8",
                    )],
                    include_str!("../../web/password-login.js"),
                )
            }),
        )
        .route(
            "/assets/password-login.css",
            get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../../web/password-login.css"),
                )
            }),
        )
        .route("/auth/password/login", post(login))
        .route("/auth/password/setup", post(setup))
        .route("/auth/password", get(own).post(change))
        .route(
            "/api/organizations/{tenant_id}/team/password-accounts",
            post(create_account),
        )
        .layer(DefaultBodyLimit::max(8192))
}

impl From<PasswordError> for ApiError {
    fn from(error: PasswordError) -> Self {
        match error {
            PasswordError::Invalid => Self::new(
                StatusCode::BAD_REQUEST,
                "invalid_password",
                "Use a passphrase of 15..128 characters without controls",
            ),
            PasswordError::Busy => rate_limited(),
            PasswordError::Unavailable => Self::unavailable(),
        }
    }
}
fn rate_limited() -> ApiError {
    ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "password_rate_limited",
        "Please wait before trying again",
    )
}
fn invalid_login() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "invalid_credentials",
        "Email or password is incorrect",
    )
}
async fn db<T>(future: impl Future<Output = Result<T, sqlx::Error>>) -> Result<T, ApiError> {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .map_err(|_| ApiError::unavailable())?
        .map_err(Into::into)
}
async fn budget(state: &AppState, email: &Email, scope: &str) -> Result<(), ApiError> {
    if !db(password::budget(&state.pool, email, scope)).await? {
        return Err(rate_limited());
    }
    Ok(())
}
fn signed_response(
    state: &AppState,
    user: UserId,
    session: &SecretToken,
    csrf: &SecretToken,
) -> Response {
    let mut response = Json(PasswordLoginResponse::Authenticated { user_id: user }).into_response();
    append_cookie(
        &mut response,
        state.auth.session_cookie_name(),
        &session.expose(),
        &state.auth,
        true,
        state.auth.session_ttl_seconds,
    );
    append_cookie(
        &mut response,
        state.auth.csrf_cookie_name(),
        &csrf.expose(),
        &state.auth,
        false,
        state.auth.session_ttl_seconds,
    );
    response
}
fn clear_cookies(state: &AppState, response: &mut Response) {
    append_cookie(
        response,
        state.auth.session_cookie_name(),
        "",
        &state.auth,
        true,
        0,
    );
    append_cookie(
        response,
        state.auth.csrf_cookie_name(),
        "",
        &state.auth,
        false,
        0,
    );
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct PasswordLoginRequest {
    email: String,
    #[schema(format=Password)]
    password: String,
}
#[derive(Serialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(super) enum PasswordLoginResponse {
    Authenticated { user_id: UserId },
    PasswordChangeRequired { setup_token: String },
}

#[utoipa::path(post,path="/auth/password/login",tag="auth",request_body=PasswordLoginRequest,responses((status=200,body=PasswordLoginResponse),(status=401,description="Invalid credentials"),(status=429,description="Try later")))]
pub(super) async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<PasswordLoginRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    check_origin(&headers, &state.auth)?;
    let _gate = state.login_gate.try_acquire().map_err(|_| rate_limited())?;
    let Json(body) = body.map_err(|_| ApiError::bad_request())?;
    let secret = Zeroizing::new(body.password);
    let email = Email::parse(&body.email).map_err(|_| invalid_login())?;
    budget(&state, &email, "login").await?;
    let credential = db(password::lookup(&state.pool, &email)).await?;
    let hash = credential.as_ref().map(|c| c.password_hash.clone());
    if !state.password_engine.verify(secret, hash).await? {
        return Err(invalid_login());
    }
    let credential = credential.ok_or_else(invalid_login)?;
    let previous = cookie_token(&headers, state.auth.session_cookie_name())?.map(|s| s.hash());
    if credential.must_change {
        let grant = SecretToken::generate().map_err(|_| ApiError::unavailable())?;
        let ready: bool = db(
            sqlx::query_scalar("SELECT auth.password_setup_begin($1,$2,$3)")
                .bind(credential.identity_id)
                .bind(credential.version)
                .bind(password::setup_digest(&grant).as_slice())
                .fetch_one(&state.pool),
        )
        .await?;
        if !ready {
            return Err(invalid_login());
        }
        if let Some(previous) = previous {
            db(store::revoke_session(&state.pool, &previous)).await?;
        }
        let mut response = Json(PasswordLoginResponse::PasswordChangeRequired {
            setup_token: grant.expose(),
        })
        .into_response();
        clear_cookies(&state, &mut response);
        return Ok(response);
    }
    let session = SecretToken::generate().map_err(|_| ApiError::unavailable())?;
    let csrf = SecretToken::generate().map_err(|_| ApiError::unavailable())?;
    let user: Option<UserId> = db(sqlx::query_scalar(
        "SELECT auth.password_finish_login($1,$2,$3,$4,$5,$6)",
    )
    .bind(credential.identity_id)
    .bind(credential.version)
    .bind(session.hash().as_slice())
    .bind(csrf.hash().as_slice())
    .bind(state.auth.session_ttl_seconds)
    .bind(previous.as_ref().map(|h| h.as_slice()))
    .fetch_one(&state.pool))
    .await?;
    Ok(signed_response(
        &state,
        user.ok_or_else(invalid_login)?,
        &session,
        &csrf,
    ))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct PasswordSetupRequest {
    setup_token: String,
    #[schema(format=Password)]
    new_password: String,
}
#[utoipa::path(post,path="/auth/password/setup",tag="auth",request_body=PasswordSetupRequest,responses((status=200,body=PasswordLoginResponse),(status=401,description="Setup expired or consumed")))]
pub(super) async fn setup(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<PasswordSetupRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    check_origin(&headers, &state.auth)?;
    let _gate = state.login_gate.try_acquire().map_err(|_| rate_limited())?;
    let Json(body) = body.map_err(|_| ApiError::bad_request())?;
    let secret = Zeroizing::new(body.new_password);
    password::validate(&secret)?;
    let token = SecretToken::parse(&body.setup_token).map_err(|_| invalid_login())?;
    let grant = password::setup_digest(&token);
    let snapshot: Option<password::SetupCredential> = db(sqlx::query_as(
        "SELECT * FROM auth.password_setup_lookup($1)",
    )
    .bind(grant.as_slice())
    .fetch_optional(&state.pool))
    .await?;
    let snapshot = snapshot.ok_or_else(invalid_login)?;
    let email = Email::parse(&snapshot.email).map_err(|_| ApiError::unavailable())?;
    budget(&state, &email, "setup").await?;
    if state
        .password_engine
        .verify(secret.clone(), snapshot.password_hash)
        .await?
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "password_unchanged",
            "Choose a different password",
        ));
    }
    let hash = state.password_engine.hash(secret).await?;
    let session = SecretToken::generate().map_err(|_| ApiError::unavailable())?;
    let csrf = SecretToken::generate().map_err(|_| ApiError::unavailable())?;
    let previous = cookie_token(&headers, state.auth.session_cookie_name())?.map(|s| s.hash());
    let user: Option<UserId> = db(sqlx::query_scalar(
        "SELECT auth.password_setup_finish($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(grant.as_slice())
    .bind(snapshot.version)
    .bind(hash.as_str())
    .bind(session.hash().as_slice())
    .bind(csrf.hash().as_slice())
    .bind(state.auth.session_ttl_seconds)
    .bind(previous.as_ref().map(|h| h.as_slice()))
    .fetch_one(&state.pool))
    .await?;
    Ok(signed_response(
        &state,
        user.ok_or_else(invalid_login)?,
        &session,
        &csrf,
    ))
}

#[derive(Serialize, ToSchema)]
pub(super) struct PasswordStatus {
    email: String,
    has_password: bool,
    requires_current_password: bool,
}
#[utoipa::path(get,path="/auth/password",tag="auth",security(("sessionCookie"=[])),responses((status=200,body=PasswordStatus)))]
pub(super) async fn own(
    State(state): State<AppState>,
    signed: SignedIn,
) -> Result<Json<PasswordStatus>, ApiError> {
    let own = db(password::own(&state.pool, &signed.token_hash))
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    Ok(Json(PasswordStatus {
        email: own.email,
        has_password: own.password_enabled,
        requires_current_password: own.requires_current_password,
    }))
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct PasswordChangeRequest {
    #[schema(format=Password)]
    current_password: Option<String>,
    #[schema(format=Password)]
    new_password: String,
}
#[utoipa::path(post,path="/auth/password",tag="auth",security(("sessionCookie"=[])),request_body=PasswordChangeRequest,responses((status=204,description="Password changed; sign in again"),(status=401,description="Session or current password invalid")))]
pub(super) async fn change(
    State(state): State<AppState>,
    signed: SignedIn,
    headers: HeaderMap,
    body: Result<Json<PasswordChangeRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    check_mutation(&headers, &state.auth, &signed)?;
    let Json(body) = body.map_err(|_| ApiError::bad_request())?;
    let secret = Zeroizing::new(body.new_password);
    password::validate(&secret)?;
    let current = Zeroizing::new(body.current_password.unwrap_or_default());
    let own = db(password::own(&state.pool, &signed.token_hash))
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    budget(
        &state,
        &Email::parse(&own.email).map_err(|_| ApiError::unavailable())?,
        "change",
    )
    .await?;
    if own.requires_current_password
        && !state
            .password_engine
            .verify(current, own.password_hash.clone())
            .await?
    {
        return Err(invalid_login());
    }
    if own.password_hash.is_some()
        && state
            .password_engine
            .verify(secret.clone(), own.password_hash)
            .await?
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "password_unchanged",
            "Choose a different password",
        ));
    }
    let hash = state.password_engine.hash(secret).await?;
    let changed: bool = db(sqlx::query_scalar("SELECT auth.password_change($1,$2,$3)")
        .bind(signed.token_hash.as_slice())
        .bind(own.version)
        .bind(hash.as_str())
        .fetch_one(&state.pool))
    .await?;
    if !changed {
        return Err(ApiError::unauthorized());
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    clear_cookies(&state, &mut response);
    Ok(response)
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct PasswordAccountRequest {
    display_name: String,
    email: String,
    role: Role,
    #[schema(format=Password)]
    temporary_password: String,
}
#[derive(Serialize, ToSchema)]
pub(super) struct PasswordAccountResponse {
    user_id: UserId,
}
#[utoipa::path(post,path="/api/organizations/{tenant_id}/team/password-accounts",tag="team",security(("sessionCookie"=[])),params(("tenant_id"=uuid::Uuid,Path)),request_body=PasswordAccountRequest,responses((status=201,body=PasswordAccountResponse),(status=409,description="Existing global account; password untouched")))]
pub(super) async fn create_account(
    State(state): State<AppState>,
    signed: SignedIn,
    headers: HeaderMap,
    path: Result<Path<TenantId>, PathRejection>,
    body: Result<Json<PasswordAccountRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<PasswordAccountResponse>), ApiError> {
    check_mutation(&headers, &state.auth, &signed)?;
    let Path(tenant) = path.map_err(|_| ApiError::bad_request())?;
    let Json(body) = body.map_err(|_| ApiError::bad_request())?;
    let secret = Zeroizing::new(body.temporary_password);
    let email = Email::parse(&body.email).map_err(|_| ApiError::bad_request())?;
    let name = Name::parse(&body.display_name)?;
    if body.role == Role::Owner {
        return Err(ApiError::forbidden());
    }
    let access = TenantSession::begin(&state.pool, signed.user_id, tenant).await?;
    if access.role() != Role::Owner {
        return Err(ApiError::forbidden());
    }
    access.commit().await?;
    let hash = state.password_engine.hash(secret).await?;
    let mut transaction = TenantSession::begin(&state.pool, signed.user_id, tenant).await?;
    let user: UserId = sqlx::query_scalar("SELECT core.team_password_account($1,$2,$3,$4,$5)")
        .bind(tenant)
        .bind(email.as_str())
        .bind(name.as_str())
        .bind(body.role)
        .bind(hash.as_str())
        .fetch_one(transaction.connection())
        .await
        .map_err(
            |e| match e.as_database_error().and_then(|e| e.code()).as_deref() {
                Some("P2302" | "23505") => ApiError::new(
                    StatusCode::CONFLICT,
                    "email_in_use",
                    "Account already exists; password was not changed",
                ),
                Some("P2303") => rate_limited(),
                Some("M2201") => ApiError::forbidden(),
                _ => e.into(),
            },
        )?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(PasswordAccountResponse { user_id: user }),
    ))
}
