use std::{sync::Arc, time::Duration};

use sqlx::{Connection, PgConnection, PgPool, postgres::PgPoolOptions, types::Json};
use tokio::sync::Barrier;
use turnesta_api::{
    core::{Name, ProvisionedOrganization, Provisioner, Role, TenantSession},
    tips::{self, Checkout, MockEvent, Recipient, TipsError},
};
use uuid::Uuid;

struct Fixture {
    runtime: PgPool,
    admin: PgPool,
    owner: PgPool,
    worker: PgPool,
    serial: PgConnection,
}
struct Business {
    organization: ProvisionedOrganization,
    recipient: Recipient,
}

impl Fixture {
    async fn new() -> Self {
        let runtime = connect("TEST_DATABASE_URL").await;
        let admin = connect("TEST_ADMIN_DATABASE_URL").await;
        let owner = connect("TEST_MIGRATION_DATABASE_URL").await;
        let worker = connect("TEST_WORKER_DATABASE_URL").await;
        let names = [
            database(&runtime).await,
            database(&admin).await,
            database(&owner).await,
            database(&worker).await,
        ];
        assert!(
            names.iter().all(|name| name == &names[0]),
            "Tips roles must use the same test database"
        );
        let url =
            std::env::var("TEST_MIGRATION_DATABASE_URL").expect("migration test URL is required");
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
        Self {
            runtime,
            admin,
            owner,
            worker,
            serial,
        }
    }

    async fn business(&self) -> Business {
        let organization = Provisioner::new(self.admin.clone())
            .create_organization(&name("Mock business"), &name("Mock owner"))
            .await
            .unwrap();
        let mut session = TenantSession::begin(
            &self.runtime,
            organization.owner.id,
            organization.organization.id,
        )
        .await
        .unwrap();
        let location = session
            .create_location(&name("Mock location"), "UTC")
            .await
            .unwrap();
        session.commit().await.unwrap();
        let recipient = tip_ok(
            tips::create_recipient(
                &self.runtime,
                organization.owner.id,
                organization.organization.id,
                location.id,
                &name("Mock recipient"),
            )
            .await,
        );
        Business {
            organization,
            recipient,
        }
    }

    async fn checkout(&self, business: &Business) -> Checkout {
        tip_ok(
            tips::create_attempt(
                &self.runtime,
                business.recipient.public_slug,
                Uuid::new_v4(),
                12345,
            )
            .await,
        )
    }

    async fn attempt_id(&self, checkout: &Checkout) -> Uuid {
        db_ok(
            sqlx::query_scalar("SELECT id FROM tips.attempts WHERE status_token = $1")
                .bind(checkout.status_token)
                .fetch_one(&self.owner)
                .await,
        )
    }

    async fn event(&self, attempt_id: Uuid) -> MockEvent {
        let Json(event) = db_ok(
            sqlx::query_scalar::<_, Json<MockEvent>>(
                "SELECT event_payload FROM tips.mock_ledger WHERE attempt_id = $1",
            )
            .bind(attempt_id)
            .fetch_one(&self.owner)
            .await,
        );
        event
    }

    async fn status(&self, checkout: &Checkout) -> String {
        tip_ok(tips::public_attempt(&self.runtime, checkout.status_token).await)
            .expect("fixture public attempt")
            .status
    }

    async fn expire(&self, attempt: Uuid) {
        db_ok(sqlx::query("UPDATE tips.mock_ledger SET expires_at = clock_timestamp() - interval '1 second' WHERE attempt_id = $1").bind(attempt).execute(&self.owner).await);
        db_ok(sqlx::query("UPDATE tips.attempts SET expires_at = clock_timestamp() - interval '1 second' WHERE id = $1").bind(attempt).execute(&self.owner).await);
    }

    async fn prioritize(&self, job: Uuid) {
        // Only a fresh fixture row is changed. Place it before historical jobs,
        // so this test never consumes another fixture's event from the queue.
        db_ok(sqlx::query("UPDATE tips.mock_delivery_outbox SET available_at = (SELECT coalesce(min(available_at), clock_timestamp()) - interval '1 second' FROM tips.mock_delivery_outbox WHERE state IN ('pending', 'leased')) WHERE id = $1").bind(job).execute(&self.owner).await);
    }

