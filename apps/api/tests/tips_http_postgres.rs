use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header::SET_COOKIE},
    response::Response,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection, PgPool, postgres::PgPoolOptions, types::Json};
use tower::ServiceExt;
use turnesta_api::{
    auth::login::{self, Email},
    core::{Name, Provisioner, Role, TenantId, UserId},
    http::{AppState, router},
    payments::PaymentConfig,
    tips::MockEvent,
};
use uuid::Uuid;

const ORIGIN: &str = "http://127.0.0.1:8080";

struct Fixture {
    runtime: PgPool,
    admin: PgPool,
    migrator: PgPool,
    serial: PgConnection,
    app: Router,
    payments: PaymentConfig,
    tenants: Vec<Uuid>,
}

struct Actor {
    tenant: TenantId,
    user: UserId,
    credentials: Credentials,
}

struct Credentials {
    cookie: String,
    csrf: String,
}

struct Checkout {
    links: Value,
    attempt_id: Uuid,
}

fn db_ok<T>(result: Result<T, sqlx::Error>) -> T {
    result.unwrap_or_else(|error| {
        let state = error
            .as_database_error()
            .and_then(|error| error.code())
            .map_or_else(|| "unavailable".to_owned(), |code| code.into_owned());
        panic!("Tips HTTP test database operation failed (SQLSTATE {state})")
    })
}

async fn pool(variable: &str) -> PgPool {
    let url = std::env::var(variable).unwrap_or_else(|_| panic!("{variable} is required"));
    let pool = db_ok(
        PgPoolOptions::new()
            .max_connections(3)
            .acquire_timeout(Duration::from_secs(3))
            .connect(&url)
            .await,
    );
    assert!(database(&pool).await.ends_with("_test"));
    pool
}

async fn database(pool: &PgPool) -> String {
    db_ok(
        sqlx::query_scalar("SELECT current_database()")
            .fetch_one(pool)
            .await,
    )
}

impl Fixture {
    async fn new() -> Self {
        let runtime = pool("TEST_DATABASE_URL").await;
        let admin = pool("TEST_ADMIN_DATABASE_URL").await;
        let migrator = pool("TEST_MIGRATION_DATABASE_URL").await;
        let name = database(&runtime).await;
        assert_eq!(database(&admin).await, name);
        assert_eq!(database(&migrator).await, name);
        let url = std::env::var("TEST_MIGRATION_DATABASE_URL").unwrap();
        let mut serial = db_ok(PgConnection::connect(&url).await);
        db_ok(
            sqlx::query("SET statement_timeout = '60s'")
                .execute(&mut serial)
                .await,
        );
        db_ok(
            sqlx::query("SELECT pg_advisory_lock(hashtextextended('turnesta.tips_test_scope', 0))")
                .execute(&mut serial)
                .await,
        );
        let payments =
            PaymentConfig::mock(&"13".repeat(32), "http://127.0.0.1:8080/hooks/tips/mock").unwrap();
        let app = router(
            AppState::new(runtime.clone(), Duration::from_secs(1)).with_payments(payments.clone()),
        );
        Self {
            runtime,
            admin,
            migrator,
            serial,
            app,
            payments,
            tenants: Vec::new(),
        }
    }

