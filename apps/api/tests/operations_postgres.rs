use sqlx::{PgPool, postgres::PgPoolOptions};
use std::time::Duration;
use turnesta_api::operations;

async fn pool(variable: &str) -> PgPool {
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(3))
        .connect(&std::env::var(variable).unwrap())
        .await
        .unwrap();
    let name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(name.ends_with("_test"));
    pool
}
#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn operations_inspection_is_readonly_deterministic_and_checks_runtime() {
    let pool = pool("TEST_MIGRATION_DATABASE_URL").await;
    let before = serde_json::to_value(
        operations::inspect(&pool, true, true, false, 300)
            .await
            .unwrap(),
    )
    .unwrap();
    let after = serde_json::to_value(
        operations::inspect(&pool, true, true, false, 300)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(before["tables"], after["tables"]);
    assert_eq!(before["migrations"].as_array().unwrap().len(), 25);
    assert_eq!(before["ownership_valid"], true);
    assert_eq!(before["runtime_verified"], true);
    assert!(
        before["tables"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["fingerprint"].as_str().unwrap().len() == 64)
    );
    let raw = before.to_string();
    assert!(
        !raw.contains("\"password_hash\"")
            && !raw.contains("$argon2id$")
            && !raw.contains("ciphertext")
            && !raw.contains("example.test")
    );
    for table in before["tables"].as_array().unwrap() {
        assert!(
            table
                .as_object()
                .unwrap()
                .keys()
                .all(|key| matches!(key.as_str(), "schema" | "name" | "rows" | "fingerprint"))
        );
    }
    pool.close().await;
}
#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn operations_rejects_runtime_and_provisioner_before_reading_private_tables() {
    for variable in ["TEST_DATABASE_URL", "TEST_ADMIN_DATABASE_URL"] {
        let pool = pool(variable).await;
        let error = operations::inspect(&pool, false, false, false, 300)
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains("migration login"));
        pool.close().await;
    }
}
