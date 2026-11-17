use std::time::{Duration, Instant};
use turnesta_api::auth::delivery::DeliveryKey;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use turnesta_api::{
    config::DatabaseConfig,
    core::{Name, Provisioner, TenantSession},
    database,
    http::{AppState, router},
};

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn appointment_journal_migration_scrubs_all_tenants_and_restores_rls() {
    let migration_url = std::env::var("TEST_MIGRATION_DATABASE_URL").unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&migration_url)
        .await
        .unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(database.ends_with("_test"));
    database::migrate(&pool).await.unwrap();
    let admin = PgPoolOptions::new()
        .connect(&std::env::var("TEST_ADMIN_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let runtime = PgPoolOptions::new()
        .connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let mut fixtures = Vec::new();
    for _ in 0..2 {
        let org = Provisioner::new(admin.clone())
            .create_organization(
                &Name::parse("Migration QA").unwrap(),
                &Name::parse("Owner").unwrap(),
            )
            .await
            .unwrap();
        let mut session = TenantSession::begin(&runtime, org.owner.id, org.organization.id)
            .await
            .unwrap();
        let point = session
            .create_location(&Name::parse("Clinic").unwrap(), "UTC")
            .await
            .unwrap()
            .id;
        session.commit().await.unwrap();
        fixtures.push((
            org.organization.id,
            point,
            org.owner.id,
            uuid::Uuid::new_v4(),
        ));
    }
    let mut tx = pool.begin().await.unwrap();
    sqlx::raw_sql("ALTER TABLE bookings.appointment_commands DROP CONSTRAINT appointment_commands_minimal_result; ALTER TABLE bookings.appointment_commands NO FORCE ROW LEVEL SECURITY;").execute(&mut *tx).await.unwrap();
    for (tenant, point, owner, appointment) in &fixtures {
        let legacy = serde_json::json!({"resource":{"kind":"appointment","item":{"id":appointment,"version":2,"client_name":"private","phone":"123","email":"private@example.test","note":"secret","answers":{"medical":"private"}}}});
        sqlx::query("INSERT INTO bookings.appointment_commands(tenant_id,location_id,actor_id,idempotency_key,fingerprint,result) VALUES($1,$2,$3,$4,$5,$6)")
            .bind(tenant).bind(point).bind(owner).bind(uuid::Uuid::new_v4()).bind(&[0_u8;32][..]).bind(sqlx::types::Json(legacy)).execute(&mut *tx).await.unwrap();
    }
    sqlx::query("ALTER TABLE bookings.appointment_commands FORCE ROW LEVEL SECURITY")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::raw_sql("SAVEPOINT malformed_result; ALTER TABLE bookings.appointment_commands NO FORCE ROW LEVEL SECURITY;")
        .execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE bookings.appointment_commands SET result=jsonb_set(result,'{resource,item,version}',to_jsonb('broken'::text)) WHERE tenant_id=$1")
        .bind(fixtures[0].0).execute(&mut *tx).await.unwrap();
    sqlx::query("ALTER TABLE bookings.appointment_commands FORCE ROW LEVEL SECURITY")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(
        sqlx::raw_sql(include_str!(
            "../../../migrations/202611120032_minimize_appointment_journal.sql"
        ))
        .execute(&mut *tx)
        .await
        .is_err()
    );
    sqlx::query("ROLLBACK TO SAVEPOINT malformed_result")
        .execute(&mut *tx)
        .await
        .unwrap();
    let force: bool = sqlx::query_scalar("SELECT relforcerowsecurity FROM pg_class WHERE oid='bookings.appointment_commands'::regclass").fetch_one(&mut *tx).await.unwrap();
    assert!(force);
    sqlx::raw_sql(include_str!(
        "../../../migrations/202611120032_minimize_appointment_journal.sql"
    ))
    .execute(&mut *tx)
    .await
    .unwrap();
    let force: bool = sqlx::query_scalar("SELECT relforcerowsecurity FROM pg_class WHERE oid='bookings.appointment_commands'::regclass").fetch_one(&mut *tx).await.unwrap();
    assert!(force);
    sqlx::query("ALTER TABLE bookings.appointment_commands NO FORCE ROW LEVEL SECURITY")
        .execute(&mut *tx)
        .await
        .unwrap();
    for (tenant, _, _, appointment) in &fixtures {
        let result: serde_json::Value = sqlx::query_scalar(
            "SELECT result FROM bookings.appointment_commands WHERE tenant_id=$1",
        )
        .bind(tenant)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(
            result,
            serde_json::json!({"kind":"appointment","id":appointment,"version":2})
        );
    }
    sqlx::query("SAVEPOINT invalid_result")
        .execute(&mut *tx)
        .await
        .unwrap();
    let result = sqlx::query("UPDATE bookings.appointment_commands SET result=jsonb_set(result,'{phone}',to_jsonb('private'::text)) WHERE tenant_id=$1")
        .bind(fixtures[0].0).execute(&mut *tx).await.unwrap_err();
    assert_eq!(
        result.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    sqlx::query("ROLLBACK TO SAVEPOINT invalid_result")
        .execute(&mut *tx)
        .await
        .unwrap();
    let error = sqlx::query("UPDATE bookings.appointment_commands SET result=jsonb_set(result,'{kind}','null'::jsonb) WHERE tenant_id=$1")
        .bind(fixtures[0].0).execute(&mut *tx).await.unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    tx.rollback().await.unwrap();
    pool.close().await;
    admin.close().await;
    runtime.close().await;
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to a dedicated PostgreSQL *_test database"]
async fn api_pool_applies_server_side_timeouts() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let config = DatabaseConfig {
        options: url.parse().unwrap(),
        max_connections: 2,
        acquire_timeout: Duration::from_secs(3),
    };
    let pool = database::connect_api(&config).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(database.ends_with("_test"));
    let start = Instant::now();
    let error = sqlx::query("SELECT pg_sleep(15)")
        .execute(&pool)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("57014")
    );
    assert!(start.elapsed() >= Duration::from_secs(9) && start.elapsed() < Duration::from_secs(15));
    let mut a = pool.acquire().await.unwrap();
    let mut b = pool.acquire().await.unwrap();
    for connection in [&mut a, &mut b] {
        let settings: (String, String, String) = sqlx::query_as("SELECT current_setting('statement_timeout'),current_setting('lock_timeout'),current_setting('idle_in_transaction_session_timeout')").fetch_one(&mut **connection).await.unwrap();
        assert_eq!(settings, ("10s".into(), "3s".into(), "15s".into()));
    }
    sqlx::query("BEGIN").execute(&mut *a).await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(908397301)")
        .execute(&mut *a)
        .await
        .unwrap();
    let start = Instant::now();
    let error = sqlx::query("SELECT pg_advisory_xact_lock(908397301)")
        .execute(&mut *b)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("55P03")
    );
    assert!(start.elapsed() >= Duration::from_secs(2) && start.elapsed() < Duration::from_secs(8));
    sqlx::query("ROLLBACK").execute(&mut *a).await.unwrap();
    sqlx::query("BEGIN").execute(&mut *b).await.unwrap();
    sqlx::query("SET LOCAL lock_timeout='2s'")
        .execute(&mut *b)
        .await
        .unwrap();
    let local: String = sqlx::query_scalar("SHOW lock_timeout")
        .fetch_one(&mut *b)
        .await
        .unwrap();
    assert_eq!(local, "2s");
    sqlx::query("ROLLBACK").execute(&mut *b).await.unwrap();
    let restored: String = sqlx::query_scalar("SHOW lock_timeout")
        .fetch_one(&mut *b)
        .await
        .unwrap();
    assert_eq!(restored, "3s");
    sqlx::query("BEGIN").execute(&mut *b).await.unwrap();
    tokio::time::sleep(Duration::from_secs(16)).await;
    assert!(sqlx::query("SELECT 1").execute(&mut *b).await.is_err());
    drop(a);
    drop(b);
    let value: i32 = sqlx::query_scalar("SELECT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(value, 1);
    pool.close().await;
    let regular = database::connect(&config).await.unwrap();
    let timeout: String = sqlx::query_scalar("SHOW statement_timeout")
        .fetch_one(&regular)
        .await
        .unwrap();
    assert_eq!(timeout, "0");
    regular.close().await;
}

#[tokio::test]
#[ignore = "requires TEST_MIGRATION_DATABASE_URL pointing to a dedicated PostgreSQL *_test database"]
async fn postgres_migrations_readiness_and_shutdown() {
    let url = std::env::var("TEST_MIGRATION_DATABASE_URL")
        .expect("TEST_MIGRATION_DATABASE_URL is required");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(2))
        .connect(&url)
        .await
        .expect("PostgreSQL must be reachable");

    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        database.ends_with("_test"),
        "integration tests require a dedicated *_test database"
    );

    database::migrate(&pool).await.expect("initial migration");
    database::migrate(&pool)
        .await
        .expect("repeated migration is safe");
    let removed: bool =
        sqlx::query_scalar("SELECT to_regprocedure('auth.password_for_session(bytea)') IS NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(removed);
    let has_core: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = 'core')")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(has_core);

    let state = AppState::new(
        pool.clone(),
        Duration::from_secs(1),
        DeliveryKey::from_hex(&"17".repeat(32)).unwrap(),
    );
    let app = router(state.clone());
    assert_eq!(
        status(app.clone(), "/health/ready").await,
        (StatusCode::OK, "ready".into())
    );

    state.begin_shutdown();
    assert_eq!(
        status(app.clone(), "/health/ready").await,
        (StatusCode::SERVICE_UNAVAILABLE, "not_ready".into()),
    );
    assert_eq!(
        status(app, "/health").await,
        (StatusCode::OK, "alive".into())
    );

    let active_app = router(AppState::new(
        pool.clone(),
        Duration::from_secs(1),
        DeliveryKey::from_hex(&"17".repeat(32)).unwrap(),
    ));
    pool.close().await;
    assert_eq!(
        status(active_app, "/health/ready").await,
        (StatusCode::SERVICE_UNAVAILABLE, "not_ready".into()),
    );
}

async fn status(app: axum::Router, path: &str) -> (StatusCode, String) {
    let response = app
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    (status, body["status"].as_str().unwrap().to_owned())
}
