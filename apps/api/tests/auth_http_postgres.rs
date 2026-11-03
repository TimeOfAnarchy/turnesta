use std::time::Duration;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header::SET_COOKIE},
    response::Response,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use turnesta_api::{
    auth::{
        AuthConfig,
        login::{self, Email},
    },
    core::{Name, Provisioner, Role, TenantId, UserId},
    http::{AppState, router},
};

const ORIGIN: &str = "http://127.0.0.1:8080";

struct Fixture {
    runtime: PgPool,
    admin: PgPool,
    migrator: PgPool,
    app: Router,
}
struct Identity {
    tenant: TenantId,
    user: UserId,
    email: Email,
}
struct Credentials {
    cookie: String,
    csrf: String,
    session_hash: [u8; 32],
}

async fn pool(variable: &str) -> PgPool {
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .acquire_timeout(Duration::from_secs(3))
        .connect(&std::env::var(variable).expect("test database URL is required"))
        .await
        .unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        database.ends_with("_test"),
        "requires a dedicated *_test database"
    );
    pool
}

impl Fixture {
    async fn new() -> Self {
        let runtime = pool("TEST_DATABASE_URL").await;
        let admin = pool("TEST_ADMIN_DATABASE_URL").await;
        let migrator = pool("TEST_MIGRATION_DATABASE_URL").await;
        let app = router(AppState::new(runtime.clone(), Duration::from_secs(1)));
        Self {
            runtime,
            admin,
            migrator,
            app,
        }
    }

    async fn identity(&self, role: Role) -> Identity {
        let provisioner = Provisioner::new(self.admin.clone());
        let org = provisioner
            .create_organization(
                &Name::parse("HTTP auth test").unwrap(),
                &Name::parse("Owner").unwrap(),
            )
            .await
            .unwrap();
        let user = if role == Role::Owner {
            org.owner.id
        } else {
            let user = provisioner
                .create_user(&Name::parse("HTTP test member").unwrap())
                .await
                .unwrap();
            provisioner
                .add_membership(org.organization.id, user.id, role)
                .await
                .unwrap();
            user.id
        };
        let email = Email::parse(&format!("http-{}@example.test", uuid::Uuid::new_v4())).unwrap();
        login::bind_email(&self.admin, user, &email).await.unwrap();
        Identity {
            tenant: org.organization.id,
            user,
            email,
        }
    }

    async fn code(&self, identity: &Identity) -> String {
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes).unwrap();
        let hash: [u8; 32] = Sha256::digest(bytes).into();
        login::issue_login_code(&self.admin, &identity.email, &hash, 600)
            .await
            .unwrap();
        hex::encode(bytes)
    }

    async fn login(&self, identity: &Identity) -> Credentials {
        let code = self.code(identity).await;
        let response = self
            .app
            .clone()
            .oneshot(
                Request::post("/auth/login")
                    .header("origin", ORIGIN)
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"code":code}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let credentials = credentials(&response);
        assert_eq!(body(response).await["user_id"], identity.user.to_string());
        credentials
    }

    async fn close(self) {
        self.runtime.close().await;
        self.admin.close().await;
        self.migrator.close().await;
    }
}

fn credentials(response: &Response) -> Credentials {
    let cookies: Vec<&str> = response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap())
        .collect();
    let session = cookies
        .iter()
        .find(|v| v.starts_with("turnesta_session=") || v.starts_with("__Host-turnesta_session="))
        .unwrap();
    let csrf = cookies
        .iter()
        .find(|v| v.starts_with("turnesta_csrf=") || v.starts_with("__Host-turnesta_csrf="))
        .unwrap();
    assert!(session.contains("HttpOnly"));
    assert!(!csrf.contains("HttpOnly"));
    assert!(session.contains("SameSite=Lax") && session.contains("Path=/"));
    assert!(!session.contains("Domain="));
    let session = session.split(';').next().unwrap();
    let csrf_pair = csrf.split(';').next().unwrap();
    let raw = hex::decode(session.split_once('=').unwrap().1).unwrap();
    Credentials {
        cookie: format!("{session}; {csrf_pair}"),
        csrf: csrf_pair.split_once('=').unwrap().1.to_owned(),
        session_hash: Sha256::digest(raw).into(),
    }
}

async fn body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap()
}