    async fn actor(&self, tenant: TenantId, user: UserId) -> Actor {
        let email = Email::parse(&format!("tips-http-{}@example.test", Uuid::new_v4())).unwrap();
        login::bind_email(&self.admin, user, &email).await.unwrap();
        let mut code = [0; 32];
        getrandom::fill(&mut code).unwrap();
        let hash: [u8; 32] = Sha256::digest(code).into();
        login::issue_login_code(&self.admin, &email, &hash, 600)
            .await
            .unwrap();
        let response = post(
            &self.app,
            "/auth/login",
            None,
            Some(ORIGIN),
            None,
            Some(json!({"code": hex::encode(code)})),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let credentials = credentials(&response);
        assert_eq!(body(response).await["user_id"], user.to_string());
        Actor {
            tenant,
            user,
            credentials,
        }
    }

    async fn owner(&mut self) -> Actor {
        let organization = Provisioner::new(self.admin.clone())
            .create_organization(
                &Name::parse("Tips HTTP business").unwrap(),
                &Name::parse("Tips HTTP owner").unwrap(),
            )
            .await
            .unwrap();
        self.tenants
            .push(organization.organization.id.to_string().parse().unwrap());
        self.actor(organization.organization.id, organization.owner.id)
            .await
    }

    async fn member(&self, owner: &Actor, role: Role) -> Actor {
        let provisioner = Provisioner::new(self.admin.clone());
        let user = provisioner
            .create_user(&Name::parse("Tips HTTP member").unwrap())
            .await
            .unwrap();
        provisioner
            .add_membership(owner.tenant, user.id, role)
            .await
            .unwrap();
        self.actor(owner.tenant, user.id).await
    }

    async fn location(&self, actor: &Actor) -> Uuid {
        let response = self
            .mutate(
                actor,
                &format!("/api/organizations/{}/locations", actor.tenant),
                Some(json!({"name":"Tips HTTP location", "time_zone":"Europe/Moscow"})),
            )
            .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        uuid(&body(response).await["id"])
    }

    async fn recipient(&self, actor: &Actor, location: Uuid, name: &str) -> Value {
        let response = self
            .mutate(
                actor,
                &format!("/api/organizations/{}/tips/recipients", actor.tenant),
                Some(json!({"display_name":name, "location_id":location})),
            )
            .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        body(response).await
    }

    async fn mutate(&self, actor: &Actor, path: &str, value: Option<Value>) -> Response {
        post(
            &self.app,
            path,
            Some(&actor.credentials),
            Some(ORIGIN),
            Some(&actor.credentials.csrf),
            value,
        )
        .await
    }

    async fn checkout(&self, recipient: &Value, key: Uuid, amount: i64) -> Checkout {
        let response = post(
            &self.app,
            &format!(
                "/api/public/tips/{}/attempts",
                recipient["public_slug"].as_str().unwrap()
            ),
            None,
            Some(ORIGIN),
            None,
            Some(json!({"amount_minor":amount, "idempotency_key":key})),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let links = body(response).await;
        let status_token: Uuid = links["status_url"]
            .as_str()
            .unwrap()
            .strip_prefix("/tips/status/")
            .unwrap()
            .parse()
            .unwrap();
        let attempt_id = db_ok(
            sqlx::query_scalar("SELECT id FROM tips.attempts WHERE status_token = $1")
                .bind(status_token)
                .fetch_one(&self.migrator)
                .await,
        );
        Checkout { links, attempt_id }
    }

    async fn complete(&self, checkout: &Checkout, outcome: &str) -> Response {
        let token = checkout.links["checkout_url"]
            .as_str()
            .unwrap()
            .strip_prefix("/mock-payments/")
            .unwrap();
        post(
            &self.app,
            &format!("/api/mock-payments/{token}/complete"),
            None,
            Some(ORIGIN),
            None,
            Some(json!({"outcome":outcome})),
        )
        .await
    }

    async fn event(&self, checkout: &Checkout) -> MockEvent {
        let Json(event) = db_ok(
            sqlx::query_scalar::<_, Json<MockEvent>>(
                "SELECT event_payload FROM tips.mock_ledger WHERE attempt_id = $1",
            )
            .bind(checkout.attempt_id)
            .fetch_one(&self.migrator)
            .await,
        );
        event
    }

    async fn state(&self, checkout: &Checkout) -> String {
        db_ok(
            sqlx::query_scalar("SELECT status FROM tips.attempts WHERE id=$1")
                .bind(checkout.attempt_id)
                .fetch_one(&self.migrator)
                .await,
        )
    }

    async fn inbox_count(&self, checkout: &Checkout) -> i64 {
        db_ok(
            sqlx::query_scalar("SELECT count(*) FROM tips.webhook_inbox WHERE attempt_id=$1")
                .bind(checkout.attempt_id)
                .fetch_one(&self.migrator)
                .await,
        )
    }

    async fn webhook(
        &self,
        raw: &[u8],
        timestamp: u64,
        signature: &str,
        duplicate: bool,
    ) -> Response {
        let mut request = Request::post("/hooks/tips/mock")
            .header("content-type", "application/json")
            .header("x-turnesta-mock-timestamp", timestamp.to_string())
            .header("x-turnesta-mock-signature", signature);
        if duplicate {
            request = request.header("x-turnesta-mock-signature", signature);
        }
        self.app
            .clone()
            .oneshot(request.body(Body::from(raw.to_vec())).unwrap())
            .await
            .unwrap()
    }

    async fn signed(&self, value: &Value) -> Response {
        let raw = serde_json::to_vec(value).unwrap();
        let timestamp = now();
        self.webhook(
            &raw,
            timestamp,
            &self.payments.sign(timestamp, &raw).unwrap(),
            false,
        )
        .await
    }

    async fn close(self) {
        // Terminalize only events belonging to organizations created by this fixture.
        // Other tests' pending/leased deliveries must remain untouched.
        db_ok(sqlx::query("UPDATE tips.mock_delivery_outbox job SET state='sent', lease_token=NULL, lease_until=NULL WHERE EXISTS (SELECT 1 FROM tips.mock_ledger ledger JOIN tips.attempts attempt ON attempt.id=ledger.attempt_id WHERE ledger.event_id=job.event_id AND attempt.tenant_id=ANY($1))")
            .bind(&self.tenants).execute(&self.migrator).await);
        db_ok(self.serial.close().await);
        self.runtime.close().await;
        self.admin.close().await;
        self.migrator.close().await;
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
fn uuid(value: &Value) -> Uuid {
    value.as_str().unwrap().parse().unwrap()
}

fn credentials(response: &Response) -> Credentials {
    let cookies: Vec<&str> = response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|header| header.to_str().unwrap())
        .collect();
    let session = cookies
        .iter()
        .find(|value| value.starts_with("turnesta_session="))
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let csrf = cookies
        .iter()
        .find(|value| value.starts_with("turnesta_csrf="))
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    Credentials {
        cookie: format!("{session}; {csrf}"),
        csrf: csrf.split_once('=').unwrap().1.to_owned(),
    }
}

async fn body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap()
}
async fn text(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
async fn get(app: &Router, path: &str, credentials: Option<&Credentials>) -> Response {
    let mut request = Request::get(path);
    if let Some(credentials) = credentials {
        request = request.header("cookie", &credentials.cookie);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}
async fn post(
    app: &Router,
    path: &str,
    credentials: Option<&Credentials>,
    origin: Option<&str>,
    csrf: Option<&str>,
    value: Option<Value>,
) -> Response {
    let mut request = Request::post(path);
    if let Some(credentials) = credentials {
        request = request.header("cookie", &credentials.cookie);
    }
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    if let Some(csrf) = csrf {
        request = request.header("x-csrf-token", csrf);
    }
    let payload = if let Some(value) = value {
        request = request.header("content-type", "application/json");
        Body::from(value.to_string())
    } else {
        Body::empty()
    };
    app.clone()
        .oneshot(request.body(payload).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database and runtime/admin/migrator URLs"]
async fn complete_http_checkout_is_idempotent_and_requires_one_verified_server_callback() {
    let mut fixture = Fixture::new().await;
    let owner = fixture.owner().await;
    let location = fixture.location(&owner).await;
    let dangerous = "<script>alert('tip')</script> & Анна";
    let recipient = fixture.recipient(&owner, location, dangerous).await;
    assert_eq!(recipient["environment"], "mock");
    let profile = get(
        &fixture.app,
        &format!("/tips/{}", recipient["public_slug"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(profile.status(), StatusCode::OK);
    let html = text(profile).await;
    assert!(!html.contains(dangerous) && !html.contains("<script>alert"));
    assert!(html.contains("&lt;script") || html.contains("&#60;script"));
    assert!(html.contains("Анна"));

    let key = Uuid::new_v4();
    let checkout = fixture.checkout(&recipient, key, 10005).await;
    let replay = fixture.checkout(&recipient, key, 10005).await;
    assert_eq!(replay.links, checkout.links);
    assert_eq!(replay.attempt_id, checkout.attempt_id);
    let different = post(
        &fixture.app,
        &format!(
            "/api/public/tips/{}/attempts",
            recipient["public_slug"].as_str().unwrap()
        ),
        None,
        Some(ORIGIN),
        None,
        Some(json!({"amount_minor":10006,"idempotency_key":key})),
    )
    .await;
    assert_eq!(different.status(), StatusCode::CONFLICT);
    assert_eq!(body(different).await["code"], "idempotency_conflict");
    let completed = fixture.complete(&checkout, "succeeded").await;
    assert_eq!(completed.status(), StatusCode::OK);
    assert_eq!(
        body(completed).await["status_url"],
        checkout.links["status_url"]
    );
    assert_eq!(fixture.state(&checkout).await, "awaiting_payment");
    let pending = get(
        &fixture.app,
        &format!(
            "/api/public{}",
            checkout.links["status_url"].as_str().unwrap()
        ),
        None,
    )
    .await;
    assert_eq!(body(pending).await["status"], "awaiting_payment");

    let event = serde_json::to_value(fixture.event(&checkout).await).unwrap();
    let accepted = fixture.signed(&event).await;
    assert_eq!(accepted.status(), StatusCode::OK);
    assert_eq!(
        body(accepted).await,
        json!({"accepted":true,"processed":true})
    );
    assert_eq!(fixture.state(&checkout).await, "succeeded");
    let duplicate = fixture.signed(&event).await;
    assert_eq!(duplicate.status(), StatusCode::OK);
    assert_eq!(
        body(duplicate).await,
        json!({"accepted":true,"processed":false})
    );
    assert_eq!(fixture.inbox_count(&checkout).await, 1);
    let attempts = get(
        &fixture.app,
        &format!("/api/organizations/{}/tips/attempts", owner.tenant),
        Some(&owner.credentials),
    )
    .await;
    let attempt = body(attempts).await["items"][0].clone();
    assert_eq!(attempt["amount_minor"], 10005);
    assert_eq!(attempt["currency"], "RUB");
    assert_eq!(attempt["status"], "succeeded");
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database and runtime/admin/migrator URLs"]
async fn webhook_rejects_signature_replay_ambiguity_mutated_facts_and_invalid_payloads() {
    let mut fixture = Fixture::new().await;
    let owner = fixture.owner().await;
    let location = fixture.location(&owner).await;
    let recipient = fixture.recipient(&owner, location, "Webhook checks").await;
    let checkout = fixture.checkout(&recipient, Uuid::new_v4(), 10005).await;
    assert_eq!(
        fixture.complete(&checkout, "succeeded").await.status(),
        StatusCode::OK
    );
    let valid = serde_json::to_value(fixture.event(&checkout).await).unwrap();
    let raw = serde_json::to_vec(&valid).unwrap();
    let timestamp = now();
    let signature = fixture.payments.sign(timestamp, &raw).unwrap();
    assert_eq!(
        fixture
            .webhook(&raw, timestamp, &"00".repeat(32), false)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let stale = timestamp - 1000;
    assert_eq!(
        fixture
            .webhook(
                &raw,
                stale,
                &fixture.payments.sign(stale, &raw).unwrap(),
                false
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .webhook(&raw, timestamp, &signature, true)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let duplicate_timestamp = fixture
        .app
        .clone()
        .oneshot(
            Request::post("/hooks/tips/mock")
                .header("content-type", "application/json")
                .header("x-turnesta-mock-timestamp", timestamp.to_string())
                .header("x-turnesta-mock-timestamp", timestamp.to_string())
                .header("x-turnesta-mock-signature", &signature)
                .body(Body::from(raw.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate_timestamp.status(), StatusCode::FORBIDDEN);

    for (field, replacement) in [
        ("event_id", json!(Uuid::new_v4())),
        ("attempt_id", json!(Uuid::new_v4())),
        ("provider_payment_id", json!(Uuid::new_v4())),
        ("recipient_id", json!(Uuid::new_v4())),
        ("amount_minor", json!(10006)),
        ("currency", json!("USD")),
        ("status", json!("failed")),
    ] {
        let mut changed = valid.clone();
        changed[field] = replacement;
        let rejected = fixture.signed(&changed).await;
        assert_eq!(
            rejected.status(),
            StatusCode::CONFLICT,
            "mutated {field} must be rejected"
        );
        assert_eq!(fixture.state(&checkout).await, "awaiting_payment");
        assert_eq!(fixture.inbox_count(&checkout).await, 0);
    }
    let mut extra = valid.clone();
    extra["unexpected"] = json!(true);
    assert_eq!(
        fixture.signed(&extra).await.status(),
        StatusCode::BAD_REQUEST
    );
    let mut float = valid.clone();
    float["amount_minor"] = json!(10005.5);
    assert_eq!(
        fixture.signed(&float).await.status(),
        StatusCode::BAD_REQUEST
    );
    let oversized = vec![b' '; 8193];
    assert_eq!(
        fixture
            .webhook(
                &oversized,
                timestamp,
                &fixture.payments.sign(timestamp, &oversized).unwrap(),
                false
            )
            .await
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let path = format!(
        "/api/public/tips/{}/attempts",
        recipient["public_slug"].as_str().unwrap()
    );
    for value in [
        json!({"amount_minor":100.05,"idempotency_key":Uuid::new_v4()}),
        json!({"amount_minor":10005,"idempotency_key":Uuid::new_v4(),"status":"succeeded"}),
    ] {
        assert_eq!(
            post(&fixture.app, &path, None, Some(ORIGIN), None, Some(value))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(fixture.state(&checkout).await, "awaiting_payment");
    assert_eq!(fixture.inbox_count(&checkout).await, 0);
    assert_eq!(fixture.signed(&valid).await.status(), StatusCode::OK);
    assert_eq!(fixture.state(&checkout).await, "succeeded");
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database and runtime/admin/migrator URLs"]
async fn management_enforces_session_role_tenant_origin_csrf_and_public_projection_boundaries() {
    let mut fixture = Fixture::new().await;
    let owner = fixture.owner().await;
    let location = fixture.location(&owner).await;
    let recipient = fixture.recipient(&owner, location, "Access checks").await;
    let checkout = fixture.checkout(&recipient, Uuid::new_v4(), 10005).await;
    let recipient_path = format!("/api/organizations/{}/tips/recipients", owner.tenant);
    let attempts_path = format!("/api/organizations/{}/tips/attempts", owner.tenant);
    let reconcile_path = format!("{attempts_path}/{}/reconcile", checkout.attempt_id);
    assert_eq!(
        get(&fixture.app, &recipient_path, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(&fixture.app, &attempts_path, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post(
            &fixture.app,
            &recipient_path,
            None,
            Some(ORIGIN),
            None,
            Some(json!({"display_name":"Anonymous","location_id":location}))
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post(
            &fixture.app,
            &reconcile_path,
            None,
            Some(ORIGIN),
            None,
            None
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    for (origin, csrf) in [
        (None, Some(owner.credentials.csrf.as_str())),
        (
            Some("https://foreign.test"),
            Some(owner.credentials.csrf.as_str()),
        ),
        (Some(ORIGIN), None),
        (Some(ORIGIN), Some("00")),
    ] {
        assert_eq!(
            post(
                &fixture.app,
                &recipient_path,
                Some(&owner.credentials),
                origin,
                csrf,
                Some(json!({"display_name":"Rejected","location_id":location}))
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            post(
                &fixture.app,
                &reconcile_path,
                Some(&owner.credentials),
                origin,
                csrf,
                None
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        fixture
            .mutate(
                &owner,
                &recipient_path,
                Some(
                    json!({"display_name":"Injection","location_id":location,"user_id":owner.user})
                )
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    for role in [Role::Operator, Role::Employee] {
        let member = fixture.member(&owner, role).await;
        assert_eq!(
            get(&fixture.app, &recipient_path, Some(&member.credentials))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            get(&fixture.app, &attempts_path, Some(&member.credentials))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            fixture
                .mutate(
                    &member,
                    &recipient_path,
                    Some(json!({"display_name":"Denied","location_id":location}))
                )
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            fixture
                .mutate(&member, &reconcile_path, None)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let manager = fixture.member(&owner, Role::Manager).await;
    assert_eq!(
        get(&fixture.app, &recipient_path, Some(&manager.credentials))
            .await
            .status(),
        StatusCode::OK
    );
    let _managed = fixture
        .recipient(&manager, location, "Manager recipient")
        .await;
    let first = body(
        get(
            &fixture.app,
            &format!("{recipient_path}?limit=1"),
            Some(&owner.credentials),
        )
        .await,
    )
    .await;
    let next = first["next_cursor"].as_str().unwrap();
    let second = body(
        get(
            &fixture.app,
            &format!("{recipient_path}?limit=1&after={next}"),
            Some(&owner.credentials),
        )
        .await,
    )
    .await;
    assert_ne!(first["items"][0]["id"], second["items"][0]["id"]);
    assert!(second["next_cursor"].is_null());
    assert_eq!(
        get(
            &fixture.app,
            &format!("{attempts_path}?limit=101"),
            Some(&owner.credentials)
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let stranger = fixture.owner().await;
    for path in [&recipient_path, &attempts_path] {
        assert_eq!(
            get(&fixture.app, path, Some(&stranger.credentials))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        fixture
            .mutate(
                &stranger,
                &recipient_path,
                Some(json!({"display_name":"Foreign","location_id":location}))
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .mutate(&stranger, &reconcile_path, None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .mutate(
                &owner,
                &format!(
                    "/api/organizations/{}/tips/attempts/{}/reconcile",
                    stranger.tenant, checkout.attempt_id
                ),
                None
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let public_path = format!(
        "/api/public{}",
        checkout.links["status_url"].as_str().unwrap()
    );
    let public = body(get(&fixture.app, &public_path, None).await).await;
    assert_eq!(public.as_object().unwrap().len(), 5);
    for field in [
        "display_name",
        "amount_minor",
        "currency",
        "status",
        "environment",
    ] {
        assert!(public.get(field).is_some());
    }
    for field in [
        "id",
        "tenant_id",
        "recipient_id",
        "location_id",
        "user_id",
        "provider_payment_id",
        "checkout_token",
        "status_token",
    ] {
        assert!(public.get(field).is_none());
    }
    let create_path = format!(
        "/api/public/tips/{}/attempts",
        recipient["public_slug"].as_str().unwrap()
    );
    for origin in [None, Some("https://foreign.test")] {
        assert_eq!(
            post(
                &fixture.app,
                &create_path,
                None,
                origin,
                None,
                Some(json!({"amount_minor":10005,"idempotency_key":Uuid::new_v4()}))
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(fixture.state(&checkout).await, "awaiting_payment");
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database and runtime/admin/migrator URLs"]
async fn browser_return_cannot_set_success_and_reconciliation_uses_private_provider_fact() {
    let mut fixture = Fixture::new().await;
    let owner = fixture.owner().await;
    let location = fixture.location(&owner).await;
    let recipient = fixture.recipient(&owner, location, "Lost callback").await;
    let checkout = fixture.checkout(&recipient, Uuid::new_v4(), 10005).await;
    let reconcile = format!(
        "/api/organizations/{}/tips/attempts/{}/reconcile",
        owner.tenant, checkout.attempt_id
    );
    let forced = fixture
        .mutate(
            &owner,
            &reconcile,
            Some(json!({"status":"succeeded","outcome":"succeeded"})),
        )
        .await;
    assert_eq!(forced.status(), StatusCode::OK);
    assert_eq!(body(forced).await["status"], "awaiting_payment");
    assert_eq!(
        fixture.complete(&checkout, "failed").await.status(),
        StatusCode::OK
    );
    let browser_url = format!(
        "{}?status=succeeded&outcome=succeeded",
        checkout.links["status_url"].as_str().unwrap()
    );
    let returned = get(&fixture.app, &browser_url, None).await;
    assert_eq!(returned.status(), StatusCode::OK);
    assert!(
        text(returned)
            .await
            .contains("data-status=\"awaiting_payment\"")
    );
    assert_eq!(fixture.state(&checkout).await, "awaiting_payment");
    let public_url = format!(
        "/api/public{}",
        checkout.links["status_url"].as_str().unwrap()
    );
    assert_eq!(
        post(
            &fixture.app,
            &public_url,
            None,
            Some(ORIGIN),
            None,
            Some(json!({"status":"succeeded"}))
        )
        .await
        .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    let reconciled = fixture.mutate(&owner, &reconcile, None).await;
    assert_eq!(reconciled.status(), StatusCode::OK);
    assert_eq!(body(reconciled).await["status"], "failed");
    let second = fixture.mutate(&owner, &reconcile, None).await;
    assert_eq!(body(second).await["status"], "failed");
    assert_eq!(fixture.inbox_count(&checkout).await, 1);
    let event = serde_json::to_value(fixture.event(&checkout).await).unwrap();
    assert_eq!(body(fixture.signed(&event).await).await["processed"], false);
    let mut changed = event;
    changed["status"] = json!("succeeded");
    assert_eq!(
        fixture.signed(&changed).await.status(),
        StatusCode::CONFLICT
    );
    assert_eq!(fixture.state(&checkout).await, "failed");
    fixture.close().await;
}

#[tokio::test]
async fn disabled_public_tips_reject_before_using_even_a_closed_database_pool() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgresql://unused:unused@127.0.0.1:1/turnesta_test")
        .unwrap();
    pool.close().await;
    let app = router(AppState::new(pool, Duration::from_millis(20)));
    let slug = Uuid::new_v4();
    let page = get(&app, &format!("/tips/{slug}"), None).await;
    assert_eq!(page.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body(page).await["code"], "tips_disabled");
    let response = post(
        &app,
        &format!("/api/public/tips/{slug}/attempts"),
        None,
        Some(ORIGIN),
        None,
        Some(json!({"amount_minor":10005,"idempotency_key":Uuid::new_v4()})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body(response).await["code"], "tips_disabled");
    let hook = post(&app, "/hooks/tips/mock", None, None, None, Some(json!({}))).await;
    assert_eq!(hook.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body(hook).await["code"], "tips_disabled");
}
