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
    /// Set a temporary password for an existing user; read one line from stdin.
    /// First login requires the user to choose a different password.
    PasswordSet {
        #[arg(long)]
        user: UserId,
        #[arg(long)]
        email: String,
        #[arg(long, required = true)]
        password_stdin: bool,
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
        AuthCommand::PasswordSet {
            user,
            email,
            password_stdin: _,
        } => {
            use std::io::Read;
            let email = Email::parse(&email)?;
            let mut password = zeroize::Zeroizing::new(String::new());
            std::io::stdin().take(515).read_to_string(&mut password)?;
            if password.ends_with('\n') {
                password.pop();
                if password.ends_with('\r') {
                    password.pop();
                }
            }
            let hash = super::password::PasswordEngine::default()
                .hash(password)
                .await?;
            super::password::admin_set(pool, user, &email, &hash)
                .await
                .map_err(|_| {
                    anyhow::anyhow!(
                        "Could not set the credential; check user/email binding and database role"
                    )
                })?;
            println!(
                "{}",
                serde_json::json!({"status":"password_set","must_change":true,"sessions_revoked":true})
            );
        }
    }
    Ok(())
}