async fn get(app: &Router, path: &str, credentials: &Credentials) -> Response {
    app.clone()
        .oneshot(
            Request::get(path)
                .header("cookie", &credentials.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn create_location(
    fixture: &Fixture,
    identity: &Identity,
    credentials: &Credentials,
    origin: &str,
    csrf: Option<&str>,
    value: Value,
) -> Response {
    let mut request = Request::post(format!("/api/organizations/{}/locations", identity.tenant))
        .header("cookie", &credentials.cookie)
        .header("origin", origin)
        .header("content-type", "application/json");
    if let Some(csrf) = csrf {
        request = request.header("x-csrf-token", csrf);
    }
    fixture
        .app
        .clone()
        .oneshot(request.body(Body::from(value.to_string())).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database and runtime/admin/migrator URLs"]
async fn login_once_session_cookie_csrf_and_logout_form_a_complete_flow() {
    let fixture = Fixture::new().await;
    let identity = fixture.identity(Role::Owner).await;
    let code = fixture.code(&identity).await;
    let request = || {
        Request::post("/auth/login")
            .header("origin", ORIGIN)
            .header("content-type", "application/json")
            .body(Body::from(json!({"code":code}).to_string()))
            .unwrap()
    };
    let response = fixture.app.clone().oneshot(request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let credentials = credentials(&response);
    let replay = fixture.app.clone().oneshot(request()).await.unwrap();
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
    assert!(!replay.headers().contains_key(SET_COOKIE));
    assert_eq!(
        get(&fixture.app, "/auth/session", &credentials)
            .await
            .status(),
        StatusCode::OK
    );

    let value = json!({"name":"HTTP location","time_zone":"Europe/Moscow"});
    assert_eq!(
        create_location(
            &fixture,
            &identity,
            &credentials,
            ORIGIN,
            None,
            value.clone()
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        create_location(
            &fixture,
            &identity,
            &credentials,
            "https://foreign.test",
            Some(&credentials.csrf),
            value.clone()
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        create_location(
            &fixture,
            &identity,
            &credentials,
            ORIGIN,
            Some(&credentials.csrf),
            value
        )
        .await
        .status(),
        StatusCode::CREATED
    );

    let logout = fixture
        .app
        .clone()
        .oneshot(
            Request::post("/auth/logout")
                .header("cookie", &credentials.cookie)
                .header("origin", ORIGIN)
                .header("x-csrf-token", &credentials.csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    assert!(
        logout
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .all(|v| v.to_str().unwrap().contains("Max-Age=0"))
    );
    assert_eq!(
        get(&fixture.app, "/auth/session", &credentials)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database and runtime/admin/migrator URLs"]
async fn tenant_authorization_comes_from_session_and_cannot_be_spoofed() {
    let fixture = Fixture::new().await;
    let first = fixture.identity(Role::Owner).await;
    let second = fixture.identity(Role::Owner).await;
    let credentials = fixture.login(&first).await;
    let visible = body(get(&fixture.app, "/api/organizations", &credentials).await).await;
    assert_eq!(visible["items"].as_array().unwrap().len(), 1);
    assert_eq!(visible["items"][0]["id"], first.tenant.to_string());
    assert_eq!(
        get(
            &fixture.app,
            &format!("/api/organizations/{}", second.tenant),
            &credentials
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(
            &fixture.app,
            &format!("/api/organizations/{}", TenantId::new()),
            &credentials
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::get("/auth/session")
                .header("cookie", &credentials.cookie)
                .header("x-user-id", second.user.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(body(response).await["user_id"], first.user.to_string());
    let response = create_location(
        &fixture,
        &first,
        &credentials,
        ORIGIN,
        Some(&credentials.csrf),
        json!({"name":"Bad actor injection","time_zone":"UTC","user_id":second.user.to_string()}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database and runtime/admin/migrator URLs"]
async fn location_roles_validation_and_pagination_are_enforced_through_http() {
    let fixture = Fixture::new().await;
    let identity = fixture.identity(Role::Owner).await;
    let credentials = fixture.login(&identity).await;
    for n in 0..3 {
        assert_eq!(
            create_location(
                &fixture,
                &identity,
                &credentials,
                ORIGIN,
                Some(&credentials.csrf),
                json!({"name":format!("Location {n}"),"time_zone":"America/New_York"})
            )
            .await
            .status(),
            StatusCode::CREATED
        );
    }
    let path = format!("/api/organizations/{}/locations?limit=2", identity.tenant);
    let first = body(get(&fixture.app, &path, &credentials).await).await;
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    let cursor = first["next_cursor"].as_str().unwrap();
    let second = body(
        get(
            &fixture.app,
            &format!("{path}&after={cursor}"),
            &credentials,
        )
        .await,
    )
    .await;
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert!(second["next_cursor"].is_null());
    assert!(
        first["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["id"] != second["items"][0]["id"])
    );
    assert_eq!(
        get(
            &fixture.app,
            &format!("/api/organizations/{}/locations?limit=101", identity.tenant),
            &credentials
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        create_location(
            &fixture,
            &identity,
            &credentials,
            ORIGIN,
            Some(&credentials.csrf),
            json!({"name":"Invalid zone","time_zone":"Mars/Bad"})
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let operator = fixture.identity(Role::Operator).await;
    let operator_session = fixture.login(&operator).await;
    assert_eq!(
        create_location(
            &fixture,
            &operator,
            &operator_session,
            ORIGIN,
            Some(&operator_session.csrf),
            json!({"name":"Forbidden","time_zone":"UTC"})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database and runtime/admin/migrator URLs"]
async fn reauthentication_rotates_session_and_expiry_is_checked_on_each_request() {
    let fixture = Fixture::new().await;
    let identity = fixture.identity(Role::Owner).await;
    let old = fixture.login(&identity).await;
    let code = fixture.code(&identity).await;
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::post("/auth/login")
                .header("cookie", &old.cookie)
                .header("origin", ORIGIN)
                .header("content-type", "application/json")
                .body(Body::from(json!({"code":code}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let fresh = credentials(&response);
    assert_eq!(
        get(&fixture.app, "/auth/session", &old).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(&fixture.app, "/auth/session", &fresh).await.status(),
        StatusCode::OK
    );
    sqlx::query("UPDATE auth.sessions SET created_at=clock_timestamp()-interval '2 hours', expires_at=clock_timestamp()-interval '1 hour' WHERE token_hash=$1")
        .bind(fresh.session_hash.as_slice()).execute(&fixture.migrator).await.unwrap();
    assert_eq!(
        get(&fixture.app, "/auth/session", &fresh).await.status(),
        StatusCode::UNAUTHORIZED
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database and runtime/admin/migrator URLs"]
async fn https_login_sets_secure_host_only_cookies() {
    let fixture = Fixture::new().await;
    let identity = fixture.identity(Role::Owner).await;
    let code = fixture.code(&identity).await;
    let app = router(
        AppState::new(fixture.runtime.clone(), Duration::from_secs(1))
            .with_auth(AuthConfig::parse("https://turnesta.example", 3600).unwrap()),
    );
    let response = app
        .oneshot(
            Request::post("/auth/login")
                .header("origin", "https://turnesta.example")
                .header("content-type", "application/json")
                .body(Body::from(json!({"code":code}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get_all(SET_COOKIE).iter().all(|value| {
        let value = value.to_str().unwrap();
        value.starts_with("__Host-") && value.contains("; Secure") && !value.contains("Domain=")
    }));
    let _ = credentials(&response);
    fixture.close().await;
}

async fn patch(app: &Router, path: &str, credentials: &Credentials, value: Value) -> Response {
    app.clone()
        .oneshot(
            Request::patch(path)
                .header("cookie", &credentials.cookie)
                .header("origin", ORIGIN)
                .header("x-csrf-token", &credentials.csrf)
                .header("content-type", "application/json")
                .body(Body::from(value.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database and runtime/admin/migrator URLs"]
async fn concurrent_organization_updates_preserve_one_winner_and_report_conflict() {
    let fixture = Fixture::new().await;
    let identity = fixture.identity(Role::Owner).await;
    let credentials = fixture.login(&identity).await;
    let path = format!("/api/organizations/{}", identity.tenant);
    let (first, second) = tokio::join!(
        patch(
            &fixture.app,
            &path,
            &credentials,
            json!({"name":"First editor","expected_version":1})
        ),
        patch(
            &fixture.app,
            &path,
            &credentials,
            json!({"name":"Second editor","expected_version":1})
        ),
    );
    assert!(matches!(
        (first.status(), second.status()),
        (StatusCode::OK, StatusCode::CONFLICT) | (StatusCode::CONFLICT, StatusCode::OK)
    ));
    let current = body(get(&fixture.app, &path, &credentials).await).await;
    assert_eq!(current["version"], 2);
    assert!(current["name"] == "First editor" || current["name"] == "Second editor");
    assert_eq!(
        patch(
            &fixture.app,
            &path,
            &credentials,
            json!({"name":"Bad version","expected_version":0})
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database and runtime/admin/migrator URLs"]
async fn location_edits_check_versions_timezone_and_tenant_before_commit() {
    let fixture = Fixture::new().await;
    let identity = fixture.identity(Role::Manager).await;
    let credentials = fixture.login(&identity).await;
    let created = create_location(
        &fixture,
        &identity,
        &credentials,
        ORIGIN,
        Some(&credentials.csrf),
        json!({"name":"Original","time_zone":"UTC"}),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = body(created).await;
    let path = format!(
        "/api/organizations/{}/locations/{}",
        identity.tenant,
        created["id"].as_str().unwrap()
    );
    let updated = patch(
        &fixture.app,
        &path,
        &credentials,
        json!({"name":"Updated","time_zone":"Europe/Moscow","expected_version":1}),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let updated = body(updated).await;
    assert_eq!(updated["version"], 2);
    assert_eq!(updated["time_zone"], "Europe/Moscow");
    let stale = patch(
        &fixture.app,
        &path,
        &credentials,
        json!({"name":"Stale","time_zone":"UTC","expected_version":1}),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(body(stale).await["code"], "version_conflict");
    assert_eq!(
        patch(
            &fixture.app,
            &path,
            &credentials,
            json!({"name":"Bad zone","time_zone":"Mars/Bad","expected_version":2})
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let foreign = fixture.identity(Role::Owner).await;
    let wrong_path = format!(
        "/api/organizations/{}/locations/{}",
        foreign.tenant,
        created["id"].as_str().unwrap()
    );
    assert_eq!(
        patch(
            &fixture.app,
            &wrong_path,
            &credentials,
            json!({"name":"Cross tenant","time_zone":"UTC","expected_version":2})
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let missing = format!(
        "/api/organizations/{}/locations/{}",
        identity.tenant,
        uuid::Uuid::new_v4()
    );
    assert_eq!(
        patch(
            &fixture.app,
            &missing,
            &credentials,
            json!({"name":"Missing","time_zone":"UTC","expected_version":1})
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database and runtime/admin/migrator URLs"]
async fn readonly_members_cannot_rename_and_timezone_lookup_is_bounded() {
    let fixture = Fixture::new().await;
    let identity = fixture.identity(Role::Operator).await;
    let credentials = fixture.login(&identity).await;
    let path = format!("/api/organizations/{}", identity.tenant);
    assert_eq!(
        patch(
            &fixture.app,
            &path,
            &credentials,
            json!({"name":"Forbidden","expected_version":1})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let zones = body(
        get(
            &fixture.app,
            "/api/time-zones?query=Moscow&limit=20",
            &credentials,
        )
        .await,
    )
    .await;
    assert!(
        zones["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|zone| zone == "Europe/Moscow")
    );
    assert_eq!(
        get(&fixture.app, "/api/time-zones?limit=10000", &credentials)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    fixture.close().await;
}

async fn navigation_request(
    app: &Router,
    path: &str,
    credentials: &Credentials,
    origin: &str,
    csrf: Option<&str>,
    value: Value,
) -> Response {
    let mut request = Request::put(path)
        .header("cookie", &credentials.cookie)
        .header("origin", origin)
        .header("content-type", "application/json");
    if let Some(csrf) = csrf {
        request = request.header("x-csrf-token", csrf);
    }
    app.clone()
        .oneshot(request.body(Body::from(value.to_string())).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database and runtime/admin/migrator URLs"]
async fn navigation_preferences_check_roles_csrf_tenant_and_strict_choices() {
    let f = Fixture::new().await;
    for role in [Role::Owner, Role::Manager, Role::Operator, Role::Employee] {
        let identity = f.identity(role).await;
        let credentials = f.login(&identity).await;
        let org_path = format!("/api/organizations/{}", identity.tenant);
        let path = format!("{org_path}/navigation");
        let before = body(get(&f.app, &org_path, &credentials).await).await;
        assert_eq!(
            before["visible_services"],
            json!(["queues", "bookings", "tips"])
        );
        let input = json!({"visible_services":["bookings"],"expected_version":1});
        for (origin, csrf) in [
            ("https://untrusted.example", Some(credentials.csrf.as_str())),
            (ORIGIN, None),
        ] {
            assert_eq!(
                navigation_request(&f.app, &path, &credentials, origin, csrf, input.clone())
                    .await
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
        let foreign = f.identity(Role::Owner).await;
        assert_eq!(
            navigation_request(
                &f.app,
                &format!("/api/organizations/{}/navigation", foreign.tenant),
                &credentials,
                ORIGIN,
                Some(&credentials.csrf),
                input.clone()
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        if matches!(role, Role::Owner | Role::Manager) {
            for invalid in [
                json!({"visible_services":["unknown"],"expected_version":1}),
                json!({"visible_services":["queues","queues"],"expected_version":1}),
                json!({"visible_services":[null],"expected_version":1}),
                json!({"visible_services":[],"expected_version":0}),
                json!({"visible_services":[],"expected_version":1,"actor":"spoof"}),
            ] {
                assert_eq!(
                    navigation_request(
                        &f.app,
                        &path,
                        &credentials,
                        ORIGIN,
                        Some(&credentials.csrf),
                        invalid
                    )
                    .await
                    .status(),
                    StatusCode::BAD_REQUEST
                );
            }
            assert_eq!(
                body(get(&f.app, &org_path, &credentials).await).await,
                before
            );
            let saved = navigation_request(
                &f.app,
                &path,
                &credentials,
                ORIGIN,
                Some(&credentials.csrf),
                input.clone(),
            )
            .await;
            assert_eq!(saved.status(), StatusCode::OK);
            let saved = body(saved).await;
            assert_eq!(saved["visible_services"], json!(["bookings"]));
            assert_eq!(saved["version"], 2);
            assert_eq!(saved["name"], before["name"]);
            assert_eq!(
                navigation_request(
                    &f.app,
                    &path,
                    &credentials,
                    ORIGIN,
                    Some(&credentials.csrf),
                    input
                )
                .await
                .status(),
                StatusCode::CONFLICT
            );
            let empty = navigation_request(
                &f.app,
                &path,
                &credentials,
                ORIGIN,
                Some(&credentials.csrf),
                json!({"visible_services":[],"expected_version":2}),
            )
            .await;
            assert_eq!(empty.status(), StatusCode::OK);
            assert_eq!(body(empty).await["visible_services"], json!([]));
            let restored = navigation_request(
                &f.app,
                &path,
                &credentials,
                ORIGIN,
                Some(&credentials.csrf),
                json!({"visible_services":["queues","bookings","tips"],"expected_version":3}),
            )
            .await;
            assert_eq!(restored.status(), StatusCode::OK);
        } else {
            assert_eq!(
                navigation_request(
                    &f.app,
                    &path,
                    &credentials,
                    ORIGIN,
                    Some(&credentials.csrf),
                    input
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
            assert_eq!(
                body(get(&f.app, &org_path, &credentials).await).await,
                before
            );
        }
    }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database and runtime/admin/migrator URLs"]
async fn navigation_and_name_share_cas_and_database_guards_reject_invalid_arrays() {
    let f = Fixture::new().await;
    let identity = f.identity(Role::Owner).await;
    let credentials = f.login(&identity).await;
    let org_path = format!("/api/organizations/{}", identity.tenant);
    let path = format!("{org_path}/navigation");
    for selection in [
        Some(vec![Some("unknown")]),
        Some(vec![Some("queues"), Some("queues")]),
        Some(vec![Some("queues"), None]),
        None,
    ] {
        let error = sqlx::query("UPDATE core.organizations SET visible_services=$1 WHERE id=$2")
            .bind(selection)
            .bind(identity.tenant)
            .execute(&f.migrator)
            .await
            .unwrap_err();
        assert!(matches!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("23514" | "23502")
        ));
    }
    let (navigation, rename) = tokio::join!(
        navigation_request(
            &f.app,
            &path,
            &credentials,
            ORIGIN,
            Some(&credentials.csrf),
            json!({"visible_services":["bookings"],"expected_version":1})
        ),
        patch(
            &f.app,
            &org_path,
            &credentials,
            json!({"name":"New name","expected_version":1})
        )
    );
    let statuses = [navigation.status(), rename.status()];
    assert_eq!(statuses.iter().filter(|s| **s == StatusCode::OK).count(), 1);
    assert_eq!(
        statuses
            .iter()
            .filter(|s| **s == StatusCode::CONFLICT)
            .count(),
        1
    );
    let current = body(get(&f.app, &org_path, &credentials).await).await;
    assert_eq!(current["version"], 2);
    if navigation.status() == StatusCode::OK {
        assert_eq!(current["visible_services"], json!(["bookings"]));
        assert_eq!(current["name"], "HTTP auth test");
    } else {
        assert_eq!(
            current["visible_services"],
            json!(["queues", "bookings", "tips"])
        );
        assert_eq!(current["name"], "New name");
    }
    f.close().await;
}
