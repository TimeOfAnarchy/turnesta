use axum::{
    Json, Router,
    extract::{
        FromRequestParts, MatchedPath, Path, Query, RawPathParams, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{CONTENT_TYPE, COOKIE, SET_COOKIE},
        request::Parts,
    },
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use utoipa::ToSchema;

use super::{AppState, ErrorResponse};
use crate::{
    auth::{AuthConfig, login as login_store, store, token::SecretToken},
    core::{
        CoreError, Location, LocationId, Name, Organization, Role, TenantId, TenantSession, UserId,
    },
};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/login",
            get(|| async { Html(include_str!("../../web/login.html")) }),
        )
        .route(
            "/assets/login.js",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../web/login.js"),
                )
            }),
        )
        .route(
            "/assets/login.css",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../../web/login.css"),
                )
            }),
        )
        .route("/auth/login", post(login))
        .route("/auth/session", get(session))
        .route("/auth/logout", post(logout))
        .route("/api/organizations", get(organizations))
        .route(
            "/api/organizations/{tenant_id}",
            get(organization).patch(rename_organization),
        )
        .route(
            "/api/organizations/{tenant_id}/locations",
            get(locations).post(create_location),
        )
        .route(
            "/api/organizations/{tenant_id}/locations/{location_id}",
            axum::routing::patch(update_location),
        )
        .route("/api/time-zones", get(time_zones))
}

#[derive(Debug)]
pub(super) struct ApiError(StatusCode, &'static str, &'static str);

impl ApiError {
    pub(super) fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self(status, code, message)
    }
    pub(super) fn unauthorized() -> Self {
        Self(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "A valid session is required",
        )
    }
    pub(super) fn forbidden() -> Self {
        Self(StatusCode::FORBIDDEN, "forbidden", "Request not permitted")
    }
    pub(super) fn bad_request() -> Self {
        Self(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Invalid request",
        )
    }
    pub(super) fn unavailable() -> Self {
        Self(
            StatusCode::SERVICE_UNAVAILABLE,
            "service_unavailable",
            "Service temporarily unavailable",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(ErrorResponse {
                code: self.1,
                message: self.2,
            }),
        )
            .into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        tracing::error!(sqlstate = ?error.as_database_error().and_then(|e| e.code()), "API database operation failed");
        Self::unavailable()
    }
}

impl From<CoreError> for ApiError {
    fn from(error: CoreError) -> Self {
        match error {
            CoreError::AccessDenied | CoreError::ReferenceNotFound => {
                Self(StatusCode::NOT_FOUND, "not_found", "Organization not found")
            }
            CoreError::Forbidden => Self::forbidden(),
            CoreError::InvalidName => Self(
                StatusCode::BAD_REQUEST,
                "invalid_name",
                "Name must contain 1..120 characters and no controls",
            ),
            CoreError::InvalidTimeZone => Self(
                StatusCode::BAD_REQUEST,
                "invalid_time_zone",
                "Unknown IANA time zone",
            ),
            CoreError::Database(error) => error.into(),
            CoreError::VersionConflict => Self(
                StatusCode::CONFLICT,
                "version_conflict",
                "Record has changed; reload before saving again",
            ),
            CoreError::InvalidVersion => Self::bad_request(),
            _ => Self::unavailable(),
        }
    }
}

pub(super) struct SignedIn {
    pub(super) user_id: UserId,
    pub(super) token_hash: [u8; 32],
    csrf_hash: [u8; 32],
}

