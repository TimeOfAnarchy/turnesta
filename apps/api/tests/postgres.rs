use std::time::Duration;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use turnesta_api::{
    database,
    http::{AppState, router},
};

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
