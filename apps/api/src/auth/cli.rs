use anyhow::{Result, bail};
use clap::Subcommand;
use sqlx::PgPool;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::core::{Name, Provisioner, UserId};

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
    /// Create the first organization and its owner with a temporary password from stdin.
    /// Does nothing when an organization already exists (admin DB role).
    Bootstrap {
        #[arg(long)]
        organization: String,
        #[arg(long)]
        owner_name: String,
        #[arg(long)]
        email: String,
        #[arg(long, required = true)]
        password_stdin: bool,
    },
}

fn read_password() -> Result<Zeroizing<String>> {
    use std::io::Read;
    let mut password = Zeroizing::new(String::new());
    std::io::stdin().take(515).read_to_string(&mut password)?;
    if password.ends_with('\n') {
        password.pop();
        if password.ends_with('\r') {
            password.pop();
        }
    }
    Ok(password)
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
            let email = Email::parse(&email)?;
            let hash = super::password::PasswordEngine::default()
                .hash(read_password()?)
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
        AuthCommand::Bootstrap {
            organization,
            owner_name,
            email,
            password_stdin: _,
        } => {
            let provisioner = Provisioner::new(pool.clone());
            if provisioner.has_organizations().await? {
                println!("{}", serde_json::json!({"status": "exists"}));
                return Ok(());
            }
            let organization = Name::parse(&organization)?;
            let owner_name = Name::parse(&owner_name)?;
            let email = Email::parse(&email)?;
            let hash = super::password::PasswordEngine::default()
                .hash(read_password()?)
                .await?;
            provisioner
                .create_organization_with_password(
                    &organization,
                    &owner_name,
                    email.as_str(),
                    &hash,
                )
                .await?;
            println!("{}", serde_json::json!({"status": "created"}));
        }
    }
    Ok(())
}
