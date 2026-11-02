use std::{future::Future, time::Duration};

use futures_util::TryStreamExt;
use serde::Serialize;
use sqlx::PgPool;
use subtle::ConstantTimeEq;
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::core::UserId;

use super::{AuthConfig, login::Email, mail::Mailer, token::SecretToken};

#[path = "delivery_crypto.rs"]
mod crypto;
use crypto::{DeliveryKey, Invitation};

const DATABASE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Error)]
pub enum DeliveryError {
    #[error(
        "TURNESTA_DELIVERY_KEY must contain a 32-byte key encoded as 64 hexadecimal characters"
    )]
    Key,
    #[error("could not obtain secure delivery randomness")]
    Entropy,
    #[error("could not encrypt invitation payload")]
    Encryption,
    #[error("invalid invitation delivery payload")]
    Payload,
    #[error("invalid delivery configuration: {0}")]
    Configuration(&'static str),
    #[error("delivery database operation failed")]
    Database,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct DeliveryStatus {
    pub job_id: Uuid,
    pub state: String,
    pub attempts: i32,
    pub error_category: Option<String>,
}

#[derive(sqlx::FromRow)]
struct ClaimedDelivery {
    job_id: Uuid,
    lease_token: Uuid,
    code_hash: Vec<u8>,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
    attempt: i32,
}

pub enum WorkOutcome {
    Idle,
    Processed,
}

impl WorkOutcome {
    pub fn has_work(&self) -> bool {
        matches!(self, Self::Processed)
    }
}

pub struct Worker {
    pool: PgPool,
    key: DeliveryKey,
    mailer: Mailer,
}

pub async fn is_worker_role(pool: &PgPool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT pg_has_role(current_user, 'turnesta_delivery_worker', 'MEMBER') \
         AND NOT pg_has_role(current_user, 'turnesta_runtime', 'MEMBER') \
         AND NOT pg_has_role(current_user, 'turnesta_provisioner', 'MEMBER') \
         AND NOT rolsuper AND NOT rolbypassrls FROM pg_roles WHERE rolname = current_user",
    )
    .fetch_one(pool)
    .await
}

pub(crate) struct PreparedInvitation {
    pub job_id: Uuid,
    pub code_hash: [u8; 32],
    pub key_id: [u8; 32],
    pub nonce: [u8; 24],
    pub ciphertext: Vec<u8>,
}

pub(crate) fn prepare_invitation(
    email: &Email,
    public_origin: &str,
) -> Result<PreparedInvitation, DeliveryError> {
    // Key validation and encryption happen before any identity/code mutation.
    let key = DeliveryKey::from_env()?;
    let origin = AuthConfig::parse(public_origin, 3600)
        .map_err(DeliveryError::Configuration)?
        .public_origin;
    let code = SecretToken::generate().map_err(|_| DeliveryError::Entropy)?;
    let code_hash = code.hash();
    let job_id = Uuid::new_v4();
    let invitation = Invitation {
        email: email.as_str().to_owned(),
        origin,
        code: code.expose(),
    };
    let encrypted = key.encrypt(job_id, &code_hash, &invitation)?;
    Ok(PreparedInvitation {
        job_id,
        code_hash,
        key_id: *key.id(),
        nonce: encrypted.nonce,
        ciphertext: encrypted.ciphertext,
    })
}

pub async fn enqueue_invitation(
    pool: &PgPool,
    user_id: UserId,
    email: &Email,
    public_origin: &str,
) -> Result<Uuid, DeliveryError> {
    let prepared = prepare_invitation(email, public_origin)?;
    db(
        sqlx::query("SELECT auth.enqueue_invitation($1, $2, $3, $4, $5, $6, $7)")
            .bind(prepared.job_id)
            .bind(user_id)
            .bind(email.as_str())
            .bind(prepared.code_hash.as_slice())
            .bind(prepared.key_id.as_slice())
            .bind(prepared.nonce.as_slice())
            .bind(prepared.ciphertext)
            .execute(pool),
    )
    .await?;
    Ok(prepared.job_id)
}

pub(crate) async fn verify_pending_invitations(
    connection: &mut sqlx::PgConnection,
) -> anyhow::Result<usize> {
    let key = DeliveryKey::from_env()?;
    let mut rows=sqlx::query_as::<_,(Uuid,Vec<u8>,Vec<u8>,Vec<u8>,Vec<u8>)>(
        "SELECT id,code_hash,key_id,nonce,ciphertext FROM auth.invitation_deliveries WHERE state IN('pending','leased') ORDER BY id"
    ).fetch(connection);
    let mut verified = 0;
    while let Some((id, hash, key_id, nonce, ciphertext)) = rows.try_next().await? {
        anyhow::ensure!(
            key_id.as_slice() == key.id(),
            "pending invitation uses another delivery key"
        );
        let hash: [u8; 32] = hash.try_into().map_err(|_| DeliveryError::Payload)?;
        let invitation = key.decrypt(id, &hash, &nonce, &ciphertext)?;
        let code = SecretToken::parse(&invitation.code).map_err(|_| DeliveryError::Payload)?;
        anyhow::ensure!(
            bool::from(code.hash().ct_eq(&hash)),
            "pending invitation authentication failed"
        );
        Email::parse(&invitation.email).map_err(|_| DeliveryError::Payload)?;
        AuthConfig::parse(&invitation.origin, 3600).map_err(|_| DeliveryError::Payload)?;
        verified += 1;
    }
    Ok(verified)
}

