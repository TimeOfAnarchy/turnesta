use sqlx::{PgPool, migrate::Migrator, postgres::PgPoolOptions};

use crate::config::DatabaseConfig;

pub static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

pub async fn connect(config: &DatabaseConfig) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(config.max_connections)
        .acquire_timeout(config.acquire_timeout)
        .connect_with(config.options.clone())
        .await
}

pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    MIGRATOR.run(pool).await
}

pub async fn is_runtime_role(pool: &PgPool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT pg_has_role(current_user, 'turnesta_runtime', 'MEMBER') \
         AND NOT pg_has_role(current_user, 'turnesta_provisioner', 'MEMBER') \
         AND NOT rolsuper AND NOT rolbypassrls FROM pg_roles WHERE rolname = current_user",
    )
    .fetch_one(pool)
    .await
}