    async fn close(self) {
        db_ok(self.serial.close().await);
        self.runtime.close().await;
        self.admin.close().await;
        self.owner.close().await;
        self.worker.close().await;
    }
}

fn name(value: &str) -> Name {
    Name::parse(value).unwrap()
}
async fn connect(variable: &str) -> PgPool {
    let url = std::env::var(variable).unwrap_or_else(|_| panic!("{variable} is required"));
    let pool = db_ok(
        PgPoolOptions::new()
            .max_connections(6)
            .acquire_timeout(Duration::from_secs(3))
            .connect(&url)
            .await,
    );
    assert!(
        database(&pool).await.ends_with("_test"),
        "Tips tests require a dedicated *_test database"
    );
    pool
}
async fn database(pool: &PgPool) -> String {
    db_ok(
        sqlx::query_scalar("SELECT current_database()")
            .fetch_one(pool)
            .await,
    )
}
fn sqlstate(error: &sqlx::Error) -> String {
    error
        .as_database_error()
        .and_then(|error| error.code())
        .map_or_else(|| "unavailable".to_owned(), |code| code.into_owned())
}
fn db_ok<T>(result: Result<T, sqlx::Error>) -> T {
    result
        .unwrap_or_else(|error| panic!("Tips SQL operation failed (SQLSTATE {})", sqlstate(&error)))
}
fn tip_ok<T>(result: Result<T, TipsError>) -> T {
    result.unwrap_or_else(|error| panic!("Tips operation failed: {error}"))
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin/migration/worker URLs"]
async fn tips_isolate_tenants_require_management_roles_and_enforce_composite_references() {
    let fixture = Fixture::new().await;
    let first = fixture.business().await;
    let second = fixture.business().await;
    let org = &first.organization;
    let _first_checkout = fixture.checkout(&first).await;
    let _second_checkout = fixture.checkout(&second).await;
    for role in [Role::Manager, Role::Operator, Role::Employee] {
        let user = Provisioner::new(fixture.admin.clone())
            .create_user(&name("Role fixture"))
            .await
            .unwrap();
        Provisioner::new(fixture.admin.clone())
            .add_membership(org.organization.id, user.id, role)
            .await
            .unwrap();
        let result =
            tips::recipients(&fixture.runtime, user.id, org.organization.id, None, 50).await;
        if role == Role::Manager {
            assert_eq!(tip_ok(result).len(), 1);
        } else {
            assert!(matches!(result, Err(TipsError::Forbidden)));
            assert!(matches!(
                tips::attempts(&fixture.runtime, user.id, org.organization.id, None, 50).await,
                Err(TipsError::Forbidden)
            ));
        }
    }
    let foreign = tips::create_recipient(
        &fixture.runtime,
        org.owner.id,
        org.organization.id,
        second.recipient.location_id,
        &name("Cross tenant"),
    )
    .await;
    assert!(matches!(foreign, Err(TipsError::NotFound)));
    let mut session = TenantSession::begin(&fixture.runtime, org.owner.id, org.organization.id)
        .await
        .unwrap();
    let ids: Vec<Uuid> = db_ok(
        sqlx::query_scalar("SELECT id FROM tips.recipients")
            .fetch_all(session.connection())
            .await,
    );
    assert_eq!(ids, vec![first.recipient.id]);
    let count: i64 = db_ok(
        sqlx::query_scalar("SELECT count(*) FROM tips.attempts")
            .fetch_one(session.connection())
            .await,
    );
    assert_eq!(count, 1);
    session.commit().await.unwrap();
    for statement in [
        "UPDATE tips.attempts SET status = 'succeeded'",
        "DELETE FROM tips.recipients",
        "SELECT * FROM tips.mock_ledger LIMIT 0",
        "SELECT * FROM tips.webhook_inbox LIMIT 0",
        "SELECT * FROM tips.mock_delivery_outbox LIMIT 0",
    ] {
        let result = sqlx::query(statement).execute(&fixture.runtime).await;
        assert_eq!(
            sqlstate(
                &result.expect_err("runtime must not mutate payment states/read private facts")
            ),
            "42501"
        );
    }
    for statement in [
        "SELECT * FROM tips.mock_ledger LIMIT 0",
        "SELECT * FROM tips.webhook_inbox LIMIT 0",
        "SELECT * FROM tips.mock_delivery_outbox LIMIT 0",
    ] {
        let result = sqlx::query(statement).execute(&fixture.worker).await;
        assert_eq!(
            sqlstate(&result.expect_err("worker must use narrow queue projections")),
            "42501"
        );
    }
    let projection =
        tip_ok(tips::public_recipient(&fixture.runtime, first.recipient.public_slug).await)
            .unwrap();
    let object = serde_json::to_value(projection).unwrap();
    assert_eq!(object.as_object().unwrap().len(), 5);
    assert!(object.get("tenant_id").is_none() && object.get("location_id").is_none());
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin/migration/worker URLs"]
async fn exact_minor_amounts_and_concurrent_idempotency_do_not_create_duplicate_payments() {
    let fixture = Fixture::new().await;
    let business = fixture.business().await;
    for amount in [i64::MIN, -1, 0, 99, 1000001, i64::MAX] {
        assert!(matches!(
            tips::create_attempt(
                &fixture.runtime,
                business.recipient.public_slug,
                Uuid::new_v4(),
                amount
            )
            .await,
            Err(TipsError::InvalidAmount)
        ));
        let failed = sqlx::query("SELECT * FROM tips.create_attempt($1, $2, $3)")
            .bind(business.recipient.public_slug)
            .bind(Uuid::new_v4())
            .bind(amount)
            .execute(&fixture.runtime)
            .await;
        assert_eq!(
            sqlstate(&failed.expect_err("database must enforce exact minor-unit range")),
            "T1001"
        );
    }
    for amount in [100, 1000000] {
        let checkout = tip_ok(
            tips::create_attempt(
                &fixture.runtime,
                business.recipient.public_slug,
                Uuid::new_v4(),
                amount,
            )
            .await,
        );
        let public =
            tip_ok(tips::checkout(&fixture.runtime, checkout.checkout_token).await).unwrap();
        assert_eq!(public.amount_minor, amount);
        assert_eq!(public.currency, "RUB");
        assert_eq!(public.environment, "mock");
        assert!(checkout.checkout_token != checkout.status_token);
    }
    let key = Uuid::new_v4();
    let barrier = Arc::new(Barrier::new(4));
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let pool = fixture.runtime.clone();
        let barrier = barrier.clone();
        let slug = business.recipient.public_slug;
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            tips::create_attempt(&pool, slug, key, 35000).await
        }));
    }
    let mut tokens = Vec::new();
    for task in tasks {
        tokens.push(tip_ok(task.await.unwrap()));
    }
    assert!(
        tokens
            .iter()
            .all(|token| token.checkout_token == tokens[0].checkout_token
                && token.status_token == tokens[0].status_token)
    );
    assert!(matches!(
        tips::create_attempt(&fixture.runtime, business.recipient.public_slug, key, 35001).await,
        Err(TipsError::IdempotencyConflict)
    ));
    let count: i64 = db_ok(
        sqlx::query_scalar("SELECT count(*) FROM tips.attempts WHERE idempotency_key = $1")
            .bind(key)
            .fetch_one(&fixture.owner)
            .await,
    );
    assert_eq!(count, 1);
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin/migration/worker URLs"]
async fn provider_commit_is_durable_but_only_callback_applies_tip_success() {
    let fixture = Fixture::new().await;
    let business = fixture.business().await;
    let checkout = fixture.checkout(&business).await;
    let attempt = fixture.attempt_id(&checkout).await;
    assert_eq!(
        tip_ok(
            tips::complete_mock_checkout(&fixture.runtime, checkout.checkout_token, "succeeded")
                .await
        ),
        checkout.status_token
    );
    assert_eq!(
        tip_ok(
            tips::complete_mock_checkout(&fixture.runtime, checkout.checkout_token, "succeeded")
                .await
        ),
        checkout.status_token
    );
    assert!(matches!(
        tips::complete_mock_checkout(&fixture.runtime, checkout.checkout_token, "failed").await,
        Err(TipsError::StateConflict)
    ));
    assert_eq!(fixture.status(&checkout).await, "awaiting_payment");
    assert_eq!(
        tip_ok(tips::checkout(&fixture.runtime, checkout.checkout_token).await)
            .unwrap()
            .status,
        "succeeded"
    );
    let event = fixture.event(attempt).await;
    let jobs: i64 = db_ok(
        sqlx::query_scalar("SELECT count(*) FROM tips.mock_delivery_outbox WHERE event_id = $1")
            .bind(event.event_id)
            .fetch_one(&fixture.owner)
            .await,
    );
    assert_eq!(jobs, 1);
    assert!(tip_ok(tips::accept_event(&fixture.runtime, &event).await));
    assert!(!tip_ok(tips::accept_event(&fixture.runtime, &event).await));
    assert_eq!(fixture.status(&checkout).await, "succeeded");
    let count: i64 = db_ok(
        sqlx::query_scalar("SELECT count(*) FROM tips.webhook_inbox WHERE attempt_id = $1")
            .bind(attempt)
            .fetch_one(&fixture.owner)
            .await,
    );
    assert_eq!(count, 1);
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin/migration/worker URLs"]
async fn callback_requires_immutable_correlated_provider_evidence_and_protects_terminals() {
    let fixture = Fixture::new().await;
    let business = fixture.business().await;
    let checkout = fixture.checkout(&business).await;
    let attempt = fixture.attempt_id(&checkout).await;
    let fabricated = MockEvent {
        event_id: Uuid::new_v4(),
        attempt_id: attempt,
        provider_payment_id: Uuid::new_v4(),
        recipient_id: business.recipient.id,
        amount_minor: 12345,
        currency: "RUB".to_owned(),
        status: "succeeded".to_owned(),
    };
    assert!(matches!(
        tips::accept_event(&fixture.runtime, &fabricated).await,
        Err(TipsError::EventMismatch)
    ));
    tip_ok(tips::complete_mock_checkout(&fixture.runtime, checkout.checkout_token, "failed").await);
    let valid = fixture.event(attempt).await;
    let mut variants = Vec::new();
    let mut changed = valid.clone();
    changed.event_id = Uuid::new_v4();
    variants.push(changed);
    let mut changed = valid.clone();
    changed.attempt_id = Uuid::new_v4();
    variants.push(changed);
    let mut changed = valid.clone();
    changed.provider_payment_id = Uuid::new_v4();
    variants.push(changed);
    let mut changed = valid.clone();
    changed.recipient_id = Uuid::new_v4();
    variants.push(changed);
    let mut changed = valid.clone();
    changed.amount_minor += 1;
    variants.push(changed);
    let mut changed = valid.clone();
    changed.currency = "USD".to_owned();
    variants.push(changed);
    let mut changed = valid.clone();
    changed.status = "succeeded".to_owned();
    variants.push(changed);
    for changed in variants {
        assert!(matches!(
            tips::accept_event(&fixture.runtime, &changed).await,
            Err(TipsError::EventMismatch)
        ));
    }
    assert!(tip_ok(tips::accept_event(&fixture.runtime, &valid).await));
    assert!(!tip_ok(tips::accept_event(&fixture.runtime, &valid).await));
    assert!(matches!(
        tips::accept_event(&fixture.runtime, &fabricated).await,
        Err(TipsError::EventMismatch)
    ));
    assert_eq!(fixture.status(&checkout).await, "failed");
    let mut json = serde_json::to_value(valid).unwrap();
    json["unexpected"] = true.into();
    assert!(serde_json::from_value::<MockEvent>(json).is_err());
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin/migration/worker URLs"]
async fn lost_callback_retries_are_fenced_bounded_and_recoverable_from_the_private_ledger() {
    let fixture = Fixture::new().await;
    let business = fixture.business().await;
    let checkout = fixture.checkout(&business).await;
    let attempt = fixture.attempt_id(&checkout).await;
    tip_ok(
        tips::complete_mock_checkout(&fixture.runtime, checkout.checkout_token, "succeeded").await,
    );
    let event = fixture.event(attempt).await;
    let job: Uuid = db_ok(
        sqlx::query_scalar("SELECT id FROM tips.mock_delivery_outbox WHERE event_id = $1")
            .bind(event.event_id)
            .fetch_one(&fixture.owner)
            .await,
    );
    fixture.prioritize(job).await;
    let first = tip_ok(tips::claim_mock_delivery(&fixture.worker).await).unwrap();
    assert!(first.job_id == job && first.event == event && first.attempt == 1);
    db_ok(sqlx::query("UPDATE tips.mock_delivery_outbox SET lease_until = clock_timestamp() - interval '1 second' WHERE id = $1").bind(job).execute(&fixture.owner).await);
    let mut active = tip_ok(tips::claim_mock_delivery(&fixture.worker).await).unwrap();
    assert!(active.job_id == job && active.lease_token != first.lease_token && active.attempt == 2);
    assert!(!tip_ok(
        tips::finish_mock_delivery(&fixture.worker, job, first.lease_token, true).await
    ));
    for number in 2..=5 {
        assert!(active.event == event && active.attempt == number);
        assert!(tip_ok(
            tips::finish_mock_delivery(&fixture.worker, job, active.lease_token, false).await
        ));
        if number < 5 {
            let delayed: bool = db_ok(sqlx::query_scalar("SELECT state = 'pending' AND available_at > clock_timestamp() FROM tips.mock_delivery_outbox WHERE id = $1").bind(job).fetch_one(&fixture.owner).await);
            assert!(delayed);
            fixture.prioritize(job).await;
            active = tip_ok(tips::claim_mock_delivery(&fixture.worker).await).unwrap();
        }
    }
    let failed: bool = db_ok(sqlx::query_scalar("SELECT state = 'failed' AND attempts = 5 AND event_payload IS NOT NULL FROM tips.mock_delivery_outbox WHERE id = $1").bind(job).fetch_one(&fixture.owner).await);
    assert!(failed);
    assert_eq!(fixture.status(&checkout).await, "awaiting_payment");
    let reconciled = tip_ok(
        tips::reconcile_attempt(
            &fixture.runtime,
            business.organization.owner.id,
            business.organization.organization.id,
            attempt,
        )
        .await,
    );
    assert_eq!(reconciled.status, "succeeded");
    assert!(!tip_ok(tips::accept_event(&fixture.runtime, &event).await));
    assert_eq!(
        tip_ok(
            tips::reconcile_attempt(
                &fixture.runtime,
                business.organization.owner.id,
                business.organization.organization.id,
                attempt
            )
            .await
        )
        .status,
        "succeeded"
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin/migration/worker URLs"]
async fn expiry_uses_server_time_without_losing_an_earlier_final_provider_fact() {
    let fixture = Fixture::new().await;
    let business = fixture.business().await;
    let pending = fixture.checkout(&business).await;
    let pending_id = fixture.attempt_id(&pending).await;
    assert_eq!(
        tip_ok(
            tips::reconcile_attempt(
                &fixture.runtime,
                business.organization.owner.id,
                business.organization.organization.id,
                pending_id
            )
            .await
        )
        .status,
        "awaiting_payment"
    );
    fixture.expire(pending_id).await;
    assert_eq!(fixture.status(&pending).await, "expired");
    assert_eq!(fixture.event(pending_id).await.status, "expired");
    tip_ok(
        tips::complete_mock_checkout(&fixture.runtime, pending.checkout_token, "succeeded").await,
    );
    assert_eq!(fixture.status(&pending).await, "expired");

    let final_before_deadline = fixture.checkout(&business).await;
    let final_id = fixture.attempt_id(&final_before_deadline).await;
    tip_ok(
        tips::complete_mock_checkout(
            &fixture.runtime,
            final_before_deadline.checkout_token,
            "succeeded",
        )
        .await,
    );
    fixture.expire(final_id).await;
    assert_eq!(
        fixture.status(&final_before_deadline).await,
        "awaiting_payment"
    );
    let reconciled = tip_ok(
        tips::reconcile_attempt(
            &fixture.runtime,
            business.organization.owner.id,
            business.organization.organization.id,
            final_id,
        )
        .await,
    );
    assert_eq!(reconciled.status, "succeeded");
    fixture.close().await;
}
