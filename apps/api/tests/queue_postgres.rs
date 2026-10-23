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
    core::{LocationId, Name, Provisioner, Role, TenantId, TenantSession, UserId},
    http::{AppState, router},
    queue::{self, Command, CommandResult, QueueError, TicketPage, TicketStatus},
};
use uuid::Uuid;

const ORIGIN: &str = "http://127.0.0.1:8080";

struct Fixture {
    runtime: PgPool,
    admin: PgPool,
    tenant: TenantId,
    owner: UserId,
    location: LocationId,
    queue: Uuid,
    app: Router,
}

async fn pool(variable: &str, max: u32) -> PgPool {
    let pool = PgPoolOptions::new()
        .max_connections(max)
        .acquire_timeout(Duration::from_secs(3))
        .connect(&std::env::var(variable).expect("test database URL required"))
        .await
        .unwrap();
    let db: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        db.ends_with("_test"),
        "requires a dedicated *_test database"
    );
    pool
}

impl Fixture {
    async fn new() -> Self {
        let runtime = pool("TEST_DATABASE_URL", 8).await;
        let admin = pool("TEST_ADMIN_DATABASE_URL", 2).await;
        let org = Provisioner::new(admin.clone())
            .create_organization(
                &Name::parse("Queue test").unwrap(),
                &Name::parse("Owner").unwrap(),
            )
            .await
            .unwrap();
        let tenant = org.organization.id;
        let owner = org.owner.id;
        let mut session = TenantSession::begin(&runtime, owner, tenant).await.unwrap();
        let location = session
            .create_location(&Name::parse("Point").unwrap(), "Pacific/Kiritimati")
            .await
            .unwrap();
        session.commit().await.unwrap();
        let queue = queue::create(
            &runtime,
            owner,
            tenant,
            location.id,
            &Name::parse("Service").unwrap(),
        )
        .await
        .unwrap()
        .id;
        let app = router(AppState::new(runtime.clone(), Duration::from_secs(1)));
        Self {
            runtime,
            admin,
            tenant,
            owner,
            location: location.id,
            queue,
            app,
        }
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

    async fn command(
        &self,
        user: UserId,
        key: Uuid,
        command: Command,
    ) -> Result<CommandResult, QueueError> {
        queue::execute(&self.runtime, user, self.tenant, self.queue, key, command).await
    }

    async fn session(&self, user: UserId) -> TenantSession {
        TenantSession::begin(&self.runtime, user, self.tenant)
            .await
            .unwrap()
    }

    async fn tickets(&self, status: Option<TicketStatus>) -> Vec<queue::Ticket> {
        queue::tickets(
            &self.runtime,
            self.owner,
            self.tenant,
            self.queue,
            TicketPage {
                after: None,
                status,
                limit: 100,
            },
        )
        .await
        .unwrap()
    }

    async fn login(&self, user: UserId) -> (String, String) {
        let email = Email::parse(&format!("queue-{}@example.test", Uuid::new_v4())).unwrap();
        login::bind_email(&self.admin, user, &email).await.unwrap();
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes).unwrap();
        let hash: [u8; 32] = Sha256::digest(bytes).into();
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
                    .body(Body::from(json!({"code":hex::encode(bytes)}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let cookies: Vec<&str> = response
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap().split(';').next().unwrap())
            .collect();
        let csrf = cookies
            .iter()
            .find_map(|c| c.strip_prefix("turnesta_csrf="))
            .unwrap()
            .to_owned();
        (cookies.join("; "), csrf)
    }

    fn root(&self) -> String {
        format!("/api/organizations/{}", self.tenant)
    }
    fn queue_path(&self) -> String {
        format!("{}/queues/{}", self.root(), self.queue)
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        auth: Option<&(String, String)>,
        body: Option<Value>,
        key: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        if let Some((cookie, csrf)) = auth {
            request = request
                .header("cookie", cookie)
                .header("origin", ORIGIN)
                .header("x-csrf-token", csrf);
        }
        if let Some(key) = key {
            request = request.header("idempotency-key", key);
        }
        let body = body.map_or_else(Body::empty, |v| Body::from(v.to_string()));
        let response = self
            .app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn close(self) {
        self.runtime.close().await;
        self.admin.close().await;
    }
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn concurrent_issue_and_call_preserve_fifo_and_unique_numbers() {
    let f = Fixture::new().await;
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..12 {
        let (pool, user, tenant, queue) = (f.runtime.clone(), f.owner, f.tenant, f.queue);
        tasks.spawn(async move {
            queue::execute(
                &pool,
                user,
                tenant,
                queue,
                Uuid::new_v4(),
                Command::Issue {},
            )
            .await
            .unwrap()
        });
    }
    let mut positions = Vec::new();
    let mut numbers = Vec::new();
    while let Some(result) = tasks.join_next().await {
        let ticket = result.unwrap().ticket.unwrap();
        positions.push(ticket.position);
        numbers.push(ticket.number);
        let expected_date: String = sqlx::query_scalar(
            "SELECT to_char(($1::text::timestamptz AT TIME ZONE 'Pacific/Kiritimati')::date, 'YYYY-MM-DD')",
        ).bind(&ticket.issued_at).fetch_one(&f.runtime).await.unwrap();
        assert_eq!(ticket.service_date, expected_date);
    }
    positions.sort();
    numbers.sort();
    assert_eq!(positions, (1..=12).collect::<Vec<_>>());
    assert_eq!(numbers, (1..=12).collect::<Vec<_>>());
    let operator = f.member(Role::Operator).await;
    let (first, second) = tokio::join!(
        f.command(f.owner, Uuid::new_v4(), Command::CallNext {}),
        f.command(operator, Uuid::new_v4(), Command::CallNext {}),
    );
    let first = first.unwrap().ticket.unwrap();
    let second = second.unwrap().ticket.unwrap();
    let mut called = vec![first.position, second.position];
    called.sort();
    assert_eq!(called, [1, 2]);
    assert_ne!(first.id, second.id);
    assert_eq!(first.called_by, Some(f.owner));
    assert_eq!(second.called_by, Some(operator));
    assert_eq!(f.tickets(Some(TicketStatus::Waiting)).await.len(), 10);
    assert_eq!(
        queue::get(&f.runtime, f.owner, f.tenant, f.queue)
            .await
            .unwrap()
            .version,
        15
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn idempotency_replays_original_results_and_empty_calls() {
    let f = Fixture::new().await;
    let empty_key = Uuid::new_v4();
    let empty = f
        .command(f.owner, empty_key, Command::CallNext {})
        .await
        .unwrap();
    assert_eq!(empty.version, 1);
    assert!(empty.ticket.is_none());
    let issue_key = Uuid::new_v4();
    let (a, b) = tokio::join!(
        f.command(f.owner, issue_key, Command::Issue {}),
        f.command(f.owner, issue_key, Command::Issue {})
    );
    let issued = a.unwrap();
    assert_eq!(issued, b.unwrap());
    assert_eq!(
        f.command(f.owner, empty_key, Command::CallNext {})
            .await
            .unwrap(),
        empty
    );
    assert!(matches!(
        f.command(f.owner, issue_key, Command::CallNext {}).await,
        Err(QueueError::IdempotencyConflict)
    ));
    let call_key = Uuid::new_v4();
    let called = f
        .command(f.owner, call_key, Command::CallNext {})
        .await
        .unwrap();
    let id = called.ticket.as_ref().unwrap().id;
    let completed = f
        .command(f.owner, Uuid::new_v4(), Command::Complete { ticket_id: id })
        .await
        .unwrap();
    assert_eq!(completed.ticket.unwrap().status, TicketStatus::Completed);
    assert_eq!(
        f.command(f.owner, issue_key, Command::Issue {})
            .await
            .unwrap(),
        issued
    );
    assert_eq!(
        f.command(f.owner, call_key, Command::CallNext {})
            .await
            .unwrap(),
        called
    );
    assert_eq!(f.tickets(None).await.len(), 1);
    assert_eq!(f.tickets(None).await[0].status, TicketStatus::Completed);
    // Keys belong to an actor and queue, not the entire organization.
    let operator = f.member(Role::Operator).await;
    f.command(operator, issue_key, Command::Issue {})
        .await
        .unwrap();
    let other = queue::create(
        &f.runtime,
        f.owner,
        f.tenant,
        f.location,
        &Name::parse("Other queue").unwrap(),
    )
    .await
    .unwrap();
    queue::execute(
        &f.runtime,
        f.owner,
        f.tenant,
        other.id,
        issue_key,
        Command::Issue {},
    )
    .await
    .unwrap();
    assert_eq!(f.tickets(None).await.len(), 2);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn lifecycle_enforces_assignment_and_serializes_terminal_races() {
    let f = Fixture::new().await;
    let operator = f.member(Role::Operator).await;
    let other = f.member(Role::Operator).await;
    let id = f
        .command(operator, Uuid::new_v4(), Command::Issue {})
        .await
        .unwrap()
        .ticket
        .unwrap()
        .id;
    let failed_key = Uuid::new_v4();
    assert!(matches!(
        f.command(operator, failed_key, Command::Complete { ticket_id: id })
            .await,
        Err(QueueError::StateConflict)
    ));
    assert_eq!(
        queue::get(&f.runtime, f.owner, f.tenant, f.queue)
            .await
            .unwrap()
            .version,
        2
    );
    f.command(operator, Uuid::new_v4(), Command::CallNext {})
        .await
        .unwrap();
    assert!(matches!(
        f.command(operator, Uuid::new_v4(), Command::CallNext {})
            .await,
        Err(QueueError::ActiveTicket)
    ));
    assert!(matches!(
        f.command(other, Uuid::new_v4(), Command::Complete { ticket_id: id })
            .await,
        Err(QueueError::Forbidden)
    ));
    assert!(matches!(
        f.command(other, Uuid::new_v4(), Command::Cancel { ticket_id: id })
            .await,
        Err(QueueError::Forbidden)
    ));
    let manager = f.member(Role::Manager).await;
    let (a, b) = tokio::join!(
        f.command(operator, failed_key, Command::Complete { ticket_id: id }),
        f.command(manager, Uuid::new_v4(), Command::Cancel { ticket_id: id }),
    );
    assert!(matches!(
        (&a, &b),
        (Ok(_), Err(QueueError::StateConflict)) | (Err(QueueError::StateConflict), Ok(_))
    ));
    assert_eq!(
        queue::get(&f.runtime, f.owner, f.tenant, f.queue)
            .await
            .unwrap()
            .version,
        4
    );
    let result = f
        .command(operator, Uuid::new_v4(), Command::CallNext {})
        .await
        .unwrap();
    assert!(result.ticket.is_none());
    let waiting = f
        .command(operator, Uuid::new_v4(), Command::Issue {})
        .await
        .unwrap()
        .ticket
        .unwrap()
        .id;
    let key = Uuid::new_v4();
    let cancelled = f
        .command(other, key, Command::Cancel { ticket_id: waiting })
        .await
        .unwrap();
    assert_eq!(
        cancelled.ticket.as_ref().unwrap().status,
        TicketStatus::Cancelled
    );
    assert_eq!(
        f.command(other, key, Command::Cancel { ticket_id: waiting })
            .await
            .unwrap(),
        cancelled
    );
    assert!(matches!(
        f.command(other, key, Command::Cancel { ticket_id: id })
            .await,
        Err(QueueError::IdempotencyConflict)
    ));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn old_day_tickets_remain_first_while_daily_number_restarts() {
    let f = Fixture::new().await;
    let mut tx = f.session(f.owner).await;
    sqlx::query(
        "UPDATE queue.queues SET next_position = 1, version = 2 WHERE tenant_id = $1 AND id = $2",
    )
    .bind(f.tenant)
    .bind(f.queue)
    .execute(tx.connection())
    .await
    .unwrap();
    let old = Uuid::new_v4();
    sqlx::query("INSERT INTO queue.tickets (tenant_id, queue_id, id, position, service_date, number, issued_at) VALUES ($1, $2, $3, 1, (clock_timestamp() AT TIME ZONE 'Pacific/Kiritimati')::date - 3, 1, clock_timestamp() - interval '3 days')")
        .bind(f.tenant).bind(f.queue).bind(old).execute(tx.connection()).await.unwrap();
    sqlx::query("INSERT INTO queue.daily_counters (tenant_id, queue_id, service_date, last_number) SELECT tenant_id, queue_id, service_date, 1 FROM queue.tickets WHERE tenant_id = $1 AND queue_id = $2")
        .bind(f.tenant).bind(f.queue).execute(tx.connection()).await.unwrap();
    tx.commit().await.unwrap();
    let new = f
        .command(f.owner, Uuid::new_v4(), Command::Issue {})
        .await
        .unwrap()
        .ticket
        .unwrap();
    assert_eq!(new.number, 1);
    assert_eq!(new.position, 2);
    let called = f
        .command(f.owner, Uuid::new_v4(), Command::CallNext {})
        .await
        .unwrap()
        .ticket
        .unwrap();
    assert_eq!(called.id, old);
    assert_ne!(called.service_date, new.service_date);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn rls_roles_and_composite_references_isolate_queue_data() {
    let f = Fixture::new().await;
    let other = Fixture::new().await;
    let employee = f.member(Role::Employee).await;
    let operator = f.member(Role::Operator).await;
    let key = Uuid::new_v4();
    f.command(f.owner, key, Command::Issue {}).await.unwrap();
    other
        .command(other.owner, Uuid::new_v4(), Command::Issue {})
        .await
        .unwrap();
    assert_eq!(
        queue::list(&f.runtime, employee, f.tenant, f.location, None, 50)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(matches!(
        f.command(employee, key, Command::Issue {}).await,
        Err(QueueError::Forbidden)
    ));
    assert!(matches!(
        queue::create(
            &f.runtime,
            operator,
            f.tenant,
            f.location,
            &Name::parse("Denied").unwrap()
        )
        .await,
        Err(QueueError::Forbidden)
    ));
    assert!(matches!(
        queue::get(&f.runtime, f.owner, f.tenant, other.queue).await,
        Err(QueueError::NotFound)
    ));
    assert!(matches!(
        queue::get(&f.runtime, f.owner, other.tenant, other.queue).await,
        Err(QueueError::Core(_))
    ));
    assert!(matches!(
        queue::create(
            &f.runtime,
            f.owner,
            f.tenant,
            other.location,
            &Name::parse("Wrong point").unwrap()
        )
        .await,
        Err(QueueError::NotFound)
    ));
    let mut tx = f.session(employee).await;
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM queue.ticket_details")
        .fetch_all(tx.connection())
        .await
        .unwrap();
    assert_eq!(ids.len(), 1);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM queue.commands")
        .fetch_one(tx.connection())
        .await
        .unwrap();
    assert_eq!(count, 0); // Actor-scoped command results, even inside one tenant.
    let error = sqlx::query(
        "INSERT INTO queue.queues (tenant_id, id, location_id, name) VALUES ($1, $2, $3, 'Denied')",
    )
    .bind(f.tenant)
    .bind(Uuid::new_v4())
    .bind(f.location)
    .execute(tx.connection())
    .await
    .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    drop(tx);
    let mut tx = f.session(f.owner).await;
    let error = sqlx::query("INSERT INTO queue.queues (tenant_id, id, location_id, name) VALUES ($1, $2, $3, 'Wrong tenant point')")
        .bind(f.tenant).bind(Uuid::new_v4()).bind(other.location).execute(tx.connection()).await.unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23503")
    );
    drop(tx);
    f.close().await;
    other.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn queue_lock_timeout_rolls_back_and_other_queues_continue() {
    let f = Fixture::new().await;
    let other = queue::create(
        &f.runtime,
        f.owner,
        f.tenant,
        f.location,
        &Name::parse("Independent").unwrap(),
    )
    .await
    .unwrap();
    let mut lock = f.session(f.owner).await;
    sqlx::query("SELECT id FROM queue.queues WHERE tenant_id = $1 AND id = $2 FOR UPDATE")
        .bind(f.tenant)
        .bind(f.queue)
        .execute(lock.connection())
        .await
        .unwrap();
    let key = Uuid::new_v4();
    let (blocked, independent) = tokio::join!(
        f.command(f.owner, key, Command::Issue {}),
        tokio::time::timeout(
            Duration::from_secs(1),
            queue::execute(
                &f.runtime,
                f.owner,
                f.tenant,
                other.id,
                Uuid::new_v4(),
                Command::Issue {}
            )
        ),
    );
    let error = blocked.unwrap_err();
    assert!(
        matches!(error, QueueError::Database(ref e) if e.as_database_error().unwrap().code().as_deref() == Some("55P03"))
    );
    independent.unwrap().unwrap();
    lock.commit().await.unwrap();
    assert!(f.tickets(None).await.is_empty());
    let ticket = f
        .command(f.owner, key, Command::Issue {})
        .await
        .unwrap()
        .ticket
        .unwrap();
    assert_eq!(ticket.number, 1);
    assert_eq!(ticket.position, 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn http_commands_require_auth_csrf_strict_payload_and_idempotency_key() {
    let f = Fixture::new().await;
    let path = format!("{}/commands", f.queue_path());
    let command = json!({"type":"issue"});
    let key = Uuid::new_v4().to_string();
    assert_eq!(
        f.request("POST", &path, None, Some(command.clone()), Some(&key))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let auth = f.login(f.owner).await;
    let bad_csrf = (auth.0.clone(), "wrong".into());
    assert_eq!(
        f.request(
            "POST",
            &path,
            Some(&bad_csrf),
            Some(command.clone()),
            Some(&key)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    for value in [None, Some("invalid")] {
        assert_eq!(
            f.request("POST", &path, Some(&auth), Some(command.clone()), value)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    for body in [
        json!({"type":"complete"}),
        json!({"type":"issue", "user_id":f.owner}),
        json!({"type":"unknown"}),
    ] {
        assert_eq!(
            f.request("POST", &path, Some(&auth), Some(body), Some(&key))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    let first = f
        .request(
            "POST",
            &path,
            Some(&auth),
            Some(command.clone()),
            Some(&key),
        )
        .await;
    assert_eq!(first.0, StatusCode::OK);
    assert_eq!(first.1["ticket"]["number"], 1);
    assert_eq!(
        f.request("POST", &path, Some(&auth), Some(command), Some(&key))
            .await,
        first
    );
    let conflict = f
        .request(
            "POST",
            &path,
            Some(&auth),
            Some(json!({"type":"call_next"})),
            Some(&key),
        )
        .await;
    assert_eq!(conflict.0, StatusCode::CONFLICT);
    assert_eq!(conflict.1["code"], "idempotency_conflict");
    let foreign_origin = f
        .app
        .clone()
        .oneshot(
            Request::post(&path)
                .header("cookie", &auth.0)
                .header("origin", "https://foreign.test")
                .header("x-csrf-token", &auth.1)
                .header("content-type", "application/json")
                .header("idempotency-key", Uuid::new_v4().to_string())
                .body(Body::from(json!({"type":"issue"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(foreign_origin.status(), StatusCode::FORBIDDEN);
    let duplicate_key = f
        .app
        .clone()
        .oneshot(
            Request::post(&path)
                .header("cookie", &auth.0)
                .header("origin", ORIGIN)
                .header("x-csrf-token", &auth.1)
                .header("content-type", "application/json")
                .header("idempotency-key", &key)
                .header("idempotency-key", &key)
                .body(Body::from(json!({"type":"issue"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate_key.status(), StatusCode::BAD_REQUEST);
    assert_eq!(f.tickets(None).await.len(), 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn http_setup_pagination_roles_and_openapi_match_contract() {
    let f = Fixture::new().await;
    let auth = f.login(f.owner).await;
    let create_path = format!("{}/locations/{}/queues", f.root(), f.location);
    let (status, created) = f
        .request(
            "POST",
            &create_path,
            Some(&auth),
            Some(json!({"name":"  Second  "})),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["name"], "Second");
    let employee = f.member(Role::Employee).await;
    let read_auth = f.login(employee).await;
    assert_eq!(
        f.request(
            "POST",
            &create_path,
            Some(&read_auth),
            Some(json!({"name":"Denied"})),
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.request(
            "POST",
            &format!("{}/commands", f.queue_path()),
            Some(&read_auth),
            Some(json!({"type":"issue"})),
            Some(&Uuid::new_v4().to_string())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let first = f
        .request(
            "GET",
            &format!("{create_path}?limit=1"),
            Some(&read_auth),
            None,
            None,
        )
        .await;
    assert_eq!(first.0, StatusCode::OK);
    assert_eq!(first.1["items"].as_array().unwrap().len(), 1);
    let second = f
        .request(
            "GET",
            &format!(
                "{create_path}?limit=1&after={}",
                first.1["next_cursor"].as_str().unwrap()
            ),
            Some(&read_auth),
            None,
            None,
        )
        .await;
    assert_eq!(second.1["items"].as_array().unwrap().len(), 1);
    assert!(second.1["next_cursor"].is_null());
    assert_ne!(first.1["items"][0]["id"], second.1["items"][0]["id"]);
    for _ in 0..3 {
        f.command(f.owner, Uuid::new_v4(), Command::Issue {})
            .await
            .unwrap();
    }
    let tickets = format!("{}/tickets", f.queue_path());
    let first = f
        .request(
            "GET",
            &format!("{tickets}?limit=2&status=waiting"),
            Some(&read_auth),
            None,
            None,
        )
        .await;
    assert_eq!(first.1["next_cursor"], 2);
    let second = f
        .request(
            "GET",
            &format!("{tickets}?after=2&limit=2"),
            Some(&read_auth),
            None,
            None,
        )
        .await;
    assert_eq!(second.1["items"][0]["position"], 3);
    assert!(second.1["next_cursor"].is_null());
    for query in [
        "limit=0",
        "limit=101",
        "after=-1",
        "status=invalid",
        "actor_id=bad",
    ] {
        assert_eq!(
            f.request(
                "GET",
                &format!("{tickets}?{query}"),
                Some(&auth),
                None,
                None
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        f.request(
            "GET",
            &format!("{}/queues/{}", f.root(), Uuid::new_v4()),
            Some(&auth),
            None,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (_, schema) = f.request("GET", "/openapi.json", None, None, None).await;
    let command =
        &schema["paths"]["/api/organizations/{tenant_id}/queues/{queue_id}/commands"]["post"];
    assert!(command["security"][0].get("sessionCookie").is_some());
    assert!(
        command["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "Idempotency-Key" && p["required"] == true)
    );
    f.close().await;
}