impl FromRequestParts<AppState> for SignedIn {
    type Rejection = ApiError;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = cookie_token(&parts.headers, state.auth.session_cookie_name())?
            .ok_or_else(ApiError::unauthorized)?;
        let token_hash = token.hash();
        // Use router captures, including percent decoding, rather than the raw URI.
        // Authentication and membership must share a snapshot before reading a body.
        let tenant = if parts
            .extensions
            .get::<MatchedPath>()
            .is_some_and(|path| path.as_str().contains("{tenant_id}"))
        {
            let params = RawPathParams::from_request_parts(parts, state)
                .await
                .map_err(|_| ApiError::bad_request())?;
            let value = params
                .iter()
                .find(|(name, _)| *name == "tenant_id")
                .map(|(_, value)| value)
                .ok_or_else(ApiError::bad_request)?;
            Some(TenantId::from_uuid(
                uuid::Uuid::parse_str(value).map_err(|_| ApiError::bad_request())?,
            ))
        } else {
            None
        };
        let record = store::find_scoped_session(&state.pool, &token_hash, tenant)
            .await?
            .ok_or_else(ApiError::unauthorized)?;
        if !record.tenant_member {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "Organization not found",
            ));
        }
        Ok(Self {
            user_id: record.user_id,
            token_hash,
            csrf_hash: record
                .csrf_hash
                .try_into()
                .map_err(|_| ApiError::unavailable())?,
        })
    }
}

pub(super) fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, ApiError> {
    let mut values = headers.get_all(name).iter();
    let value = values.next().ok_or_else(ApiError::forbidden)?;
    if values.next().is_some() {
        return Err(ApiError::forbidden());
    }
    value.to_str().map_err(|_| ApiError::forbidden())
}

pub(super) fn check_origin(headers: &HeaderMap, config: &AuthConfig) -> Result<(), ApiError> {
    if single_header(headers, "origin")? != config.public_origin {
        return Err(ApiError::forbidden());
    }
    Ok(())
}

pub(super) fn check_mutation(
    headers: &HeaderMap,
    config: &AuthConfig,
    signed_in: &SignedIn,
) -> Result<(), ApiError> {
    check_origin(headers, config)?;
    let supplied = SecretToken::parse(single_header(headers, "x-csrf-token")?)
        .map_err(|_| ApiError::forbidden())?;
    if !bool::from(supplied.hash().ct_eq(&signed_in.csrf_hash)) {
        return Err(ApiError::forbidden());
    }
    Ok(())
}

pub(super) fn cookie_token(
    headers: &HeaderMap,
    name: &str,
) -> Result<Option<SecretToken>, ApiError> {
    let mut found = None;
    for header in headers.get_all(COOKIE) {
        for pair in header
            .to_str()
            .map_err(|_| ApiError::unauthorized())?
            .split(';')
        {
            if let Some((key, value)) = pair.trim().split_once('=')
                && key == name
            {
                if found.is_some() {
                    return Err(ApiError::unauthorized());
                }
                found = Some(SecretToken::parse(value).map_err(|_| ApiError::unauthorized())?);
            }
        }
    }
    Ok(found)
}

pub(super) fn append_cookie(
    response: &mut Response,
    name: &str,
    value: &str,
    config: &AuthConfig,
    http_only: bool,
    max_age: i32,
) {
    let cookie = format!(
        "{name}={value}; Path=/; Max-Age={max_age}; SameSite=Lax{}{}",
        if http_only { "; HttpOnly" } else { "" },
        if config.secure_cookies {
            "; Secure"
        } else {
            ""
        }
    );
    response.headers_mut().append(
        SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("validated token and constant cookie attributes"),
    );
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct LoginRequest {
    #[schema(min_length = 64, max_length = 64)]
    code: String,
}

#[derive(Serialize, ToSchema)]
pub(super) struct SessionResponse {
    user_id: UserId,
}

#[utoipa::path(post, path="/auth/login", tag="auth", request_body=LoginRequest,
    params(("Origin" = String, Header, description="Configured public origin")),
    responses((status=200, body=SessionResponse), (status=401, body=ErrorResponse), (status=403, body=ErrorResponse), (status=429, body=ErrorResponse)))]
