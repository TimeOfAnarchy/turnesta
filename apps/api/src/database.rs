use sqlx::{
    PgPool,
    migrate::Migrator,
    postgres::{PgConnectOptions, PgPoolOptions},
};

use crate::config::DatabaseConfig;

pub static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

pub async fn connect(config: &DatabaseConfig) -> Result<PgPool, sqlx::Error> {
    connect_with(config, config.options.clone()).await
}

pub async fn connect_api(config: &DatabaseConfig) -> Result<PgPool, sqlx::Error> {
    let options = config.options.clone().options([
        ("statement_timeout", "10s"),
        ("lock_timeout", "3s"),
        ("idle_in_transaction_session_timeout", "15s"),
    ]);
    connect_with(config, options).await
}

async fn connect_with(
    config: &DatabaseConfig,
    options: PgConnectOptions,
) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(config.max_connections)
        .acquire_timeout(config.acquire_timeout)
        .connect_with(options)
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
