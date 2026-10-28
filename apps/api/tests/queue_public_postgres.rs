use std::time::Duration;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header::SET_COOKIE},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use turnesta_api::{
    auth::login::{self, Email},
    core::{Name, Provisioner, Role, TenantId, TenantSession, UserId},
    http::{AppState, router},
    queue::{self, Command, TicketStatus},
    queue_public::{self as public, PublicQueueError},
};
use uuid::Uuid;

const ORIGIN: &str = "http://127.0.0.1:8080";

async fn pool(variable: &str, connections: u32) -> PgPool {
    let pool = PgPoolOptions::new()
        .max_connections(connections)
        .acquire_timeout(Duration::from_secs(3))
        .connect(&std::env::var(variable).expect("test database URL required"))
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

fn token() -> (String, [u8; 32]) {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).unwrap();
    (hex::encode(bytes), Sha256::digest(bytes).into())
}

struct Fixture {
    runtime: PgPool,
    admin: PgPool,
    migrator: PgPool,
    tenant: TenantId,
    owner: UserId,
    queue: Uuid,
    app: Router,
}
impl Fixture {
    async fn new() -> Self {
        let runtime = pool("TEST_DATABASE_URL", 5).await;
        let admin = pool("TEST_ADMIN_DATABASE_URL", 2).await;
        let migrator = pool("TEST_MIGRATION_DATABASE_URL", 1).await;
        let org = Provisioner::new(admin.clone())
            .create_organization(
                &Name::parse("Public test").unwrap(),
                &Name::parse("Owner").unwrap(),
            )
            .await
            .unwrap();
        let tenant = org.organization.id;
        let owner = org.owner.id;
        let mut tx = TenantSession::begin(&runtime, owner, tenant).await.unwrap();
        let point = tx
            .create_location(&Name::parse("Point").unwrap(), "Pacific/Kiritimati")
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let queue = queue::create(
            &runtime,
            owner,
            tenant,
            point.id,
            &Name::parse("Очередь <script>alert(1)</script>").unwrap(),
        )
        .await
        .unwrap()
        .id;
        let app = router(AppState::new(runtime.clone(), Duration::from_secs(1)));
        Self {
            runtime,
            admin,
            migrator,
            tenant,
            owner,
            queue,
            app,
        }
    }
    async fn publish(&self) -> Uuid {
        public::configure(&self.runtime, self.owner, self.tenant, self.queue, true, 0)
            .await
            .unwrap()
            .public_slug
            .unwrap()
    }
    async fn member(&self, role: Role) -> UserId {
        let provisioner = Provisioner::new(self.admin.clone());
        let user = provisioner
            .create_user(&Name::parse("Member").unwrap())
            .await
            .unwrap();
        provisioner
            .add_membership(self.tenant, user.id, role)
            .await
            .unwrap();
        user.id
    }
    async fn staff(&self, command: Command) -> queue::CommandResult {
        queue::execute(
            &self.runtime,
            self.owner,
            self.tenant,
            self.queue,
            Uuid::new_v4(),
            command,
        )
        .await
        .unwrap()
    }
    async fn login(&self, user: UserId) -> (String, String) {
        let email = Email::parse(&format!("guest-{}@example.test", Uuid::new_v4())).unwrap();
        login::bind_email(&self.admin, user, &email).await.unwrap();
        let (code, hash) = token();
        login::issue_login_code(&self.admin, &email, &hash, 600)
            .await
            .unwrap();
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
        let cookies: Vec<&str> = response
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .map(|c| c.to_str().unwrap().split(';').next().unwrap())
            .collect();
        let csrf = cookies
            .iter()
            .find_map(|c| c.strip_prefix("turnesta_csrf="))
            .unwrap()
            .to_owned();
        (cookies.join("; "), csrf)
    }
    fn settings_url(&self) -> String {
        format!(
            "/api/organizations/{}/queues/{}/public-access",
            self.tenant, self.queue
        )
    }
    async fn json(&self, request: Request<Body>) -> (StatusCode, Value) {
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }
    async fn close(self) {
        self.runtime.close().await;
        self.admin.close().await;
        self.migrator.close().await;
    }
}