pub(super) async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<LoginRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    check_origin(&headers, &state.auth)?;
    let _permit = state.login_gate.try_acquire().map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "busy",
            "Please retry shortly",
        )
    })?;
    let Json(body) = body.map_err(|_| ApiError::bad_request())?;
    let code = SecretToken::parse(&body.code).map_err(|_| ApiError::unauthorized())?;
    let previous = cookie_token(&headers, state.auth.session_cookie_name())?;
    let previous_hash = previous.map(|token| token.hash());
    let session = SecretToken::generate().map_err(|_| ApiError::unavailable())?;
    let csrf = SecretToken::generate().map_err(|_| ApiError::unavailable())?;
    let user_id = login_store::consume_login_code(
        &state.pool,
        &code.hash(),
        &session.hash(),
        &csrf.hash(),
        state.auth.session_ttl_seconds,
        previous_hash.as_ref(),
    )
    .await?
    .ok_or_else(ApiError::unauthorized)?;
    let mut response = Json(SessionResponse { user_id }).into_response();
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
    Ok(response)
}

#[utoipa::path(get, path="/auth/session", tag="auth", security(("sessionCookie"=[])), responses((status=200, body=SessionResponse), (status=401, body=ErrorResponse)))]
pub(super) async fn session(signed_in: SignedIn) -> Json<SessionResponse> {
    Json(SessionResponse {
        user_id: signed_in.user_id,
    })
}

#[utoipa::path(post, path="/auth/logout", tag="auth", security(("sessionCookie"=[])),
    params(("Origin"=String, Header), ("X-CSRF-Token"=String, Header)), responses((status=204), (status=401, body=ErrorResponse), (status=403, body=ErrorResponse)))]
pub(super) async fn logout(
    signed_in: SignedIn,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    check_mutation(&headers, &state.auth, &signed_in)?;
    store::revoke_session(&state.pool, &signed_in.token_hash).await?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    append_cookie(
        &mut response,
        state.auth.session_cookie_name(),
        "",
        &state.auth,
        true,
        0,
    );
    append_cookie(
        &mut response,
        state.auth.csrf_cookie_name(),
        "",
        &state.auth,
        false,
        0,
    );
    Ok(response)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PageQuery {
    after: Option<uuid::Uuid>,
    #[serde(default = "default_limit")]
    limit: i32,
}
fn default_limit() -> i32 {
    50
}
impl PageQuery {
    fn parse(query: Result<Query<Self>, QueryRejection>) -> Result<Self, ApiError> {
        let Query(value) = query.map_err(|_| ApiError::bad_request())?;
        if !(1..=100).contains(&value.limit) {
            return Err(ApiError::bad_request());
        }
        Ok(value)
    }
}

#[derive(Serialize, sqlx::FromRow, ToSchema)]
pub(super) struct OrganizationSummary {
    id: TenantId,
    name: String,
    role: Role,
}
#[derive(Serialize, ToSchema)]
pub(super) struct OrganizationList {
    items: Vec<OrganizationSummary>,
    next_cursor: Option<TenantId>,
}
#[derive(Serialize, ToSchema)]
pub(super) struct LocationList {
    items: Vec<Location>,
    next_cursor: Option<LocationId>,
}

#[utoipa::path(get, path="/api/organizations", tag="core", security(("sessionCookie"=[])),
    params(("after"=Option<String>, Query), ("limit"=Option<i32>, Query)), responses((status=200, body=OrganizationList), (status=401, body=ErrorResponse)))]
pub(super) async fn organizations(
    signed_in: SignedIn,
    State(state): State<AppState>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Json<OrganizationList>, ApiError> {
    let page = PageQuery::parse(query)?;
    let mut items: Vec<OrganizationSummary> =
        sqlx::query_as("SELECT id, name, role FROM core.list_user_organizations($1, $2, $3)")
            .bind(signed_in.user_id)
            .bind(page.after)
            .bind(page.limit)
            .fetch_all(&state.pool)
            .await?;
    let more = items.len() > page.limit as usize;
    items.truncate(page.limit as usize);
    let next_cursor = if more {
        items.last().map(|item| item.id)
    } else {
        None
    };
    Ok(Json(OrganizationList { items, next_cursor }))
}

#[utoipa::path(get, path="/api/organizations/{tenant_id}", tag="core", security(("sessionCookie"=[])),
    params(("tenant_id"=String, Path)), responses((status=200, body=Organization), (status=401, body=ErrorResponse), (status=404, body=ErrorResponse)))]
