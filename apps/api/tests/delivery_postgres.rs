use std::{sync::Arc, time::Duration};

use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::sync::Barrier;
use turnesta_api::{
    auth::delivery,
    core::{Name, Provisioner, UserId},
};
use uuid::Uuid;

struct Fixture {
    runtime: PgPool,
    admin: PgPool,
    owner: PgPool,
    worker: PgPool,
    key_id: [u8; 32],
}

struct Queued {
    id: Uuid,
    code_hash: [u8; 32],
    nonce: [u8; 24],
    ciphertext: [u8; 48],
}

#[derive(sqlx::FromRow)]
struct Claim {
    job_id: Uuid,
    lease_token: Uuid,
    code_hash: Vec<u8>,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
    attempt: i32,
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
            "delivery test roles must use the same database"
        );
        Self {
            runtime,
            admin,
            owner,
            worker,
            key_id: random(),
        }
    }

    async fn user(&self) -> UserId {
        match Provisioner::new(self.admin.clone())
            .create_user(&Name::parse("Delivery fixture").unwrap())
            .await
        {
            Ok(user) => user.id,
            Err(_) => panic!("could not create delivery fixture user"),
        }
    }

    async fn queue(&self) -> Queued {
        let user = self.user().await;
        let email = format!("delivery-{}@example.test", Uuid::new_v4());
        let queued = queued();
        enqueue(&self.admin, user, &email, &self.key_id, &queued).await;
        queued
    }

    async fn state(&self, id: Uuid) -> String {
        match delivery::delivery_status(&self.admin, id).await {
            Ok(Some(status)) => status.state,
            _ => panic!("could not read safe delivery status"),
        }
    }

    async fn close(self) {
        self.runtime.close().await;
        self.admin.close().await;
        self.owner.close().await;
        self.worker.close().await;
    }
}

async fn connect(variable: &str) -> PgPool {
    let url = std::env::var(variable).unwrap_or_else(|_| panic!("{variable} is required"));
    let pool = db_ok(
        PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(3))
            .connect(&url)
            .await,
    );
    assert!(
        database(&pool).await.ends_with("_test"),
        "delivery tests require a dedicated *_test database"
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

fn db_ok<T>(result: Result<T, sqlx::Error>) -> T {
    result.unwrap_or_else(|error| {
        panic!(
            "delivery test database operation failed (SQLSTATE {})",
            sqlstate(&error)
        )
    })
}

fn sqlstate(error: &sqlx::Error) -> String {
    error
        .as_database_error()
        .and_then(|error| error.code())
        .map_or_else(|| "unavailable".to_owned(), |code| code.into_owned())
}

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes).expect("secure fixture randomness");
    bytes
}

fn queued() -> Queued {
    // SQL contract tests use opaque byte fixtures, never invoke SMTP. Actual
    // payload encryption/authentication is verified in delivery_crypto tests.
    Queued {
        id: Uuid::new_v4(),
        code_hash: random(),
        nonce: random(),
        ciphertext: random(),
    }
}

async fn enqueue(pool: &PgPool, user: UserId, email: &str, key_id: &[u8; 32], queued: &Queued) {
    db_ok(
        sqlx::query("SELECT auth.enqueue_invitation($1, $2, $3, $4, $5, $6, $7)")
            .bind(queued.id)
            .bind(user)
            .bind(email)
            .bind(queued.code_hash.as_slice())
            .bind(key_id.as_slice())
            .bind(queued.nonce.as_slice())
            .bind(queued.ciphertext.as_slice())
            .execute(pool)
            .await,
    );
}

async fn claim(pool: &PgPool, key_id: &[u8; 32]) -> Result<Option<Claim>, sqlx::Error> {
    sqlx::query_as("SELECT job_id, lease_token, code_hash, nonce, ciphertext, attempt FROM auth.claim_invitation_delivery($1)")
        .bind(key_id.as_slice()).fetch_optional(pool).await
}

