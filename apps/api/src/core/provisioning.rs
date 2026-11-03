use sqlx::{PgPool, Postgres, Transaction};

use super::{
    CoreError, Membership, Name, Organization, ProvisionedOrganization, Role, TenantId, User,
    UserId,
};

/// Trusted operator commands, using a separate database login. This capability
/// must not be constructed from the HTTP runtime's database credentials.
pub struct Provisioner {
    pool: PgPool,
}

impl Provisioner {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin(&self) -> Result<Transaction<'static, Postgres>, CoreError> {
        let mut tx = self.pool.begin().await?;
        let allowed: bool = sqlx::query_scalar(
            "SELECT pg_has_role(current_user, 'turnesta_provisioner', 'MEMBER')",
        )
        .fetch_one(&mut *tx)
        .await?;
        if !allowed {
            return Err(CoreError::ProvisioningRoleRequired);
        }
        Ok(tx)
    }

    pub async fn create_organization(
        &self,
        name: &Name,
        owner_name: &Name,
    ) -> Result<ProvisionedOrganization, CoreError> {
        let mut tx = self.begin().await?;
        let organization = sqlx::query_as::<_, Organization>(
            "INSERT INTO core.organizations (id, name) VALUES ($1, $2) RETURNING id, name, version, visible_services",
        )
        .bind(TenantId::new())
        .bind(name.as_str())
        .fetch_one(&mut *tx)
        .await?;
        let owner = insert_user(&mut tx, owner_name).await?;
        insert_membership(&mut tx, organization.id, owner.id, Role::Owner).await?;
        tx.commit().await?;
        Ok(ProvisionedOrganization {
            organization,
            owner,
        })
    }

    pub async fn create_user(&self, name: &Name) -> Result<User, CoreError> {
        let mut tx = self.begin().await?;
        let user = insert_user(&mut tx, name).await?;
        tx.commit().await?;
        Ok(user)
    }

    pub async fn add_membership(
        &self,
        tenant_id: TenantId,
        user_id: UserId,
        role: Role,
    ) -> Result<Membership, CoreError> {
        let mut tx = self.begin().await?;
        let membership = insert_membership(&mut tx, tenant_id, user_id, role).await?;
        tx.commit().await?;
        Ok(membership)
    }
}

async fn insert_user(tx: &mut Transaction<'_, Postgres>, name: &Name) -> Result<User, CoreError> {
    Ok(sqlx::query_as::<_, User>(
        "INSERT INTO core.users (id, display_name) VALUES ($1, $2) RETURNING id, display_name",
    )
    .bind(UserId::new())
    .bind(name.as_str())
    .fetch_one(&mut **tx)
    .await?)
}

async fn insert_membership(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: TenantId,
    user_id: UserId,
    role: Role,
) -> Result<Membership, CoreError> {
    sqlx::query_as::<_, Membership>(
        "INSERT INTO core.memberships (tenant_id, user_id, role) VALUES ($1, $2, $3) \
         RETURNING tenant_id, user_id, role",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(role)
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| {
        match error
            .as_database_error()
            .and_then(|db| db.code())
            .as_deref()
        {
            Some("23505") => CoreError::MembershipExists,
            Some("23503") => CoreError::ReferenceNotFound,
            _ => CoreError::Database(error),
        }
    })
}
