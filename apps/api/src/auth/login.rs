use sqlx::PgPool;
use thiserror::Error;

use crate::core::UserId;

/// The first email provider uses case-insensitive ASCII addresses. Unicode
/// mailboxes and address replacement require a separate verified linking flow.
pub struct Email(String);

#[derive(Debug, Error)]
#[error("email must be a valid ASCII mailbox address")]
pub struct EmailError;

impl Email {
    pub fn parse(value: &str) -> Result<Self, EmailError> {
        let value = value.trim();
        if !value.is_ascii() || value.len() > 254 {
            return Err(EmailError);
        }
        let (local, domain) = value.split_once('@').ok_or(EmailError)?;
        if local.is_empty()
            || local.len() > 64
            || local.starts_with('.')
            || local.ends_with('.')
            || local.contains("..")
            || !local
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".!#$%&'*+-/=?^_`{|}~".contains(&byte))
            || !domain.contains('.')
            || !domain.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
        {
            return Err(EmailError);
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub async fn bind_email(pool: &PgPool, user_id: UserId, email: &Email) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT auth.bind_email($1, $2)")
        .bind(user_id)
        .bind(email.as_str())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn issue_login_code(
    pool: &PgPool,
    email: &Email,
    token_hash: &[u8; 32],
    ttl_seconds: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT auth.issue_login_code($1, $2, $3)")
        .bind(email.as_str())
        .bind(token_hash.as_slice())
        .bind(ttl_seconds)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn invalidate_login_code(
    pool: &PgPool,
    token_hash: &[u8; 32],
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT auth.invalidate_login_code($1)")
        .bind(token_hash.as_slice())
        .execute(pool)
        .await?;
    Ok(())
}

pub(crate) async fn consume_login_code(
    pool: &PgPool,
    code_hash: &[u8; 32],
    session_hash: &[u8; 32],
    csrf_hash: &[u8; 32],
    session_ttl_seconds: i32,
    previous_session_hash: Option<&[u8; 32]>,
) -> Result<Option<UserId>, sqlx::Error> {
    sqlx::query_scalar("SELECT auth.consume_login_code($1, $2, $3, $4, $5)")
        .bind(code_hash.as_slice())
        .bind(session_hash.as_slice())
        .bind(csrf_hash.as_slice())
        .bind(session_ttl_seconds)
        .bind(previous_session_hash.map(|hash| hash.as_slice()))
        .fetch_one(pool)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails_are_normalized_without_accepting_headers_or_display_names() {
        assert_eq!(
            Email::parse("  Owner+One@Example.TEST ").unwrap().as_str(),
            "owner+one@example.test"
        );
        for invalid in [
            "owner",
            "a@@example.test",
            "a@localhost",
            "a..b@example.test",
            ".a@example.test",
            "a@-example.test",
            "a@example..test",
            "Имя@example.test",
            "Owner <a@example.test>",
            "a@example.test\r\nBcc: other@example.test",
        ] {
            assert!(Email::parse(invalid).is_err());
        }
    }
}
