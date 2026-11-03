//! Password secrets never enter a command journal or serialized response.
use std::{sync::Arc, time::Duration};

use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use thiserror::Error;
use tokio::sync::Semaphore;
use zeroize::Zeroizing;

use super::login::Email;
use super::token::SecretToken;
use crate::core::UserId;

const PREFIX: &str = "$argon2id$v=19$m=19456,t=2,p=1$";
// Public dummy digest: unknown email still pays the same verification work.
// It is never attached to an account; success cannot authenticate a missing row.
const DUMMY: &str = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXB1YmxpY3NhbHQ$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

pub(crate) fn setup_digest(token: &SecretToken) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"turnesta.password-setup:v1:");
    digest.update(token.hash());
    digest.finalize().into()
}

#[derive(Debug, Error)]
pub enum PasswordError {
    #[error(
        "password must contain 15..128 characters, at most 512 UTF-8 bytes, and no control characters"
    )]
    Invalid,
    #[error("password processing is busy")]
    Busy,
    #[error("password processing unavailable")]
    Unavailable,
}

pub fn validate(password: &str) -> Result<(), PasswordError> {
    let size = password.chars().count();
    if !(15..=128).contains(&size)
        || password.len() > 512
        || password.chars().any(char::is_control)
        || matches!(
            password,
            "123456789012345" | "passwordpassword" | "qwertyuiopasdfgh"
        )
    {
        return Err(PasswordError::Invalid);
    }
    Ok(())
}

fn algorithm() -> Argon2<'static> {
    Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(19456, 2, 1, Some(32)).expect("constant Argon2 policy"),
    )
}

#[derive(Clone)]
pub struct PasswordEngine {
    gate: Arc<Semaphore>,
}

impl Default for PasswordEngine {
    fn default() -> Self {
        Self {
            gate: Arc::new(Semaphore::new(2)),
        }
    }
}

impl PasswordEngine {
    async fn blocking<T: Send + 'static>(
        &self,
        operation: impl FnOnce() -> Result<T, PasswordError> + Send + 'static,
    ) -> Result<T, PasswordError> {
        let permit = self
            .gate
            .clone()
            .try_acquire_owned()
            .map_err(|_| PasswordError::Busy)?;
        let task = tokio::task::spawn_blocking(move || {
            // A timeout cannot cancel blocking code: keep its permit until it exits.
            let _permit = permit;
            operation()
        });
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .map_err(|_| PasswordError::Unavailable)?
            .map_err(|_| PasswordError::Unavailable)?
    }

    pub async fn hash(
        &self,
        password: Zeroizing<String>,
    ) -> Result<Zeroizing<String>, PasswordError> {
        validate(&password)?;
        self.blocking(move || {
            algorithm()
                .hash_password(password.as_bytes())
                .map(|hash| Zeroizing::new(hash.to_string()))
                .map_err(|_| PasswordError::Unavailable)
        })
        .await
    }

    pub async fn verify(
        &self,
        password: Zeroizing<String>,
        hash: Option<String>,
    ) -> Result<bool, PasswordError> {
        if password.len() > 512 || password.chars().count() > 128 {
            return Ok(false);
        }
        self.blocking(move || {
            let supplied = hash.is_some();
            let hash = Zeroizing::new(hash.unwrap_or_else(|| DUMMY.into()));
            if !hash.starts_with(PREFIX) {
                return Err(PasswordError::Unavailable);
            }
            let parsed = PasswordHash::new(&hash).map_err(|_| PasswordError::Unavailable)?;
            Ok(algorithm()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
                && supplied)
        })
        .await
    }
}

#[derive(sqlx::FromRow)]
pub struct Credential {
    pub identity_id: uuid::Uuid,
    pub user_id: UserId,
    pub password_hash: String,
    pub version: i64,
    pub must_change: bool,
}

#[derive(sqlx::FromRow)]
pub struct OwnCredential {
    pub email: String,
    pub password_hash: Option<String>,
    pub version: i64,
    pub requires_current_password: bool,
    pub password_enabled: bool,
}

#[derive(sqlx::FromRow)]
pub struct SetupCredential {
    pub email: String,
    pub password_hash: Option<String>,
    pub version: i64,
}

pub async fn budget(pool: &PgPool, email: &Email, scope: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT auth.password_budget($1,$2)")
        .bind(email.as_str())
        .bind(scope)
        .fetch_one(pool)
        .await
}

pub async fn lookup(pool: &PgPool, email: &Email) -> Result<Option<Credential>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM auth.password_lookup($1)")
        .bind(email.as_str())
        .fetch_optional(pool)
        .await
}

pub async fn own(pool: &PgPool, session: &[u8; 32]) -> Result<Option<OwnCredential>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM auth.password_owner_session($1)")
        .bind(session.as_slice())
        .fetch_optional(pool)
        .await
}

pub async fn admin_set(
    pool: &PgPool,
    user: UserId,
    email: &Email,
    hash: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT auth.password_admin_set($1,$2,$3)")
        .bind(user)
        .bind(email.as_str())
        .bind(hash)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn salts_differ_and_verification_rejects_unknown_or_wrong_password() {
        let engine = PasswordEngine::default();
        let secret = Zeroizing::new(String::from("A correct horse battery phrase"));
        let a = engine.hash(secret.clone()).await.unwrap();
        let b = engine.hash(secret.clone()).await.unwrap();
        assert_ne!(*a, *b);
        assert!(a.starts_with(PREFIX));
        assert!(
            engine
                .verify(secret.clone(), Some(a.to_string()))
                .await
                .unwrap()
        );
        assert!(
            !engine
                .verify(
                    Zeroizing::new("another long password".into()),
                    Some(a.to_string())
                )
                .await
                .unwrap()
        );
        assert!(!engine.verify(secret, None).await.unwrap());
        assert!(
            engine
                .verify(
                    Zeroizing::new("password".into()),
                    Some("$argon2id$v=19$m=999999,t=9,p=9$bad".into())
                )
                .await
                .is_err()
        );
    }

    #[test]
    fn passphrases_preserve_spaces_and_unicode_without_composition_rules() {
        assert!(validate("  длинная фраза со словами  ").is_ok());
        assert!(validate("aaaaaaaaaaaaaaa").is_ok());
        for bad in [
            "short",
            "123456789012345",
            "a long password\nwith a newline",
        ] {
            assert!(validate(bad).is_err());
        }
        assert!(validate(&"x".repeat(129)).is_err());
    }

    #[tokio::test]
    async fn blocking_capacity_is_bounded_without_waiting() {
        let engine = PasswordEngine::default();
        let _permits = engine.gate.clone().acquire_many_owned(2).await.unwrap();
        assert!(matches!(
            engine
                .hash(Zeroizing::new("some long password".into()))
                .await,
            Err(PasswordError::Busy)
        ));
    }
}
