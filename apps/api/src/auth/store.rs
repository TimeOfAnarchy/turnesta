//! Private session persistence through a restricted PostgreSQL function API.
//!
//! Sessions have an absolute lifetime of 60..86400 seconds with no sliding
//! refresh. Each lookup checks the database; it does not cache authentication.
//! A lookup after revocation commits sees no session under normal READ COMMITTED
//! isolation. A concurrent lookup or an already authenticated request may finish.
//! Repeated revocation is harmless and preserves the first revocation timestamp.

use sqlx::PgPool;

use crate::core::UserId;

#[derive(sqlx::FromRow)]
pub(crate) struct SessionRecord {
    pub(crate) user_id: UserId,
    pub(crate) csrf_hash: Vec<u8>,
}

pub(crate) async fn find_session(
    pool: &PgPool,
    token_hash: &[u8; 32],
) -> Result<Option<SessionRecord>, sqlx::Error> {
    sqlx::query_as("SELECT user_id, csrf_hash FROM auth.find_session($1)")
        .bind(token_hash.as_slice())
        .fetch_optional(pool)
        .await
}

pub(crate) async fn revoke_session(
    pool: &PgPool,
    token_hash: &[u8; 32],
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT auth.revoke_session($1)")
        .bind(token_hash.as_slice())
        .execute(pool)
        .await?;
    Ok(())
}