pub(super) async fn organization(
    signed_in: SignedIn,
    State(state): State<AppState>,
    path: Result<Path<TenantId>, PathRejection>,
) -> Result<Json<Organization>, ApiError> {
    let Path(tenant) = path.map_err(|_| ApiError::bad_request())?;
    let mut session = TenantSession::begin(&state.pool, signed_in.user_id, tenant).await?;
    let result = session.organization().await?;
    session.commit().await?;
    Ok(Json(result))
}

#[utoipa::path(get, path="/api/organizations/{tenant_id}/locations", tag="core", security(("sessionCookie"=[])),
    params(("tenant_id"=String, Path), ("after"=Option<String>, Query), ("limit"=Option<i32>, Query)), responses((status=200, body=LocationList), (status=401, body=ErrorResponse), (status=404, body=ErrorResponse)))]
pub(super) async fn locations(
    signed_in: SignedIn,
    State(state): State<AppState>,
    path: Result<Path<TenantId>, PathRejection>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Json<LocationList>, ApiError> {
    let Path(tenant) = path.map_err(|_| ApiError::bad_request())?;
    let page = PageQuery::parse(query)?;
    let after = page.after.map(LocationId::from_uuid);
    let mut session = TenantSession::begin(&state.pool, signed_in.user_id, tenant).await?;
    let mut items = session.locations_page(after, i64::from(page.limit)).await?;
    session.commit().await?;
    let more = items.len() > page.limit as usize;
    items.truncate(page.limit as usize);
    let next_cursor = if more {
        items.last().map(|item| item.id)
    } else {
        None
    };
    Ok(Json(LocationList { items, next_cursor }))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateLocationRequest {
    name: String,
    time_zone: String,
}

#[utoipa::path(post, path="/api/organizations/{tenant_id}/locations", tag="core", security(("sessionCookie"=[])), request_body=CreateLocationRequest,
    params(("tenant_id"=String, Path), ("Origin"=String, Header), ("X-CSRF-Token"=String, Header)),
    responses((status=201, body=Location), (status=400, body=ErrorResponse), (status=401, body=ErrorResponse), (status=403, body=ErrorResponse), (status=404, body=ErrorResponse)))]
pub(super) async fn create_location(
    signed_in: SignedIn,
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<TenantId>, PathRejection>,
    body: Result<Json<CreateLocationRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Location>), ApiError> {
    check_mutation(&headers, &state.auth, &signed_in)?;
    let Path(tenant) = path.map_err(|_| ApiError::bad_request())?;
    let Json(body) = body.map_err(|_| ApiError::bad_request())?;
    let mut session = TenantSession::begin(&state.pool, signed_in.user_id, tenant).await?;
    let location = session
        .create_location(&Name::parse(&body.name)?, &body.time_zone)
        .await?;
    session.commit().await?;
    Ok((StatusCode::CREATED, Json(location)))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct RenameOrganizationRequest {
    name: String,
    #[schema(minimum = 1)]
    expected_version: i32,
}

#[utoipa::path(patch, path="/api/organizations/{tenant_id}", tag="core", security(("sessionCookie"=[])), request_body=RenameOrganizationRequest,
    params(("tenant_id"=String, Path), ("Origin"=String, Header), ("X-CSRF-Token"=String, Header)),
    responses((status=200, body=Organization), (status=400, body=ErrorResponse), (status=401, body=ErrorResponse), (status=403, body=ErrorResponse), (status=404, body=ErrorResponse), (status=409, body=ErrorResponse)))]
