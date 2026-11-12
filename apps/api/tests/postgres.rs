use std::time::{Duration, Instant};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use turnesta_api::{
    config::DatabaseConfig,
    database,
    http::{AppState, router},
};

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

    let state = AppState::new(pool.clone(), Duration::from_secs(1));
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

    let active_app = router(AppState::new(pool.clone(), Duration::from_secs(1)));
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
