use anyhow::{Result, bail};
use clap::Subcommand;
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::UserId;

use super::{delivery, login::Email};

#[derive(Subcommand)]
pub enum AuthCommand {
    /// Bind an email and durably queue its sign-in invitation (admin DB role).
    Invite {
        #[arg(long)]
        user: UserId,
        #[arg(long)]
        email: String,
    },
    /// Inspect safe delivery metadata for a queued invitation (admin DB role).
    DeliveryStatus {
        #[arg(long)]
        job: Uuid,
    },
}

pub async fn run(command: AuthCommand, pool: &PgPool, public_origin: &str) -> Result<()> {
    match command {
        AuthCommand::Invite { user, email } => {
            let email = Email::parse(&email)?;
            let job_id = delivery::enqueue_invitation(pool, user, &email, public_origin).await?;
            println!(
                "{}",
                serde_json::json!({"status": "queued", "job_id": job_id})
            );
        }
        AuthCommand::DeliveryStatus { job } => {
            let Some(status) = delivery::delivery_status(pool, job).await? else {
                bail!("invitation delivery job not found");
            };
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
    }
    Ok(())
}
