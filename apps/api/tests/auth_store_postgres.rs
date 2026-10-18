use std::{sync::Arc, time::Duration};

use sha2::{Digest, Sha256};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::sync::Barrier;
use turnesta_api::{
    auth::login::{self, Email},
    core::{Name, Provisioner, UserId},
};

struct Fixture {
    runtime: PgPool,
    admin: PgPool,
    owner: PgPool,
}

impl Fixture {
    async fn new() -> Self {
        let runtime = connect("TEST_DATABASE_URL").await;
        let admin = connect("TEST_ADMIN_DATABASE_URL").await;
        let owner = connect("TEST_MIGRATION_DATABASE_URL").await;
        let names = [
            database(&runtime).await,
            database(&admin).await,
            database(&owner).await,
        ];
        assert!(
            names.iter().all(|name| name == &names[0]),
            "test roles must use the same database"
        );
        Self {
            runtime,
            admin,
            owner,
        }
    }

    async fn user(&self) -> UserId {
        match Provisioner::new(self.admin.clone())
            .create_user(&Name::parse("Auth fixture").unwrap())
            .await
        {
            Ok(user) => user.id,
            Err(_) => panic!("could not provision auth fixture user"),
        }
    }

    async fn identity(&self) -> (UserId, Email) {
        let user = self.user().await;
        let email = email();
        db_ok(login::bind_email(&self.admin, user, &email).await);
        (user, email)
    }

    async fn code(&self, email: &Email) -> [u8; 32] {
        let code = hash();
        db_ok(login::issue_login_code(&self.admin, email, &code, 600).await);
        code
    }