fn guest_request(slug: Uuid, secret: &str, method: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(format!("/api/public/queues/{slug}/ticket"))
        .header("origin", ORIGIN)
        .header("x-queue-ticket", secret)
        .header("content-type", "application/json")
        .body(if method == "POST" {
            Body::from("{}")
        } else {
            Body::empty()
        })
        .unwrap()
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn publication_requires_manager_and_keeps_optimistic_versions_and_tenant_isolation() {
    let f = Fixture::new().await;
    let initial = public::settings(&f.runtime, f.owner, f.tenant, f.queue)
        .await
        .unwrap();
    assert!(initial.public_slug.is_none());
    assert_eq!(initial.version, 0);
    for role in [Role::Operator, Role::Employee] {
        let user = f.member(role).await;
        assert!(matches!(
            public::configure(&f.runtime, user, f.tenant, f.queue, true, 0).await,
            Err(PublicQueueError::Core(_))
        ));
        let mut tx = TenantSession::begin(&f.runtime, user, f.tenant)
            .await
            .unwrap();
        let error = sqlx::query("INSERT INTO queue.public_settings (tenant_id, queue_id, public_slug, join_open) VALUES ($1,$2,$3,true)")
            .bind(f.tenant).bind(f.queue).bind(Uuid::new_v4()).execute(tx.connection()).await.unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("42501")
        );
    }
    let slug = f.publish().await;
    let manager = f.member(Role::Manager).await;
    let closed = public::configure(&f.runtime, manager, f.tenant, f.queue, false, 1)
        .await
        .unwrap();
    assert_eq!(closed.public_slug, Some(slug));
    assert_eq!(closed.version, 2);
    assert!(matches!(
        public::configure(&f.runtime, f.owner, f.tenant, f.queue, true, 1).await,
        Err(PublicQueueError::Core(_))
    ));
    assert!(matches!(
        public::settings(&f.runtime, f.owner, f.tenant, Uuid::new_v4()).await,
        Err(PublicQueueError::NotFound)
    ));
    let outsider = Provisioner::new(f.admin.clone())
        .create_user(&Name::parse("Outsider").unwrap())
        .await
        .unwrap();
    assert!(matches!(
        public::settings(&f.runtime, outsider.id, f.tenant, f.queue).await,
        Err(PublicQueueError::Core(_))
    ));
    assert!(!public::profile(&f.runtime, slug).await.unwrap().join_open);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn guest_replay_and_staff_share_fifo_numbers_and_current_private_ticket_state() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let staff = f.staff(Command::Issue {}).await.ticket.unwrap();
    let (_, first) = token();
    let (_, second) = token();
    let (a, b) = tokio::join!(
        public::issue(&f.runtime, slug, &first),
        public::issue(&f.runtime, slug, &first)
    );
    let a = a.unwrap();
    assert_eq!(a, b.unwrap());
    assert_eq!(a.number, 2);
    assert_eq!(a.people_ahead, Some(1));
    let next = public::issue(&f.runtime, slug, &second).await.unwrap();
    assert_eq!(next.number, 3);
    assert_eq!(next.people_ahead, Some(2));
    let local_date: String = sqlx::query_scalar("SELECT to_char(($1::text::timestamptz AT TIME ZONE 'Pacific/Kiritimati')::date, 'YYYY-MM-DD')")
        .bind(&a.issued_at).fetch_one(&f.runtime).await.unwrap();
    assert_eq!(a.service_date, local_date);
    let called = f.staff(Command::CallNext {}).await.ticket.unwrap();
    assert_eq!(called.id, staff.id);
    assert_eq!(
        public::ticket(&f.runtime, slug, &first)
            .await
            .unwrap()
            .people_ahead,
        Some(0)
    );
    f.staff(Command::Complete {
        ticket_id: called.id,
    })
    .await;
    let called = f.staff(Command::CallNext {}).await.ticket.unwrap();
    assert_eq!(called.number, 2);
    let actual = public::issue(&f.runtime, slug, &first).await.unwrap();
    assert_eq!(actual.status, TicketStatus::Called);
    assert_eq!(actual.people_ahead, None);
    assert_eq!(actual.number, a.number);
    f.staff(Command::Complete {
        ticket_id: called.id,
    })
    .await;
    assert_eq!(
        public::ticket(&f.runtime, slug, &first)
            .await
            .unwrap()
            .status,
        TicketStatus::Completed
    );
    assert_eq!(
        public::ticket(&f.runtime, slug, &second)
            .await
            .unwrap()
            .people_ahead,
        Some(0)
    );
    assert_eq!(
        queue::get(&f.runtime, f.owner, f.tenant, f.queue)
            .await
            .unwrap()
            .version,
        8
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn closing_serializes_with_public_issuance_but_does_not_revoke_existing_ticket() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let (_, existing) = token();
    public::issue(&f.runtime, slug, &existing).await.unwrap();
    let mut tx = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM queue.queues WHERE tenant_id=$1 AND id=$2 FOR UPDATE")
        .bind(f.tenant)
        .bind(f.queue)
        .execute(tx.connection())
        .await
        .unwrap();
    sqlx::query("UPDATE queue.public_settings SET join_open=false, version=version+1 WHERE tenant_id=$1 AND queue_id=$2")
        .bind(f.tenant).bind(f.queue).execute(tx.connection()).await.unwrap();
    let guest_pool = pool("TEST_DATABASE_URL", 1).await;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&guest_pool)
        .await
        .unwrap();
    let (_, fresh) = token();
    let blocked_pool = guest_pool.clone();
    let request = tokio::spawn(async move { public::issue(&blocked_pool, slug, &fresh).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT coalesce(wait_event_type='Lock', false) FROM pg_stat_activity WHERE pid=$1",
            )
            .bind(pid)
            .fetch_one(&f.runtime)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("public issuance must wait on the queue lock");
    tx.commit().await.unwrap();
    assert!(matches!(
        request.await.unwrap(),
        Err(PublicQueueError::Closed)
    ));
    assert_eq!(
        public::issue(&f.runtime, slug, &existing)
            .await
            .unwrap()
            .number,
        1
    );
    assert_eq!(
        public::ticket(&f.runtime, slug, &existing)
            .await
            .unwrap()
            .status,
        TicketStatus::Waiting
    );
    assert!(!public::profile(&f.runtime, slug).await.unwrap().join_open);
    guest_pool.close().await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn public_budget_is_serialized_bounded_and_exempts_existing_credentials() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let (_, first) = token();
    public::issue(&f.runtime, slug, &first).await.unwrap();
    for _ in 0..28 {
        public::issue(&f.runtime, slug, &token().1).await.unwrap();
    }
    let (_, a) = token();
    let (_, b) = token();
    let (a, b) = tokio::join!(
        public::issue(&f.runtime, slug, &a),
        public::issue(&f.runtime, slug, &b)
    );
    assert!(matches!(
        (&a, &b),
        (Ok(_), Err(PublicQueueError::RateLimit)) | (Err(PublicQueueError::RateLimit), Ok(_))
    ));
    assert_eq!(
        public::issue(&f.runtime, slug, &first)
            .await
            .unwrap()
            .number,
        1
    );
    assert_eq!(
        queue::get(&f.runtime, f.owner, f.tenant, f.queue)
            .await
            .unwrap()
            .version,
        31
    );
    sqlx::query("UPDATE queue.guest_claims SET created_at=clock_timestamp()-interval '61 seconds' WHERE tenant_id=$1 AND queue_id=$2")
        .bind(f.tenant).bind(f.queue).execute(&f.migrator).await.unwrap();
    assert_eq!(
        public::issue(&f.runtime, slug, &token().1)
            .await
            .unwrap()
            .number,
        31
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn full_waiting_queue_rejects_new_public_ticket_without_consuming_number() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let mut tx = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    sqlx::query("INSERT INTO queue.tickets (tenant_id,queue_id,id,position,service_date,number,issued_at) SELECT $1,$2,gen_random_uuid(),n,current_date,n,clock_timestamp() FROM generate_series(1,500) n")
        .bind(f.tenant).bind(f.queue).execute(tx.connection()).await.unwrap();
    sqlx::query("UPDATE queue.queues SET next_position=500 WHERE tenant_id=$1 AND id=$2")
        .bind(f.tenant)
        .bind(f.queue)
        .execute(tx.connection())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(matches!(
        public::issue(&f.runtime, slug, &token().1).await,
        Err(PublicQueueError::Capacity)
    ));
    assert_eq!(
        queue::get(&f.runtime, f.owner, f.tenant, f.queue)
            .await
            .unwrap()
            .version,
        1
    );
    let claims: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM queue.guest_claims WHERE tenant_id=$1 AND queue_id=$2",
    )
    .bind(f.tenant)
    .bind(f.queue)
    .fetch_one(&f.migrator)
    .await
    .unwrap();
    assert_eq!(claims, 0);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn active_access_outlives_age_but_terminal_expiry_never_reuses_a_credential() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let (_, hash) = token();
    let first = public::issue(&f.runtime, slug, &hash).await.unwrap();
    sqlx::query("UPDATE queue.tickets SET issued_at=clock_timestamp()-interval '8 days' WHERE tenant_id=$1 AND queue_id=$2")
        .bind(f.tenant).bind(f.queue).execute(&f.migrator).await.unwrap();
    assert_eq!(
        public::ticket(&f.runtime, slug, &hash)
            .await
            .unwrap()
            .number,
        first.number
    );
    let called = f.staff(Command::CallNext {}).await.ticket.unwrap();
    f.staff(Command::Complete {
        ticket_id: called.id,
    })
    .await;
    assert_eq!(
        public::ticket(&f.runtime, slug, &hash)
            .await
            .unwrap()
            .status,
        TicketStatus::Completed
    );
    sqlx::query("UPDATE queue.tickets SET closed_at=clock_timestamp()-interval '25 hours' WHERE tenant_id=$1 AND queue_id=$2")
        .bind(f.tenant).bind(f.queue).execute(&f.migrator).await.unwrap();
    assert!(matches!(
        public::ticket(&f.runtime, slug, &hash).await,
        Err(PublicQueueError::Expired)
    ));
    assert!(matches!(
        public::issue(&f.runtime, slug, &hash).await,
        Err(PublicQueueError::Expired)
    ));
    assert_eq!(
        public::issue(&f.runtime, slug, &token().1)
            .await
            .unwrap()
            .number,
        2
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn private_grants_and_narrow_functions_work_without_a_tenant_context() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let (_, hash) = token();
    public::issue(&f.runtime, slug, &hash).await.unwrap();
    let denied = sqlx::query("SELECT * FROM queue.guest_claims")
        .fetch_all(&f.runtime)
        .await
        .unwrap_err();
    assert_eq!(
        denied.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    let hidden: i64 = sqlx::query_scalar("SELECT count(*) FROM queue.tickets")
        .fetch_one(&f.runtime)
        .await
        .unwrap();
    assert_eq!(hidden, 0);
    let grants_safe: bool = sqlx::query_scalar("SELECT NOT EXISTS (SELECT 1 FROM pg_proc p CROSS JOIN LATERAL aclexplode(p.proacl) a WHERE p.pronamespace='queue'::regnamespace AND p.proname IN ('public_profile','public_ticket','public_issue','allocate_ticket') AND a.grantee=0 AND a.privilege_type='EXECUTE')")
        .fetch_one(&f.runtime).await.unwrap();
    assert!(grants_safe);
    let functions_safe: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_proc WHERE pronamespace='queue'::regnamespace AND proname IN ('public_profile','public_ticket','public_issue') AND prosecdef AND 'search_path=pg_catalog'=ANY(proconfig)")
        .fetch_one(&f.runtime).await.unwrap();
    assert_eq!(functions_safe, 3);
    let employee = f.member(Role::Employee).await;
    let mut tx = TenantSession::begin(&f.runtime, employee, f.tenant)
        .await
        .unwrap();
    assert!(
        sqlx::query_scalar::<_, Uuid>("SELECT queue.allocate_ticket($1,$2)")
            .bind(f.tenant)
            .bind(f.queue)
            .fetch_one(tx.connection())
            .await
            .is_err()
    );
    drop(tx);
    assert!(matches!(
        public::ticket(&f.runtime, slug, &token().1).await,
        Err(PublicQueueError::NotFound)
    ));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn anonymous_http_requires_capability_origin_and_strict_body_without_leaking_private_fields()
{
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let (secret, _) = token();
    let base = format!("/api/public/queues/{slug}");
    let response = f
        .app
        .clone()
        .oneshot(
            Request::get(format!("{base}/ticket"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(!response.headers().contains_key("location"));
    for bad in ["short", &"A".repeat(64)] {
        assert_eq!(
            f.json(guest_request(slug, bad, "GET")).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut foreign = guest_request(slug, &secret, "POST");
    foreign
        .headers_mut()
        .insert("origin", "https://foreign.test".parse().unwrap());
    assert_eq!(f.json(foreign).await.0, StatusCode::FORBIDDEN);
    let mut missing_origin = guest_request(slug, &secret, "POST");
    missing_origin.headers_mut().remove("origin");
    assert_eq!(f.json(missing_origin).await.0, StatusCode::FORBIDDEN);
    let mut extra = guest_request(slug, &secret, "POST");
    *extra.body_mut() = Body::from(json!({"called_by":f.owner}).to_string());
    assert_eq!(f.json(extra).await.0, StatusCode::BAD_REQUEST);
    let mut duplicated = guest_request(slug, &secret, "POST");
    duplicated
        .headers_mut()
        .append("x-queue-ticket", secret.parse().unwrap());
    assert_eq!(f.json(duplicated).await.0, StatusCode::BAD_REQUEST);
    let (status, created) = f.json(guest_request(slug, &secret, "POST")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["number"], 1);
    let mut fields: Vec<_> = created
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort();
    assert_eq!(
        fields,
        [
            "issued_at",
            "number",
            "people_ahead",
            "service_date",
            "status"
        ]
    );
    assert!(!created.to_string().contains(&secret));
    assert_eq!(f.json(guest_request(slug, &secret, "GET")).await.1, created);
    assert_eq!(
        f.json(guest_request(Uuid::new_v4(), &secret, "GET"))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.json(guest_request(slug, &token().0, "GET")).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, profile) = f
        .json(Request::get(&base).body(Body::empty()).unwrap())
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(profile.get("tenant_id").is_none());
    assert!(profile.get("called_by").is_none());
    let page = f
        .app
        .clone()
        .oneshot(
            Request::get(format!("/q/{slug}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()["cache-control"], "no-store");
    assert_eq!(page.headers()["referrer-policy"], "no-referrer");
    assert!(!page.headers().contains_key("access-control-allow-origin"));
    let html = String::from_utf8(
        to_bytes(page.into_body(), 64 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("alert(1)"));
    assert!(!html.contains("<script>alert(1)"));
    assert!(!html.contains(&secret));
    let staff_path = format!("/api/organizations/{}/queues/{}/tickets", f.tenant, f.queue);
    assert_eq!(
        f.json(
            Request::get(staff_path)
                .header("x-queue-ticket", &secret)
                .body(Body::empty())
                .unwrap()
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn protected_settings_http_and_qr_expose_only_the_public_queue_url() {
    let f = Fixture::new().await;
    let auth = f.login(f.owner).await;
    let path = f.settings_url();
    assert_eq!(
        f.json(Request::get(&path).body(Body::empty()).unwrap())
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let request = Request::put(&path)
        .header("cookie", &auth.0)
        .header("origin", ORIGIN)
        .header("x-csrf-token", &auth.1)
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"join_open":true,"expected_version":0}).to_string(),
        ))
        .unwrap();
    let (status, result) = f.json(request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["version"], 1);
    let profile_url = result["public_url"].as_str().unwrap();
    assert!(profile_url.starts_with(&format!("{ORIGIN}/q/")));
    assert!(!profile_url.contains(&f.tenant.to_string()));
    assert!(!profile_url.contains(&f.queue.to_string()));
    let qr_path = result["qr_url"].as_str().unwrap();
    let qr = f
        .app
        .clone()
        .oneshot(
            Request::get(qr_path)
                .header("cookie", &auth.0)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(qr.status(), StatusCode::OK);
    assert_eq!(qr.headers()["content-type"], "image/svg+xml; charset=utf-8");
    assert!(
        qr.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("img-src 'self'")
    );
    let svg =
        String::from_utf8(to_bytes(qr.into_body(), 64 * 1024).await.unwrap().to_vec()).unwrap();
    assert!(svg.contains("<svg"));
    assert!(svg.contains("<path"));
    let operator = f.member(Role::Operator).await;
    let operator_auth = f.login(operator).await;
    let denied = Request::put(&path)
        .header("cookie", &operator_auth.0)
        .header("origin", ORIGIN)
        .header("x-csrf-token", &operator_auth.1)
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"join_open":false,"expected_version":1}).to_string(),
        ))
        .unwrap();
    assert_eq!(f.json(denied).await.0, StatusCode::FORBIDDEN);
    let (_, document) = f
        .json(Request::get("/openapi.json").body(Body::empty()).unwrap())
        .await;
    assert!(
        document["paths"]["/api/public/queues/{slug}/ticket"]["get"]["security"][0]
            .get("queueTicket")
            .is_some()
    );
    assert_eq!(
        document["components"]["securitySchemes"]["queueTicket"]["name"],
        "X-Queue-Ticket"
    );
    f.close().await;
}