pub(super) async fn rename_organization(
    signed_in: SignedIn,
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<TenantId>, PathRejection>,
    body: Result<Json<RenameOrganizationRequest>, JsonRejection>,
) -> Result<Json<Organization>, ApiError> {
    check_mutation(&headers, &state.auth, &signed_in)?;
    let Path(tenant) = path.map_err(|_| ApiError::bad_request())?;
    let Json(body) = body.map_err(|_| ApiError::bad_request())?;
    let mut session = TenantSession::begin(&state.pool, signed_in.user_id, tenant).await?;
    let organization = session
        .rename_organization(&Name::parse(&body.name)?, body.expected_version)
        .await?;
    session.commit().await?;
    Ok(Json(organization))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct UpdateLocationRequest {
    name: String,
    time_zone: String,
    #[schema(minimum = 1)]
    expected_version: i32,
}

#[utoipa::path(patch, path="/api/organizations/{tenant_id}/locations/{location_id}", tag="core", security(("sessionCookie"=[])), request_body=UpdateLocationRequest,
    params(("tenant_id"=String, Path), ("location_id"=String, Path), ("Origin"=String, Header), ("X-CSRF-Token"=String, Header)),
    responses((status=200, body=Location), (status=400, body=ErrorResponse), (status=401, body=ErrorResponse), (status=403, body=ErrorResponse), (status=404, body=ErrorResponse), (status=409, body=ErrorResponse)))]
pub(super) async fn update_location(
    signed_in: SignedIn,
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<(TenantId, LocationId)>, PathRejection>,
    body: Result<Json<UpdateLocationRequest>, JsonRejection>,
) -> Result<Json<Location>, ApiError> {
    check_mutation(&headers, &state.auth, &signed_in)?;
    let Path((tenant, location)) = path.map_err(|_| ApiError::bad_request())?;
    let Json(body) = body.map_err(|_| ApiError::bad_request())?;
    let mut session = TenantSession::begin(&state.pool, signed_in.user_id, tenant).await?;
    let location = session
        .update_location(
            location,
            &Name::parse(&body.name)?,
            &body.time_zone,
            body.expected_version,
        )
        .await?;
    session.commit().await?;
    Ok(Json(location))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TimeZoneQuery {
    #[serde(default)]
    query: String,
    #[serde(default = "zone_limit")]
    limit: i32,
}
fn zone_limit() -> i32 {
    20
}

#[derive(Serialize, ToSchema)]
pub(super) struct TimeZoneList {
    items: Vec<String>,
}

#[utoipa::path(get, path="/api/time-zones", tag="core", security(("sessionCookie"=[])),
    params(("query"=Option<String>, Query), ("limit"=Option<i32>, Query)),
    responses((status=200, body=TimeZoneList), (status=400, body=ErrorResponse), (status=401, body=ErrorResponse)))]
pub(super) async fn time_zones(
    _signed_in: SignedIn,
    State(state): State<AppState>,
    query: Result<Query<TimeZoneQuery>, QueryRejection>,
) -> Result<Json<TimeZoneList>, ApiError> {
    let Query(query) = query.map_err(|_| ApiError::bad_request())?;
    if query.query.chars().count() > 100 || !(1..=50).contains(&query.limit) {
        return Err(ApiError::bad_request());
    }
    let items = sqlx::query_scalar("SELECT name FROM core.time_zones WHERE strpos(lower(name), lower($1)) > 0 ORDER BY name LIMIT $2")
        .bind(query.query.trim()).bind(query.limit).fetch_all(&state.pool).await?;
    Ok(Json(TimeZoneList { items }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_session_cookies_are_rejected_and_other_cookies_are_ignored() {
        let mut headers = HeaderMap::new();
        headers.insert(COOKIE, HeaderValue::from_static("other=anything"));
        assert!(cookie_token(&headers, "session").unwrap().is_none());
        let token = "a".repeat(64);
        headers.insert(
            COOKIE,
            HeaderValue::from_str(&format!("session={token}; session={token}")).unwrap(),
        );
        assert!(cookie_token(&headers, "session").is_err());
    }
}