    async fn close(self) {
        self.runtime.close().await;
        self.admin.close().await;
        self.owner.close().await;
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
        "auth tests require a dedicated *_test database"
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
    result
        .unwrap_or_else(|error| panic!("database operation failed (SQLSTATE {})", sqlstate(&error)))
}

fn sqlstate(error: &sqlx::Error) -> String {
    error
        .as_database_error()
        .and_then(|error| error.code())
        .map_or_else(|| "unavailable".to_owned(), |code| code.into_owned())
}

fn email() -> Email {
    Email::parse(&format!("auth-{}@example.test", UserId::new())).unwrap()
}

fn hash() -> [u8; 32] {
    let mut random = [0_u8; 32];
    getrandom::fill(&mut random).expect("secure fixture randomness");
    Sha256::digest(random).into()
}

async fn create_session(pool: &PgPool, user: UserId, token: &[u8; 32], csrf: &[u8; 32], ttl: i32) {
    db_ok(
        sqlx::query("SELECT auth.create_session($1, $2, $3, $4)")
            .bind(user)
            .bind(token.as_slice())
            .bind(csrf.as_slice())
            .bind(ttl)
            .execute(pool)
            .await,
    );
}

async fn consume(
    pool: &PgPool,
    code: &[u8; 32],
    session: &[u8; 32],
    previous: Option<&[u8; 32]>,
) -> Result<Option<UserId>, sqlx::Error> {
    sqlx::query_scalar("SELECT auth.consume_login_code($1, $2, $3, 3600, $4)")
        .bind(code.as_slice())
        .bind(session.as_slice())
        .bind(hash().as_slice())
        .bind(previous.map(|token| token.as_slice()))
        .fetch_one(pool)
        .await
}

async fn find(pool: &PgPool, token: &[u8; 32]) -> Option<UserId> {
    db_ok(
        sqlx::query_scalar("SELECT user_id FROM auth.find_session($1)")
            .bind(token.as_slice())
            .fetch_optional(pool)
            .await,
    )
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin/migration URLs"]
async fn private_auth_tables_and_invitation_functions_enforce_role_boundaries() {
    let fixture = Fixture::new().await;
    for pool in [&fixture.runtime, &fixture.admin] {
        for statement in [
            "SELECT * FROM auth.sessions LIMIT 0",
            "SELECT * FROM auth.identities LIMIT 0",
            "SELECT * FROM auth.login_codes LIMIT 0",
        ] {
            let result = sqlx::query(statement).execute(pool).await;
            let error = result.expect_err("private auth table must not be readable");
            assert_eq!(sqlstate(&error), "42501");
        }
    }
    let user = fixture.user().await;
    let email = email();
    let denied = login::bind_email(&fixture.runtime, user, &email).await;
    assert_eq!(
        sqlstate(&denied.expect_err("runtime cannot bind identities")),
        "42501"
    );
    let denied = login::issue_login_code(&fixture.runtime, &email, &hash(), 600).await;
    assert_eq!(
        sqlstate(&denied.expect_err("runtime cannot issue invitations")),
        "42501"
    );
    let denied = consume(&fixture.admin, &hash(), &hash(), None).await;
    assert_eq!(
        sqlstate(&denied.expect_err("provisioner cannot consume invitations")),
        "42501"
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin/migration URLs"]
async fn email_binding_is_stable_and_reissue_invalidates_the_previous_code() {
    let fixture = Fixture::new().await;
    let (user, email) = fixture.identity().await;
    db_ok(login::bind_email(&fixture.admin, user, &email).await);
    let other = fixture.user().await;
    let conflict = login::bind_email(&fixture.admin, other, &email).await;
    assert_eq!(
        sqlstate(&conflict.expect_err("identity reassignment must fail")),
        "23505"
    );
    let conflict = login::bind_email(&fixture.admin, user, &self::email()).await;
    assert_eq!(
        sqlstate(&conflict.expect_err("email replacement requires a separate flow")),
        "23505"
    );
    let count: i64 = db_ok(
        sqlx::query_scalar("SELECT count(*) FROM auth.identities WHERE user_id = $1")
            .bind(user)
            .fetch_one(&fixture.owner)
            .await,
    );
    assert_eq!(count, 1);
    let earlier = fixture.code(&email).await;
    let latest = fixture.code(&email).await;
    assert!(db_ok(consume(&fixture.runtime, &earlier, &hash(), None).await).is_none());
    assert_eq!(
        db_ok(consume(&fixture.runtime, &latest, &hash(), None).await),
        Some(user)
    );
    let verified: bool = db_ok(
        sqlx::query_scalar(
            "SELECT verified_at IS NOT NULL FROM auth.identities WHERE user_id = $1",
        )
        .bind(user)
        .fetch_one(&fixture.owner)
        .await,
    );
    assert!(verified);
    let invalidated = fixture.code(&email).await;
    db_ok(login::invalidate_login_code(&fixture.admin, &invalidated).await);
    db_ok(login::invalidate_login_code(&fixture.admin, &invalidated).await);
    assert!(db_ok(consume(&fixture.runtime, &invalidated, &hash(), None).await).is_none());
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin/migration URLs"]
async fn concurrent_consumption_creates_one_session_and_rotates_the_previous_session() {
    let fixture = Fixture::new().await;
    let (user, email) = fixture.identity().await;
    let code = fixture.code(&email).await;
    let previous = hash();
    create_session(&fixture.runtime, user, &previous, &hash(), 3600).await;
    let first = hash();
    let second = hash();
    let barrier = Arc::new(Barrier::new(2));
    let mut tasks = Vec::new();
    for session in [first, second] {
        let pool = fixture.runtime.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            consume(&pool, &code, &session, Some(&previous)).await
        }));
    }
    let mut successful = 0;
    for task in tasks {
        let result = db_ok(task.await.expect("concurrent consume task"));
        if let Some(found) = result {
            assert_eq!(found, user);
            successful += 1;
        }
    }
    assert_eq!(successful, 1);
    let count: i64 = db_ok(
        sqlx::query_scalar(
            "SELECT count(*) FROM auth.sessions WHERE token_hash = $1 OR token_hash = $2",
        )
        .bind(first.as_slice())
        .bind(second.as_slice())
        .fetch_one(&fixture.owner)
        .await,
    );
    assert_eq!(count, 1);
    assert!(find(&fixture.runtime, &previous).await.is_none());
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin/migration URLs"]
async fn expired_codes_fail_and_failed_session_creation_does_not_burn_a_valid_code() {
    let fixture = Fixture::new().await;
    let (user, email) = fixture.identity().await;
    let expired = fixture.code(&email).await;
    db_ok(sqlx::query("UPDATE auth.login_codes SET created_at = clock_timestamp() - interval '2 seconds', expires_at = clock_timestamp() - interval '1 second' WHERE token_hash = $1").bind(expired.as_slice()).execute(&fixture.owner).await);
    assert!(db_ok(consume(&fixture.runtime, &expired, &hash(), None).await).is_none());

    let occupied = hash();
    create_session(&fixture.runtime, user, &occupied, &hash(), 3600).await;
    let valid = fixture.code(&email).await;
    let failed = consume(&fixture.runtime, &valid, &occupied, None).await;
    assert_eq!(
        sqlstate(&failed.expect_err("duplicate session key must roll back consumption")),
        "23505"
    );
    let fresh = hash();
    assert_eq!(
        db_ok(consume(&fixture.runtime, &valid, &fresh, None).await),
        Some(user)
    );
    assert_eq!(find(&fixture.runtime, &fresh).await, Some(user));
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin/migration URLs"]
async fn sessions_have_absolute_expiry_and_idempotent_revocation_with_bounded_inputs() {
    let fixture = Fixture::new().await;
    let user = fixture.user().await;
    let session = hash();
    create_session(&fixture.runtime, user, &session, &hash(), 60).await;
    let ttl: i64 = db_ok(sqlx::query_scalar("SELECT extract(epoch FROM expires_at - created_at)::bigint FROM auth.sessions WHERE token_hash = $1").bind(session.as_slice()).fetch_one(&fixture.owner).await);
    assert_eq!(ttl, 60);
    assert_eq!(find(&fixture.runtime, &session).await, Some(user));
    let before: String = db_ok(
        sqlx::query_scalar("SELECT expires_at::text FROM auth.sessions WHERE token_hash = $1")
            .bind(session.as_slice())
            .fetch_one(&fixture.owner)
            .await,
    );
    assert_eq!(find(&fixture.runtime, &session).await, Some(user));
    let after: String = db_ok(
        sqlx::query_scalar("SELECT expires_at::text FROM auth.sessions WHERE token_hash = $1")
            .bind(session.as_slice())
            .fetch_one(&fixture.owner)
            .await,
    );
    assert_eq!(before, after);
    db_ok(
        sqlx::query("SELECT auth.revoke_session($1)")
            .bind(session.as_slice())
            .execute(&fixture.runtime)
            .await,
    );
    let first: String = db_ok(
        sqlx::query_scalar("SELECT revoked_at::text FROM auth.sessions WHERE token_hash = $1")
            .bind(session.as_slice())
            .fetch_one(&fixture.owner)
            .await,
    );
    db_ok(
        sqlx::query("SELECT auth.revoke_session($1)")
            .bind(session.as_slice())
            .execute(&fixture.runtime)
            .await,
    );
    let second: String = db_ok(
        sqlx::query_scalar("SELECT revoked_at::text FROM auth.sessions WHERE token_hash = $1")
            .bind(session.as_slice())
            .fetch_one(&fixture.owner)
            .await,
    );
    assert_eq!(first, second);
    assert!(find(&fixture.runtime, &session).await.is_none());

    let expiring = hash();
    create_session(&fixture.runtime, user, &expiring, &hash(), 60).await;
    db_ok(sqlx::query("UPDATE auth.sessions SET created_at = clock_timestamp() - interval '2 seconds', expires_at = clock_timestamp() - interval '1 second' WHERE token_hash = $1").bind(expiring.as_slice()).execute(&fixture.owner).await);
    assert!(find(&fixture.runtime, &expiring).await.is_none());
    for ttl in [59, 86401] {
        let invalid = sqlx::query("SELECT auth.create_session($1, $2, $3, $4)")
            .bind(user)
            .bind(hash().as_slice())
            .bind(hash().as_slice())
            .bind(ttl)
            .execute(&fixture.runtime)
            .await;
        assert_eq!(
            sqlstate(&invalid.expect_err("session TTL must be bounded")),
            "22023"
        );
    }
    let invalid = sqlx::query("SELECT auth.create_session($1, $2, $3, 60)")
        .bind(user)
        .bind(vec![0_u8; 31])
        .bind(hash().as_slice())
        .execute(&fixture.runtime)
        .await;
    assert_eq!(
        sqlstate(&invalid.expect_err("session hash must have 32 bytes")),
        "22023"
    );
    fixture.close().await;
}
