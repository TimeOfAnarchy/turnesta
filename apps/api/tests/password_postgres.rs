use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header::SET_COOKIE},
    response::Response,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction, postgres::PgPoolOptions};
use std::time::Duration;
use tower::ServiceExt;
use turnesta_api::{
    auth::{
        login::{self, Email},
        password::{self, PasswordEngine},
    },
    core::{Name, Provisioner, TenantId, TenantSession, UserId},
    http::{AppState, router},
};
use uuid::Uuid;
use zeroize::Zeroizing;

const ORIGIN: &str = "http://127.0.0.1:8080";
const TEMP: &str = "Temporary access phrase 2026";
const PERSONAL: &str = "My own personal passphrase 2026";
const NEXT: &str = "A different personal passphrase 2026";

struct Fixture {
    runtime: PgPool,
    admin: PgPool,
    migrator: PgPool,
    worker: PgPool,
    app: Router,
    tenant: TenantId,
    user: UserId,
    email: Email,
    guard: Transaction<'static, Postgres>,
}
struct Session {
    cookie: String,
    csrf: String,
    hash: [u8; 32],
}
async fn pool(key: &str) -> PgPool {
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(3))
        .connect(&std::env::var(key).unwrap())
        .await
        .unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(database.ends_with("_test"));
    pool
}
fn session(response: &Response) -> Session {
    let cookies: Vec<_> = response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap())
        .collect();
    let session = cookies
        .iter()
        .find(|s| s.starts_with("turnesta_session="))
        .unwrap();
    let csrf = cookies
        .iter()
        .find(|s| s.starts_with("turnesta_csrf="))
        .unwrap();
    assert!(session.contains("HttpOnly") && session.contains("SameSite=Lax"));
    let session = session.split(';').next().unwrap();
    let csrf = csrf.split(';').next().unwrap();
    let bytes = hex::decode(session.split_once('=').unwrap().1).unwrap();
    Session {
        cookie: format!("{session}; {csrf}"),
        csrf: csrf.split_once('=').unwrap().1.into(),
        hash: Sha256::digest(bytes).into(),
    }
}
async fn body(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    }
}
impl Fixture {
    async fn new() -> Self {
        let runtime = pool("TEST_DATABASE_URL").await;
        let admin = pool("TEST_ADMIN_DATABASE_URL").await;
        let migrator = pool("TEST_MIGRATION_DATABASE_URL").await;
        let worker = pool("TEST_WORKER_DATABASE_URL").await;
        let mut guard = migrator.begin().await.unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('turnesta.password.tests',0))")
            .execute(&mut *guard)
            .await
            .unwrap();
        sqlx::query("DELETE FROM auth.password_attempt_windows")
            .execute(&migrator)
            .await
            .unwrap();
        let org = Provisioner::new(admin.clone())
            .create_organization(
                &Name::parse("Password test").unwrap(),
                &Name::parse("Owner").unwrap(),
            )
            .await
            .unwrap();
        let email = Email::parse(&format!("password-{}@example.test", Uuid::new_v4())).unwrap();
        login::bind_email(&admin, org.owner.id, &email)
            .await
            .unwrap();
        let app = router(AppState::new(runtime.clone(), Duration::from_secs(1)));
        Self {
            runtime,
            admin,
            migrator,
            worker,
            app,
            tenant: org.organization.id,
            user: org.owner.id,
            email,
            guard,
        }
    }
    async fn close(self) {
        self.guard.rollback().await.unwrap();
        self.runtime.close().await;
        self.admin.close().await;
        self.migrator.close().await;
        self.worker.close().await;
    }
    async fn post(&self, path: &str, value: Value, session: Option<&Session>) -> Response {
        let mut request = Request::post(path)
            .header("origin", ORIGIN)
            .header("content-type", "application/json");
        if let Some(session) = session {
            request = request
                .header("cookie", &session.cookie)
                .header("x-csrf-token", &session.csrf);
        }
        self.app
            .clone()
            .oneshot(request.body(Body::from(value.to_string())).unwrap())
            .await
            .unwrap()
    }
    async fn get(&self, path: &str, session: Option<&Session>) -> Response {
        let mut request = Request::get(path);
        if let Some(session) = session {
            request = request.header("cookie", &session.cookie);
        }
        self.app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }
    async fn admin_password(&self, password: &str) {
        let hash = PasswordEngine::default()
            .hash(Zeroizing::new(password.into()))
            .await
            .unwrap();
        password::admin_set(&self.admin, self.user, &self.email, &hash)
            .await
            .unwrap();
    }
    async fn password_login(&self, email: &str, password: &str) -> Response {
        self.post(
            "/auth/password/login",
            json!({"email":email,"password":password}),
            None,
        )
        .await
    }
    async fn setup_token(&self) -> String {
        let response = self.password_login(self.email.as_str(), TEMP).await;
        assert_eq!(response.status(), StatusCode::OK);
        for cookie in response.headers().get_all(SET_COOKIE) {
            assert!(cookie.to_str().unwrap().contains("Max-Age=0"));
        }
        let result = body(response).await;
        assert_eq!(result["status"], "password_change_required");
        assert!(result.get("user_id").is_none());
        result["setup_token"].as_str().unwrap().into()
    }
    async fn activate(&self) -> Session {
        self.admin_password(TEMP).await;
        let token = self.setup_token().await;
        let response = self
            .post(
                "/auth/password/setup",
                json!({"setup_token":token,"new_password":PERSONAL}),
                None,
            )
            .await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{}",
            body_error(&response)
        );
        session(&response)
    }
    async fn magic_code(&self) -> String {
        let mut token = [0; 32];
        getrandom::fill(&mut token).unwrap();
        login::issue_login_code(&self.admin, &self.email, &Sha256::digest(token).into(), 600)
            .await
            .unwrap();
        hex::encode(token)
    }
    async fn magic_login(&self) -> Session {
        let response = self
            .post("/auth/login", json!({"code":self.magic_code().await}), None)
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        session(&response)
    }
    fn account_path(&self) -> String {
        format!("/api/organizations/{}/team/password-accounts", self.tenant)
    }
}
fn body_error(response: &Response) -> String {
    format!("response status {}", response.status())
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn temporary_login_only_allows_setup_then_two_devices_keep_independent_sessions() {
    let f = Fixture::new().await;
    f.admin_password(TEMP).await;
    let token = f.setup_token().await;
    assert_eq!(
        f.get("/api/organizations", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let response = f
        .post(
            "/auth/password/setup",
            json!({"setup_token":token,"new_password":TEMP}),
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body(response).await["code"], "password_unchanged");
    let response = f
        .post(
            "/auth/password/setup",
            json!({"setup_token":token,"new_password":PERSONAL}),
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let pc = session(&response);
    let result = body(response).await;
    assert_eq!(result["status"], "authenticated");
    assert!(!result.to_string().contains(PERSONAL));
    assert_eq!(
        f.post(
            "/auth/password/setup",
            json!({"setup_token":token,"new_password":NEXT}),
            None
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.password_login(f.email.as_str(), TEMP).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let response = f
        .password_login(&format!(" {} ", f.email.as_str().to_uppercase()), PERSONAL)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let phone = session(&response);
    assert_ne!(pc.hash, phone.hash);
    for device in [&pc, &phone] {
        assert_eq!(
            f.get("/auth/session", Some(device)).await.status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        f.post("/auth/logout", json!({}), Some(&pc)).await.status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        f.get("/auth/session", Some(&pc)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.get("/auth/session", Some(&phone)).await.status(),
        StatusCode::OK
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn password_change_and_admin_reset_revoke_sessions_and_reject_stale_verification() {
    let f = Fixture::new().await;
    let pc = f.activate().await;
    let phone = session(&f.password_login(f.email.as_str(), PERSONAL).await);
    let snapshot = password::lookup(&f.runtime, &f.email)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        f.post(
            "/auth/password",
            json!({"current_password":"wrong","new_password":NEXT}),
            Some(&pc)
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let request = Request::post("/auth/password")
        .header("origin", ORIGIN)
        .header("cookie", &pc.cookie)
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"current_password":PERSONAL,"new_password":NEXT}).to_string(),
        ))
        .unwrap();
    assert_eq!(
        f.app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.post(
            "/auth/password",
            json!({"current_password":PERSONAL,"new_password":NEXT}),
            Some(&pc)
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    for device in [&pc, &phone] {
        assert_eq!(
            f.get("/auth/session", Some(device)).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        f.password_login(f.email.as_str(), PERSONAL).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let live = session(&f.password_login(f.email.as_str(), NEXT).await);
    f.admin_password(TEMP).await;
    assert_eq!(
        f.get("/auth/session", Some(&live)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let stale: Option<UserId> =
        sqlx::query_scalar("SELECT auth.password_finish_login($1,$2,$3,$4,$5,NULL)")
            .bind(snapshot.identity_id)
            .bind(snapshot.version)
            .bind([19u8; 32].as_slice())
            .bind([20u8; 32].as_slice())
            .bind(3600)
            .fetch_one(&f.runtime)
            .await
            .unwrap();
    assert!(stale.is_none());
    let hash = PasswordEngine::default()
        .hash(Zeroizing::new(NEXT.into()))
        .await
        .unwrap();
    let stale: bool = sqlx::query_scalar("SELECT auth.password_change($1,$2,$3)")
        .bind(pc.hash.as_slice())
        .bind(snapshot.version)
        .bind(hash.as_str())
        .fetch_one(&f.runtime)
        .await
        .unwrap();
    assert!(!stale);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn existing_magic_link_account_can_set_password_without_rebinding_identity() {
    let f = Fixture::new().await;
    let old = f.magic_login().await;
    let pending = f.magic_code().await;
    let status = body(f.get("/auth/password", Some(&old)).await).await;
    assert_eq!(status["has_password"], false);
    assert_eq!(status["email"], f.email.as_str());
    assert_eq!(
        f.post(
            "/auth/password",
            json!({"new_password":PERSONAL}),
            Some(&old)
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        f.get("/auth/session", Some(&old)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.post("/auth/login", json!({"code":pending}), None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = f.password_login(f.email.as_str(), PERSONAL).await;
    assert_eq!(response.status(), StatusCode::OK);
    let live = session(&response);
    assert_eq!(
        body(f.get("/auth/password", Some(&live)).await).await["has_password"],
        true
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn setup_grants_expire_are_replaced_and_cannot_survive_admin_reset() {
    let f = Fixture::new().await;
    f.admin_password(TEMP).await;
    let old = f.setup_token().await;
    let active = f.setup_token().await;
    assert_eq!(
        f.post(
            "/auth/password/setup",
            json!({"setup_token":old,"new_password":PERSONAL}),
            None
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let magic = f.magic_code().await;
    f.admin_password(TEMP).await;
    assert_eq!(
        f.post("/auth/login", json!({"code":magic}), None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.post(
            "/auth/password/setup",
            json!({"setup_token":active,"new_password":PERSONAL}),
            None
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let expired = f.setup_token().await;
    sqlx::query("UPDATE auth.password_setup_grants SET expires_at=clock_timestamp()-interval '1 second' WHERE identity_id=(SELECT id FROM auth.identities WHERE subject=$1)")
        .bind(f.email.as_str()).execute(&f.migrator).await.unwrap();
    assert_eq!(
        f.post(
            "/auth/password/setup",
            json!({"setup_token":expired,"new_password":PERSONAL}),
            None
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn wrong_and_unknown_emails_share_errors_and_database_throttling_across_routers() {
    let f = Fixture::new().await;
    f.admin_password(TEMP).await;
    let missing = format!("absent-{}@example.test", Uuid::new_v4());
    for attempt in 0..11 {
        let known = f
            .password_login(f.email.as_str(), "A wrong but long passphrase")
            .await;
        let unknown = f
            .password_login(&missing, "A wrong but long passphrase")
            .await;
        let expected = if attempt < 10 {
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::TOO_MANY_REQUESTS
        };
        assert_eq!(known.status(), expected);
        assert_eq!(unknown.status(), expected);
        assert_eq!(body(known).await, body(unknown).await);
    }
    let other = router(AppState::new(f.runtime.clone(), Duration::from_secs(1)));
    let request = Request::post("/auth/password/login")
        .header("origin", ORIGIN)
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"email":missing,"password":TEMP}).to_string(),
        ))
        .unwrap();
    assert_eq!(
        other.oneshot(request).await.unwrap().status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    sqlx::query("UPDATE auth.password_attempt_windows SET window_start=clock_timestamp()-interval '6 minutes'").execute(&f.migrator).await.unwrap();
    assert_eq!(
        f.password_login(&missing, TEMP).await.status(),
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("UPDATE auth.password_attempt_windows SET attempts=200 WHERE bucket=sha256(convert_to('turnesta.password.global:login','UTF8'))").execute(&f.migrator).await.unwrap();
    assert_eq!(
        f.password_login(&format!("another-{}@example.test", Uuid::new_v4()), TEMP)
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn team_owner_creates_only_new_global_accounts_with_private_secret_free_audit() {
    let f = Fixture::new().await;
    let owner = f.magic_login().await;
    let email = format!("staff-{}@example.test", Uuid::new_v4());
    let command =
        json!({"display_name":"Staff","email":email,"role":"operator","temporary_password":TEMP});
    let response = f
        .post(&f.account_path(), command.clone(), Some(&owner))
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let staff: Uuid = body(response).await["user_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        f.post(&f.account_path(), command, Some(&owner))
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let login = body(f.password_login(&email, TEMP).await).await;
    assert_eq!(login["status"], "password_change_required");
    let response = f
        .post(
            "/auth/password/setup",
            json!({"setup_token":login["setup_token"],"new_password":PERSONAL}),
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let staff_session = session(&response);
    let rejected=f.post(&f.account_path(),json!({"display_name":"Intruder","email":f.email.as_str(),"role":"operator","temporary_password":NEXT}),Some(&owner)).await;
    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    let denied=f.post(&f.account_path(),json!({"display_name":"Intruder","email":format!("denied-{}@example.test",Uuid::new_v4()),"role":"operator","temporary_password":NEXT}),Some(&staff_session)).await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let command: Value = sqlx::query_scalar(
        "SELECT command FROM core.team_commands WHERE tenant_id=$1 AND result->>'user_id'=$2",
    )
    .bind(f.tenant)
    .bind(staff.to_string())
    .fetch_one(&f.migrator)
    .await
    .unwrap();
    assert_eq!(command["type"], "password_account");
    assert!(!command.to_string().contains(TEMP));
    assert!(!command.to_string().contains("$argon2"));
    assert!(!command.to_string().contains("password_hash"));
    assert_eq!(
        f.get("/auth/session", Some(&owner)).await.status(),
        StatusCode::OK
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn account_creation_race_and_failed_hash_constraint_leave_no_partial_account() {
    let f = Fixture::new().await;
    let owner = f.magic_login().await;
    let email = format!("race-{}@example.test", Uuid::new_v4());
    let command = json!({"display_name":"Concurrent staff","email":email,"role":"operator","temporary_password":TEMP});
    let path = f.account_path();
    let (a, b) = tokio::join!(
        f.post(&path, command.clone(), Some(&owner)),
        f.post(&path, command, Some(&owner))
    );
    let mut statuses = [a.status().as_u16(), b.status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [201, 409]);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.identities WHERE subject=$1")
        .bind(email)
        .fetch_one(&f.migrator)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let failed = format!("rollback-{}@example.test", Uuid::new_v4());
    let mut scope = TenantSession::begin(&f.runtime, f.user, f.tenant)
        .await
        .unwrap();
    let error =
        sqlx::query("SELECT core.team_password_account($1,$2,'Invalid hash','operator','bad')")
            .bind(f.tenant)
            .bind(&failed)
            .execute(scope.connection())
            .await
            .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    drop(scope);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.identities WHERE subject=$1")
        .bind(failed)
        .fetch_one(&f.migrator)
        .await
        .unwrap();
    assert_eq!(count, 0);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn concurrent_setup_consumption_has_one_winner_and_reset_cannot_issue_stale_session() {
    let f = Fixture::new().await;
    f.admin_password(TEMP).await;
    let _token = f.setup_token().await;
    let snapshot = password::lookup(&f.runtime, &f.email)
        .await
        .unwrap()
        .unwrap();
    let grant:Vec<u8>=sqlx::query_scalar("SELECT token_hash FROM auth.password_setup_grants WHERE identity_id=$1 AND used_at IS NULL")
        .bind(snapshot.identity_id).fetch_one(&f.migrator).await.unwrap();
    let hash = PasswordEngine::default()
        .hash(Zeroizing::new(PERSONAL.into()))
        .await
        .unwrap();
    let consume = |session: [u8; 32]| {
        sqlx::query_scalar::<_, Option<UserId>>(
            "SELECT auth.password_setup_finish($1,$2,$3,$4,$5,3600,NULL)",
        )
        .bind(grant.clone())
        .bind(snapshot.version)
        .bind(hash.to_string())
        .bind(session.to_vec())
        .bind([7u8; 32].to_vec())
        .fetch_one(&f.runtime)
    };
    let mut first = [0; 32];
    let mut second = [0; 32];
    getrandom::fill(&mut first).unwrap();
    getrandom::fill(&mut second).unwrap();
    let (a, b) = tokio::join!(consume(first), consume(second));
    assert_eq!(
        [a.unwrap(), b.unwrap()]
            .iter()
            .filter(|r| r.is_some())
            .count(),
        1
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.sessions WHERE user_id=$1 AND revoked_at IS NULL",
    )
    .bind(f.user)
    .fetch_one(&f.migrator)
    .await
    .unwrap();
    assert_eq!(count, 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn credentials_and_setup_tables_have_no_direct_runtime_worker_or_provisioner_access() {
    let f = Fixture::new().await;
    for pool in [&f.runtime, &f.admin, &f.worker] {
        for query in [
            "SELECT * FROM auth.password_credentials",
            "SELECT * FROM auth.password_setup_grants",
            "SELECT * FROM auth.password_attempt_windows",
        ] {
            let error = sqlx::query(query).fetch_all(pool).await.unwrap_err();
            assert_eq!(
                error.as_database_error().unwrap().code().as_deref(),
                Some("42501")
            );
        }
    }
    let error = sqlx::query("SELECT auth.password_admin_set($1,$2,'bad')")
        .bind(f.user)
        .bind(f.email.as_str())
        .execute(&f.runtime)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    let error = sqlx::query("SELECT auth.password_lookup($1)")
        .bind(f.email.as_str())
        .execute(&f.worker)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn password_http_checks_origin_strict_bodies_limits_and_owner_role() {
    let f = Fixture::new().await;
    for path in ["/auth/password/login", "/auth/password/setup"] {
        for origin in [None, Some("https://foreign.example")] {
            let mut request = Request::post(path).header("content-type", "application/json");
            if let Some(origin) = origin {
                request = request.header("origin", origin);
            }
            assert_eq!(
                f.app
                    .clone()
                    .oneshot(request.body(Body::from("{}")).unwrap())
                    .await
                    .unwrap()
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
    }
    assert_eq!(
        f.post(
            "/auth/password/login",
            json!({"email":f.email.as_str(),"password":TEMP,"role":"owner"}),
            None
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.post(
            "/auth/password/login",
            json!({"email":f.email.as_str(),"password":"x".repeat(9000)}),
            None
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let owner = f.magic_login().await;
    assert_eq!(f.post(&f.account_path(),json!({"display_name":"Owner","email":format!("owner-{}@example.test",Uuid::new_v4()),"role":"owner","temporary_password":TEMP}),Some(&owner)).await.status(),StatusCode::FORBIDDEN);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn tenant_owner_cannot_override_a_global_account_from_another_organization() {
    let f = Fixture::new().await;
    let owner = f.magic_login().await;
    let foreign = Provisioner::new(f.admin.clone())
        .create_organization(
            &Name::parse("Other organization").unwrap(),
            &Name::parse("Other owner").unwrap(),
        )
        .await
        .unwrap();
    let email = Email::parse(&format!("foreign-{}@example.test", Uuid::new_v4())).unwrap();
    let hash = PasswordEngine::default()
        .hash(Zeroizing::new(PERSONAL.into()))
        .await
        .unwrap();
    password::admin_set(&f.admin, foreign.owner.id, &email, &hash)
        .await
        .unwrap();
    let before = password::lookup(&f.runtime, &email).await.unwrap().unwrap();
    let response=f.post(&f.account_path(),json!({"display_name":"Replacement","email":email.as_str(),"role":"operator","temporary_password":NEXT}),Some(&owner)).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let after = password::lookup(&f.runtime, &email).await.unwrap().unwrap();
    assert_eq!(before.password_hash, after.password_hash);
    assert_eq!(before.version, after.version);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM core.memberships WHERE tenant_id=$1 AND user_id=$2",
    )
    .bind(f.tenant)
    .bind(foreign.owner.id)
    .fetch_one(&f.migrator)
    .await
    .unwrap();
    assert_eq!(count, 0);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn setup_grant_cannot_bypass_the_hashing_rate_budget() {
    let f = Fixture::new().await;
    f.admin_password(TEMP).await;
    let token = f.setup_token().await;
    for attempt in 0..11 {
        let response = f
            .post(
                "/auth/password/setup",
                json!({"setup_token":token,"new_password":TEMP}),
                None,
            )
            .await;
        assert_eq!(
            response.status(),
            if attempt < 10 {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::TOO_MANY_REQUESTS
            }
        );
    }
    let credential = password::lookup(&f.runtime, &f.email)
        .await
        .unwrap()
        .unwrap();
    assert!(credential.must_change);
    assert_eq!(credential.version, 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn mailbox_claim_revokes_unverified_owner_issued_password_before_cross_org_access() {
    let f = Fixture::new().await;
    let owner = f.magic_login().await;
    let email = Email::parse(&format!("claim-{}@example.test", Uuid::new_v4())).unwrap();
    let created=f.post(&f.account_path(),json!({"display_name":"New staff","email":email.as_str(),"role":"operator","temporary_password":TEMP}),Some(&owner)).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let target: UserId = serde_json::from_value(body(created).await["user_id"].clone()).unwrap();
    let setup = body(f.password_login(email.as_str(), TEMP).await).await;
    let access = f
        .post(
            "/auth/password/setup",
            json!({"setup_token":setup["setup_token"],"new_password":PERSONAL}),
            None,
        )
        .await;
    assert_eq!(access.status(), StatusCode::OK);
    let unverified_session = session(&access);
    let snapshot = password::lookup(&f.runtime, &email).await.unwrap().unwrap();
    let foreign = Provisioner::new(f.admin.clone())
        .create_organization(
            &Name::parse("Other clinic").unwrap(),
            &Name::parse("Other owner").unwrap(),
        )
        .await
        .unwrap();
    let foreign_email = Email::parse(&format!("inviter-{}@example.test", Uuid::new_v4())).unwrap();
    login::bind_email(&f.admin, foreign.owner.id, &foreign_email)
        .await
        .unwrap();
    let mut code = [0; 32];
    getrandom::fill(&mut code).unwrap();
    login::issue_login_code(&f.admin, &foreign_email, &Sha256::digest(code).into(), 600)
        .await
        .unwrap();
    let inviter = session(
        &f.post("/auth/login", json!({"code":hex::encode(code)}), None)
            .await,
    );
    let invitation = |key: Uuid| {
        Request::post(format!("/api/organizations/{}/team/commands",foreign.organization.id))
        .header("origin",ORIGIN).header("cookie",&inviter.cookie).header("x-csrf-token",&inviter.csrf)
        .header("idempotency-key",key.to_string()).header("content-type","application/json")
        .body(Body::from(json!({"type":"invite","display_name":"Staff","email":email.as_str(),"role":"operator"}).to_string())).unwrap()
    };
    let denied = f
        .app
        .clone()
        .oneshot(invitation(Uuid::new_v4()))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::CONFLICT);
    assert_eq!(body(denied).await["code"], "account_email_unverified");
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM core.memberships WHERE tenant_id=$1 AND user_id=$2",
    )
    .bind(foreign.organization.id)
    .bind(target)
    .fetch_one(&f.migrator)
    .await
    .unwrap();
    assert_eq!(count, 0);
    getrandom::fill(&mut code).unwrap();
    login::issue_login_code(&f.admin, &email, &Sha256::digest(code).into(), 600)
        .await
        .unwrap();
    let claimed = f
        .post("/auth/login", json!({"code":hex::encode(code)}), None)
        .await;
    assert_eq!(claimed.status(), StatusCode::OK);
    let mailbox = session(&claimed);
    assert_eq!(
        f.get("/auth/session", Some(&unverified_session))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.password_login(email.as_str(), PERSONAL).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(
        password::lookup(&f.runtime, &email)
            .await
            .unwrap()
            .is_none()
    );
    let old: Option<UserId> =
        sqlx::query_scalar("SELECT auth.password_finish_login($1,$2,$3,$4,3600,NULL)")
            .bind(snapshot.identity_id)
            .bind(snapshot.version)
            .bind([29u8; 32].as_slice())
            .bind([30u8; 32].as_slice())
            .fetch_one(&f.runtime)
            .await
            .unwrap();
    assert!(old.is_none());
    let status = body(f.get("/auth/password", Some(&mailbox)).await).await;
    assert_eq!(status["requires_current_password"], false);
    assert_eq!(status["has_password"], false);
    assert_eq!(
        f.post(
            "/auth/password",
            json!({"new_password":PERSONAL}),
            Some(&mailbox)
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.post(
            "/auth/password",
            json!({"new_password":NEXT}),
            Some(&mailbox)
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let current = password::lookup(&f.runtime, &email).await.unwrap().unwrap();
    assert!(current.version > snapshot.version);
    assert_eq!(
        f.password_login(email.as_str(), NEXT).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        f.password_login(email.as_str(), PERSONAL).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let admitted = f
        .app
        .clone()
        .oneshot(invitation(Uuid::new_v4()))
        .await
        .unwrap();
    assert_eq!(admitted.status(), StatusCode::OK);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn tenant_membership_is_checked_before_body_and_uses_decoded_route_captures() {
    let f = Fixture::new().await;
    let signed_in = f.magic_login().await;
    let foreign = Uuid::new_v4().to_string();
    let encoded = |value: &str| {
        value
            .bytes()
            .map(|byte| format!("%{byte:02X}"))
            .collect::<String>()
    };
    for tenant in [foreign.clone(), encoded(&foreign)] {
        let request = Request::post(format!("/api/organizations/{tenant}/team/commands"))
            .header("cookie", &signed_in.cookie)
            .header("origin", ORIGIN)
            .header("x-csrf-token", &signed_in.csrf)
            .header("content-type", "application/json")
            .body(Body::from_stream(futures_util::stream::pending::<
                Result<axum::body::Bytes, std::io::Error>,
            >()))
            .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(2), f.app.clone().oneshot(request))
            .await
            .expect("nonmember request must not wait for its body")
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    assert_eq!(
        f.get(
            &format!("/api/organizations/{}", encoded(&f.tenant.to_string())),
            Some(&signed_in)
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        f.get("/api/organizations/%FF", Some(&signed_in))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn fresh_mailbox_proof_recovers_temporary_password_and_invalidates_setup_grant() {
    let f = Fixture::new().await;
    f.admin_password(TEMP).await;
    let token = f.setup_token().await;
    let mailbox = f.magic_login().await;
    let status = body(f.get("/auth/password", Some(&mailbox)).await).await;
    assert_eq!(status["has_password"], false);
    assert_eq!(status["requires_current_password"], false);
    assert_eq!(
        f.post(
            "/auth/password/setup",
            json!({"setup_token":token,"new_password":PERSONAL}),
            None
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.post(
            "/auth/password",
            json!({"new_password":PERSONAL}),
            Some(&mailbox)
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        f.password_login(f.email.as_str(), TEMP).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.password_login(f.email.as_str(), PERSONAL).await.status(),
        StatusCode::OK
    );
    f.close().await;
}
