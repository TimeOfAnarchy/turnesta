use crate::{
    auth::{delivery, login::Email},
    core::{CoreError, Name, Role, TenantId, TenantSession, UserId},
};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, types::Json};
use thiserror::Error;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum TeamError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("invalid team command")]
    Invalid,
    #[error("team command key reused")]
    Idempotency,
    #[error("member not found or has no email")]
    NotFound,
    #[error("wait before requesting another login email")]
    RateLimited,
    #[error("member already exists")]
    Exists,
    #[error("owner membership is protected")]
    OwnerProtected,
    #[error("member version changed")]
    Version,
    #[error("invitation delivery unavailable")]
    Delivery,
    #[error("team database error")]
    Database(sqlx::Error),
    #[error("mailbox ownership must be confirmed before another organization grants access")]
    EmailUnverified,
}
impl From<sqlx::Error> for TeamError {
    fn from(error: sqlx::Error) -> Self {
        match error.as_database_error().and_then(|e| e.code()).as_deref() {
            Some("P2401") => Self::EmailUnverified,
            Some("M2201") => Self::Core(CoreError::Forbidden),
            Some("M2202") => Self::Idempotency,
            Some("M2203") => Self::NotFound,
            Some("M2204") => Self::RateLimited,
            Some("M2205") => Self::Invalid,
            Some("M2206") => Self::Exists,
            Some("M2207") => Self::OwnerProtected,
            Some("M2208") => Self::Version,
            _ => Self::Database(error),
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TeamCommand {
    Invite {
        display_name: String,
        email: String,
        role: Role,
    },
    SetMember {
        user_id: UserId,
        role: Role,
        active: bool,
        expected_version: i32,
    },
    SendLogin {
        user_id: UserId,
        expected_version: i32,
    },
}
impl TeamCommand {
    fn normalize(&mut self) -> Result<(), TeamError> {
        match self {
            Self::Invite {
                display_name,
                email,
                role,
            } => {
                *display_name = Name::parse(display_name)?.as_str().into();
                *email = Email::parse(email)
                    .map_err(|_| TeamError::Invalid)?
                    .as_str()
                    .into();
                if *role == Role::Owner {
                    return Err(TeamError::OwnerProtected);
                }
            }
            Self::SetMember {
                role,
                expected_version,
                ..
            } => {
                if *role == Role::Owner {
                    return Err(TeamError::OwnerProtected);
                }
                if *expected_version < 1 {
                    return Err(TeamError::Invalid);
                }
            }
            Self::SendLogin {
                expected_version, ..
            } => {
                if *expected_version < 1 {
                    return Err(TeamError::Invalid);
                }
            }
        }
        Ok(())
    }
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TeamDelivery {
    pub state: String,
    pub retry_after: String,
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TeamReceipt {
    pub user_id: UserId,
    pub role: Role,
    pub active: bool,
    pub version: i32,
    pub delivery: Option<TeamDelivery>,
}
#[derive(Debug, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct TeamMember {
    pub user_id: UserId,
    pub display_name: String,
    pub email: Option<String>,
    pub role: Role,
    pub active: bool,
    pub version: i32,
    pub delivery_state: Option<String>,
    pub delivery_attempts: Option<i32>,
    pub delivery_expires_at: Option<String>,
    pub resend_after: Option<String>,
}
async fn owner(pool: &PgPool, user: UserId, tenant: TenantId) -> Result<TenantSession, TeamError> {
    let mut session = TenantSession::begin(pool, user, tenant).await?;
    if session.role() != Role::Owner {
        return Err(CoreError::Forbidden.into());
    }
    sqlx::query("SET LOCAL lock_timeout='2s'")
        .execute(session.connection())
        .await?;
    Ok(session)
}
pub async fn members(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    after: Option<UserId>,
    limit: i32,
) -> Result<Vec<TeamMember>, TeamError> {
    let mut s = owner(pool, user, tenant).await?;
    let items = sqlx::query_as("SELECT * FROM core.team_members($1,$2,$3)")
        .bind(tenant)
        .bind(after)
        .bind(limit)
        .fetch_all(s.connection())
        .await?;
    s.commit().await?;
    Ok(items)
}
pub async fn execute(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    key: Uuid,
    mut command: TeamCommand,
    origin: &str,
) -> Result<TeamReceipt, TeamError> {
    command.normalize()?;
    let mut s = owner(pool, user, tenant).await?;
    let previous: Option<Json<TeamReceipt>> =
        sqlx::query_scalar("SELECT core.team_replay($1,$2,$3)")
            .bind(tenant)
            .bind(key)
            .bind(Json(&command))
            .fetch_one(s.connection())
            .await?;
    if let Some(Json(receipt)) = previous {
        s.commit().await?;
        return Ok(receipt);
    }
    let email = match &command {
        TeamCommand::Invite { email, .. } => Some(email.clone()),
        TeamCommand::SendLogin { user_id, .. } => Some(
            sqlx::query_scalar::<_, String>("SELECT core.team_email($1,$2)")
                .bind(tenant)
                .bind(user_id)
                .fetch_one(s.connection())
                .await?,
        ),
        TeamCommand::SetMember { .. } => None,
    };
    let prepared = email
        .as_deref()
        .map(|email| {
            delivery::prepare_invitation(
                &Email::parse(email).map_err(|_| TeamError::Invalid)?,
                origin,
            )
            .map_err(|_| TeamError::Delivery)
        })
        .transpose()?;
    let Json(result) = sqlx::query_scalar::<_, Json<TeamReceipt>>(
        "SELECT core.team_execute($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(tenant)
    .bind(key)
    .bind(Json(&command))
    .bind(prepared.as_ref().map(|p| p.job_id))
    .bind(prepared.as_ref().map(|p| p.code_hash.as_slice()))
    .bind(prepared.as_ref().map(|p| p.key_id.as_slice()))
    .bind(prepared.as_ref().map(|p| p.nonce.as_slice()))
    .bind(prepared.as_ref().map(|p| p.ciphertext.as_slice()))
    .fetch_one(s.connection())
    .await?;
    s.commit().await?;
    Ok(result)
}