pub async fn delivery_status(
    pool: &PgPool,
    job_id: Uuid,
) -> Result<Option<DeliveryStatus>, DeliveryError> {
    db(sqlx::query_as(
        "SELECT job_id, state, attempts, error_category FROM auth.invitation_delivery_status($1)",
    )
    .bind(job_id)
    .fetch_optional(pool))
    .await
}

impl Worker {
    pub fn from_env(pool: PgPool) -> Result<Self, DeliveryError> {
        let key = DeliveryKey::from_env()?;
        let mailer = Mailer::from_env().map_err(|_| DeliveryError::Configuration("SMTP"))?;
        Ok(Self { pool, key, mailer })
    }

    /// Processes at most one lease. Callers must await this future after a stop
    /// signal: canceling during SMTP can leave an accepted message unconfirmed.
    pub async fn run_once(&self) -> Result<WorkOutcome, DeliveryError> {
        let claimed: Option<ClaimedDelivery> = db(sqlx::query_as(
            "SELECT job_id, lease_token, code_hash, nonce, ciphertext, attempt FROM auth.claim_invitation_delivery($1)",
        ).bind(self.key.id().as_slice()).fetch_optional(&self.pool)).await?;
        let Some(job) = claimed else {
            return Ok(WorkOutcome::Idle);
        };
        let invitation = self.decode(&job);
        let (email, login_link) = match invitation {
            Ok(value) => value,
            Err(_) => {
                self.finish(&job, "invalid_payload").await?;
                tracing::error!(job_id = %job.job_id, category = "invalid_payload", "invitation delivery failed");
                return Ok(WorkOutcome::Processed);
            }
        };
        let permitted: bool = db(sqlx::query_scalar(
            "SELECT auth.invitation_delivery_preflight($1, $2)",
        )
        .bind(job.job_id)
        .bind(job.lease_token)
        .fetch_one(&self.pool))
        .await?;
        if !permitted {
            self.finish(&job, "retry").await?;
            return Ok(WorkOutcome::Processed);
        }

        // No database transaction remains open across this bounded SMTP call.
        // A concurrent reissue after preflight can still deliver an invalid old
        // link; SMTP cannot participate in the database's lease fencing.
        let outcome = if self
            .mailer
            .send_invitation(&email, &login_link)
            .await
            .is_ok()
        {
            "sent"
        } else {
            "retry"
        };
        let confirmed = self.finish(&job, outcome).await?;
        tracing::info!(job_id = %job.job_id, attempt = job.attempt, outcome, confirmed, "invitation delivery attempt finished");
        Ok(WorkOutcome::Processed)
    }

    fn decode(&self, job: &ClaimedDelivery) -> Result<(Email, Zeroizing<String>), DeliveryError> {
        let code_hash: [u8; 32] = job
            .code_hash
            .as_slice()
            .try_into()
            .map_err(|_| DeliveryError::Payload)?;
        let invitation = self
            .key
            .decrypt(job.job_id, &code_hash, &job.nonce, &job.ciphertext)?;
        let code = SecretToken::parse(&invitation.code).map_err(|_| DeliveryError::Payload)?;
        if !bool::from(code.hash().ct_eq(&code_hash)) {
            return Err(DeliveryError::Payload);
        }
        let email = Email::parse(&invitation.email).map_err(|_| DeliveryError::Payload)?;
        let config =
            AuthConfig::parse(&invitation.origin, 3600).map_err(|_| DeliveryError::Payload)?;
        Ok((
            email,
            Zeroizing::new(format!(
                "{}/login#code={}",
                config.public_origin, invitation.code
            )),
        ))
    }

    async fn finish(&self, job: &ClaimedDelivery, outcome: &str) -> Result<bool, DeliveryError> {
        db(
            sqlx::query_scalar("SELECT auth.finish_invitation_delivery($1, $2, $3)")
                .bind(job.job_id)
                .bind(job.lease_token)
                .bind(outcome)
                .fetch_one(&self.pool),
        )
        .await
    }
}

async fn db<T>(future: impl Future<Output = Result<T, sqlx::Error>>) -> Result<T, DeliveryError> {
    let result = tokio::time::timeout(DATABASE_TIMEOUT, future)
        .await
        .map_err(|_| DeliveryError::Database)?;
    result.map_err(|error| {
        tracing::warn!(sqlstate = ?error.as_database_error().and_then(|error| error.code()), "delivery database operation failed");
        DeliveryError::Database
    })
}
