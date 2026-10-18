use anyhow::Result;
use clap::Subcommand;
use sqlx::PgPool;

use super::{Name, Provisioner, Role, TenantId, TenantSession, UserId};

#[derive(Subcommand)]
pub enum CoreCommand {
    /// Create an organization and its initial owner atomically (admin DB role).
    OrganizationCreate {
        #[arg(long)]
        name: String,
        #[arg(long)]
        owner_name: String,
    },
    /// Create a user record; this does not create login credentials (admin DB role).
    UserCreate {
        #[arg(long)]
        name: String,
    },
    /// Grant an existing user membership in an organization (admin DB role).
    MembershipAdd {
        #[arg(long)]
        organization: TenantId,
        #[arg(long)]
        user: UserId,
        #[arg(long)]
        role: Role,
    },
    /// Create a location as a known member (runtime DB role, trusted operator CLI).
    LocationCreate {
        #[arg(long)]
        organization: TenantId,
        #[arg(long)]
        actor: UserId,
        #[arg(long)]
        name: String,
        #[arg(long)]
        time_zone: String,
    },
    /// List locations visible to a known member (runtime DB role, trusted operator CLI).
    Locations {
        #[arg(long)]
        organization: TenantId,
        #[arg(long)]
        actor: UserId,
    },
}

pub async fn run(command: CoreCommand, pool: &PgPool) -> Result<()> {
    match command {
        CoreCommand::OrganizationCreate { name, owner_name } => {
            let result = Provisioner::new(pool.clone())
                .create_organization(&Name::parse(&name)?, &Name::parse(&owner_name)?)
                .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        CoreCommand::UserCreate { name } => {
            let result = Provisioner::new(pool.clone())
                .create_user(&Name::parse(&name)?)
                .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        CoreCommand::MembershipAdd {
            organization,
            user,
            role,
        } => {
            let result = Provisioner::new(pool.clone())
                .add_membership(organization, user, role)
                .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        CoreCommand::LocationCreate {
            organization,
            actor,
            name,
            time_zone,
        } => {
            let mut session = TenantSession::begin(pool, actor, organization).await?;
            let result = session
                .create_location(&Name::parse(&name)?, &time_zone)
                .await?;
            session.commit().await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        CoreCommand::Locations {
            organization,
            actor,
        } => {
            let mut session = TenantSession::begin(pool, actor, organization).await?;
            let result = session.locations().await?;
            session.commit().await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }
    Ok(())
}
