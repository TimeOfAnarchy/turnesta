use sqlx::{PgPool, postgres::PgPoolOptions};
use std::time::Duration;
use turnesta_api::{database, operations};

static OPERATIONS_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    let _guard = OPERATIONS_TEST_LOCK.lock().await;
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
    let versions: Vec<_> = before["migrations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|migration| migration["version"].as_i64().unwrap())
        .collect();
    let expected: Vec<_> = database::MIGRATOR
        .iter()
        .map(|migration| migration.version)
        .collect();
    assert_eq!(versions, expected);
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
    let _guard = OPERATIONS_TEST_LOCK.lock().await;
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

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn auth_retention_is_bounded_preserves_live_work_and_dry_run_is_read_only() {
    let _guard = OPERATIONS_TEST_LOCK.lock().await;
    let pool = pool("TEST_MIGRATION_DATABASE_URL").await;
    let mut tx = pool.begin().await.unwrap();
    let user = uuid::Uuid::new_v4();
    let identity = uuid::Uuid::new_v4();
    let email = format!("{user}@example.test");
    sqlx::query("INSERT INTO core.users(id,display_name) VALUES($1,'Retention QA')")
        .bind(user)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO auth.identities(id,provider,subject,user_id) VALUES($1,'email',$2,$3)",
    )
    .bind(identity)
    .bind(email)
    .bind(user)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO auth.sessions(token_hash,user_id,csrf_hash,created_at,expires_at) SELECT sha256(convert_to($1::text||'old'||n::text,'UTF8')),$1,sha256(convert_to(n::text,'UTF8')),now()-interval '90 days',now()-interval '31 days' FROM generate_series(1,1001) n")
        .bind(user).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO auth.sessions(token_hash,user_id,csrf_hash,created_at,expires_at) VALUES(sha256(convert_to($1::text||'live','UTF8')),$1,decode(repeat('01',32),'hex'),now()-interval '1 day',now()+interval '1 day')")
        .bind(user).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO auth.password_setup_grants(token_hash,identity_id,credential_version,expires_at) SELECT sha256(convert_to($1::text||v.label,'UTF8')),$1,1,now()+v.age FROM (VALUES ('old',interval '-31 days'),('new',interval '1 day')) v(label,age)")
        .bind(identity).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO auth.login_codes(token_hash,identity_id,created_at,expires_at) SELECT sha256(convert_to($1::text||v.label,'UTF8')),$1,now()-interval '90 days',now()-interval '31 days' FROM (VALUES ('pending'),('sent'),('free')) v(label)")
        .bind(identity).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO auth.invitation_deliveries(id,identity_id,code_hash,key_id,nonce,ciphertext,state,created_at,expires_at,finished_at) SELECT gen_random_uuid(),$1,sha256(convert_to($1::text||v.label,'UTF8')),decode(repeat('02',32),'hex'),CASE WHEN v.state='pending' THEN decode(repeat('03',24),'hex') END,CASE WHEN v.state='pending' THEN decode(repeat('04',16),'hex') END,v.state,now()-interval '90 days',now()-interval '31 days',CASE WHEN v.state='sent' THEN now()-interval '31 days' END FROM (VALUES ('pending','pending'),('sent','sent')) v(label,state)")
        .bind(identity).execute(&mut *tx).await.unwrap();

    let count = |kind: &'static str, dry: bool| {
        sqlx::query_scalar::<_, i64>("SELECT auth.purge_batch($1,$2)")
            .bind(kind)
            .bind(dry)
    };
    assert!(count("sessions", true).fetch_one(&mut *tx).await.unwrap() >= 1001);
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.sessions WHERE user_id=$1")
        .bind(user)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(before, 1002);
    assert_eq!(
        count("sessions", false).fetch_one(&mut *tx).await.unwrap(),
        1000
    );
    assert!(count("sessions", false).fetch_one(&mut *tx).await.unwrap() >= 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM auth.sessions WHERE user_id=$1")
            .bind(user)
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
        1
    );
    count("password_setup_grants", false)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    count("invitation_deliveries", false)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    count("login_codes", false)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM auth.password_setup_grants WHERE identity_id=$1"
        )
        .bind(identity)
        .fetch_one(&mut *tx)
        .await
        .unwrap(),
        1
    );
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM auth.invitation_deliveries WHERE identity_id=$1 AND state='pending'").bind(identity).fetch_one(&mut *tx).await.unwrap(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM auth.login_codes WHERE identity_id=$1")
            .bind(identity)
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
        1
    );
    let runtime_has_execute: bool = sqlx::query_scalar(
        "SELECT has_function_privilege('turnesta_api','auth.purge_batch(text,boolean)','EXECUTE')",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(!runtime_has_execute);
    tx.rollback().await.unwrap();
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn purge_orchestrator_reports_all_classes_and_rejects_runtime() {
    let _guard = OPERATIONS_TEST_LOCK.lock().await;
    let runtime = pool("TEST_DATABASE_URL").await;
    assert!(operations::purge(&runtime, true).await.is_err());
    let migrator = pool("TEST_MIGRATION_DATABASE_URL").await;
    let before = serde_json::to_value(
        operations::inspect(&migrator, true, false, false, 300)
            .await
            .unwrap(),
    )
    .unwrap();
    let dry = serde_json::to_value(operations::purge(&migrator, true).await.unwrap()).unwrap();
    let after = serde_json::to_value(
        operations::inspect(&migrator, true, false, false, 300)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(before["tables"], after["tables"]);
    assert_eq!(dry["dry_run"], true);
    assert_eq!(dry["counts"].as_array().unwrap().len(), 12);
    let actual = serde_json::to_value(operations::purge(&migrator, false).await.unwrap()).unwrap();
    assert_eq!(actual["dry_run"], false);
    assert_eq!(actual["counts"].as_array().unwrap().len(), 12);
    runtime.close().await;
    migrator.close().await;
}