async fn finish(pool: &PgPool, claim: &Claim, outcome: &str) -> bool {
    db_ok(
        sqlx::query_scalar("SELECT auth.finish_invitation_delivery($1, $2, $3)")
            .bind(claim.job_id)
            .bind(claim.lease_token)
            .bind(outcome)
            .fetch_one(pool)
            .await,
    )
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin/migration/worker URLs"]
async fn worker_has_narrow_delivery_capability_without_core_or_private_table_access() {
    let fixture = Fixture::new().await;
    assert!(db_ok(delivery::is_worker_role(&fixture.worker).await));
    assert!(!db_ok(delivery::is_worker_role(&fixture.runtime).await));
    assert!(!db_ok(delivery::is_worker_role(&fixture.admin).await));
    for pool in [&fixture.runtime, &fixture.admin, &fixture.worker] {
        let result = sqlx::query("SELECT * FROM auth.invitation_deliveries LIMIT 0")
            .execute(pool)
            .await;
        assert_eq!(
            sqlstate(&result.expect_err("delivery table must be private")),
            "42501"
        );
    }
    for statement in [
        "SELECT * FROM core.users LIMIT 0",
        "SELECT * FROM auth.identities LIMIT 0",
        "SELECT * FROM auth.sessions LIMIT 0",
        "SELECT * FROM auth.login_codes LIMIT 0",
    ] {
        let result = sqlx::query(statement).execute(&fixture.worker).await;
        assert_eq!(
            sqlstate(&result.expect_err("worker must not read identity or business tables")),
            "42501"
        );
    }
    for pool in [&fixture.runtime, &fixture.admin] {
        let denied = claim(pool, &fixture.key_id).await;
        let Err(error) = denied else {
            panic!("only worker may claim delivery jobs");
        };
        assert_eq!(sqlstate(&error), "42501");
    }
    let denied = sqlx::query("SELECT auth.enqueue_invitation(gen_random_uuid(), gen_random_uuid(), 'denied@example.test', $1, $1, $2, $3)")
        .bind(random::<32>().as_slice()).bind(random::<24>().as_slice()).bind(random::<48>().as_slice())
        .execute(&fixture.worker).await;
    assert_eq!(
        sqlstate(&denied.expect_err("worker must not issue invitations")),
        "42501"
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin/migration/worker URLs"]
async fn binding_code_and_queue_commit_atomically_and_reissue_cancels_obsolete_jobs() {
    let fixture = Fixture::new().await;
    let user = fixture.user().await;
    let email = format!("delivery-{}@example.test", Uuid::new_v4());
    let first = queued();
    enqueue(&fixture.admin, user, &email, &fixture.key_id, &first).await;
    let second_user = fixture.user().await;
    let second_email = format!("delivery-{}@example.test", Uuid::new_v4());
    let mut conflicting = queued();
    conflicting.id = first.id;
    let failed = sqlx::query("SELECT auth.enqueue_invitation($1, $2, $3, $4, $5, $6, $7)")
        .bind(conflicting.id)
        .bind(second_user)
        .bind(&second_email)
        .bind(conflicting.code_hash.as_slice())
        .bind(fixture.key_id.as_slice())
        .bind(conflicting.nonce.as_slice())
        .bind(conflicting.ciphertext.as_slice())
        .execute(&fixture.admin)
        .await;
    assert_eq!(
        sqlstate(&failed.expect_err("duplicate job must roll back all provisioning")),
        "23505"
    );
    let binding_exists: bool = db_ok(
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM auth.identities WHERE user_id = $1)")
            .bind(second_user)
            .fetch_one(&fixture.owner)
            .await,
    );
    let code_exists: bool = db_ok(
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM auth.login_codes WHERE token_hash = $1)")
            .bind(conflicting.code_hash.as_slice())
            .fetch_one(&fixture.owner)
            .await,
    );
    assert!(!binding_exists && !code_exists);

    let second = queued();
    enqueue(&fixture.admin, user, &email, &fixture.key_id, &second).await;
    assert_eq!(fixture.state(first.id).await, "canceled");
    let wiped: bool = db_ok(sqlx::query_scalar("SELECT nonce IS NULL AND ciphertext IS NULL FROM auth.invitation_deliveries WHERE id = $1").bind(first.id).fetch_one(&fixture.owner).await);
    assert!(wiped);
    let old_used: bool = db_ok(
        sqlx::query_scalar(
            "SELECT used_at IS NOT NULL FROM auth.login_codes WHERE token_hash = $1",
        )
        .bind(first.code_hash.as_slice())
        .fetch_one(&fixture.owner)
        .await,
    );
    assert!(old_used);
    let same_expiry: bool = db_ok(sqlx::query_scalar("SELECT job.expires_at = code.expires_at FROM auth.invitation_deliveries job JOIN auth.login_codes code ON code.token_hash = job.code_hash WHERE job.id = $1").bind(second.id).fetch_one(&fixture.owner).await);
    assert!(same_expiry);
    let claimed =
        db_ok(claim(&fixture.worker, &fixture.key_id).await).expect("latest job must be claimable");
    assert_eq!(claimed.job_id, second.id);
    assert!(finish(&fixture.worker, &claimed, "sent").await);
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin/migration/worker URLs"]
async fn concurrent_claim_and_crash_recovery_fence_a_stale_worker_acknowledgement() {
    let fixture = Fixture::new().await;
    let queued = fixture.queue().await;
    let barrier = Arc::new(Barrier::new(2));
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let pool = fixture.worker.clone();
        let key_id = fixture.key_id;
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            claim(&pool, &key_id).await
        }));
    }
    let mut claimed = Vec::new();
    for task in tasks {
        if let Some(job) = db_ok(task.await.expect("claim task")) {
            claimed.push(job);
        }
    }
    assert_eq!(claimed.len(), 1);
    let first = claimed.pop().unwrap();
    assert_eq!(first.attempt, 1);
    db_ok(sqlx::query("UPDATE auth.invitation_deliveries SET lease_until = clock_timestamp() - interval '1 second' WHERE id = $1").bind(queued.id).execute(&fixture.owner).await);
    let recovered =
        db_ok(claim(&fixture.worker, &fixture.key_id).await).expect("expired lease must recover");
    assert_eq!(recovered.attempt, 2);
    assert!(first.lease_token != recovered.lease_token);
    assert!(!finish(&fixture.worker, &first, "sent").await);
    assert!(finish(&fixture.worker, &recovered, "sent").await);
    assert_eq!(fixture.state(queued.id).await, "sent");
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin/migration/worker URLs"]
async fn retries_keep_the_same_code_payload_and_stop_after_five_claims_without_wrong_key_drain() {
    let fixture = Fixture::new().await;
    let queued = fixture.queue().await;
    assert!(db_ok(claim(&fixture.worker, &random()).await).is_none());
    let attempts: i32 = db_ok(
        sqlx::query_scalar("SELECT attempts FROM auth.invitation_deliveries WHERE id = $1")
            .bind(queued.id)
            .fetch_one(&fixture.owner)
            .await,
    );
    assert_eq!(attempts, 0);
    for attempt in 1..=5 {
        let job = db_ok(claim(&fixture.worker, &fixture.key_id).await).expect("ready job");
        assert_eq!(job.attempt, attempt);
        assert!(
            job.code_hash == queued.code_hash
                && job.nonce == queued.nonce
                && job.ciphertext == queued.ciphertext
        );
        assert!(finish(&fixture.worker, &job, "retry").await);
        if attempt < 5 {
            assert_eq!(fixture.state(queued.id).await, "pending");
            assert!(db_ok(claim(&fixture.worker, &fixture.key_id).await).is_none());
            let delay: f64 = db_ok(sqlx::query_scalar("SELECT extract(epoch FROM available_at - clock_timestamp())::double precision FROM auth.invitation_deliveries WHERE id = $1").bind(queued.id).fetch_one(&fixture.owner).await);
            let expected = match attempt {
                1 => 15.0,
                2 => 30.0,
                3 => 60.0,
                _ => 120.0,
            };
            assert!(
                delay > expected - 5.0 && delay <= expected,
                "retry delay must follow bounded backoff"
            );
            db_ok(sqlx::query("UPDATE auth.invitation_deliveries SET available_at = clock_timestamp() - interval '1 second' WHERE id = $1").bind(queued.id).execute(&fixture.owner).await);
        }
    }
    assert_eq!(fixture.state(queued.id).await, "failed");
    assert!(db_ok(claim(&fixture.worker, &fixture.key_id).await).is_none());
    let terminal: bool = db_ok(sqlx::query_scalar("SELECT nonce IS NULL AND ciphertext IS NULL AND last_error_category = 'attempts_exhausted' FROM auth.invitation_deliveries WHERE id = $1").bind(queued.id).fetch_one(&fixture.owner).await);
    assert!(terminal);
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin/migration/worker URLs"]
async fn expiry_consumed_codes_and_last_attempt_crashes_are_terminal_without_dispatch() {
    let fixture = Fixture::new().await;
    let expired = fixture.queue().await;
    db_ok(sqlx::query("UPDATE auth.invitation_deliveries SET created_at = clock_timestamp() - interval '2 seconds', expires_at = clock_timestamp() - interval '1 second' WHERE id = $1").bind(expired.id).execute(&fixture.owner).await);
    assert!(db_ok(claim(&fixture.worker, &fixture.key_id).await).is_none());
    assert_eq!(fixture.state(expired.id).await, "expired");

    let used = fixture.queue().await;
    let user: Option<UserId> = db_ok(
        sqlx::query_scalar("SELECT auth.consume_login_code($1, $2, $3, 3600, NULL)")
            .bind(used.code_hash.as_slice())
            .bind(random::<32>().as_slice())
            .bind(random::<32>().as_slice())
            .fetch_one(&fixture.runtime)
            .await,
    );
    assert!(user.is_some());
    assert!(db_ok(claim(&fixture.worker, &fixture.key_id).await).is_none());
    assert_eq!(fixture.state(used.id).await, "canceled");

    let crashed = fixture.queue().await;
    db_ok(
        sqlx::query("UPDATE auth.invitation_deliveries SET attempts = 4 WHERE id = $1")
            .bind(crashed.id)
            .execute(&fixture.owner)
            .await,
    );
    let final_claim =
        db_ok(claim(&fixture.worker, &fixture.key_id).await).expect("last attempt is claimable");
    assert_eq!(final_claim.attempt, 5);
    db_ok(sqlx::query("UPDATE auth.invitation_deliveries SET lease_until = clock_timestamp() - interval '1 second' WHERE id = $1").bind(crashed.id).execute(&fixture.owner).await);
    assert!(db_ok(claim(&fixture.worker, &fixture.key_id).await).is_none());
    assert!(!finish(&fixture.worker, &final_claim, "sent").await);
    assert_eq!(fixture.state(crashed.id).await, "failed");
    fixture.close().await;
}
