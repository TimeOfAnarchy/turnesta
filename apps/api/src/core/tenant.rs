use sqlx::{PgPool, Postgres, Transaction};

use super::{CoreError, Location, LocationId, Name, Organization, Role, TenantId, UserId};

/// One checked membership and one transaction-local tenant context. The caller
/// supplies a user ID established by authentication, never an untrusted header.
/// Dropping an uncommitted session rolls back its transaction.
pub struct TenantSession {
    tx: Transaction<'static, Postgres>,
    tenant_id: TenantId,
    role: Role,
}

impl TenantSession {
    pub async fn begin(
        pool: &PgPool,
        user_id: UserId,
        tenant_id: TenantId,
    ) -> Result<Self, CoreError> {
        if !crate::database::is_runtime_role(pool).await? {
            return Err(CoreError::RuntimeRoleRequired);
        }
        let mut tx = pool.begin().await?;
        sqlx::query(
            "SELECT set_config('app.user_id', $1, true), set_config('app.tenant_id', $2, true)",
        )
        .bind(user_id.to_string())
        .bind(tenant_id.to_string())
        .execute(&mut *tx)
        .await?;
        let role = sqlx::query_scalar::<_, Role>(
            "SELECT role FROM core.memberships WHERE tenant_id = $1 AND user_id = $2",
        )
        .bind(tenant_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(CoreError::AccessDenied)?;
        Ok(Self {
            tx,
            tenant_id,
            role,
        })
    }

    pub async fn organization(&mut self) -> Result<Organization, CoreError> {
        sqlx::query_as("SELECT id, name, version FROM core.organizations WHERE id = $1")
            .bind(self.tenant_id)
            .fetch_optional(&mut *self.tx)
            .await?
            .ok_or(CoreError::AccessDenied)
    }

    pub async fn locations(&mut self) -> Result<Vec<Location>, CoreError> {
        Ok(sqlx::query_as(
            "SELECT tenant_id, id, name, time_zone, version FROM core.locations \
             WHERE tenant_id = $1 ORDER BY created_at, id",
        )
        .bind(self.tenant_id)
        .fetch_all(&mut *self.tx)
        .await?)
    }

    pub async fn create_location(
        &mut self,
        name: &Name,
        time_zone: &str,
    ) -> Result<Location, CoreError> {
        if !self.role.can_manage_locations() {
            return Err(CoreError::Forbidden);
        }
        let known: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM core.time_zones WHERE name = $1)")
                .bind(time_zone)
                .fetch_one(&mut *self.tx)
                .await?;
        if !known {
            return Err(CoreError::InvalidTimeZone);
        }
        Ok(sqlx::query_as(
            "INSERT INTO core.locations (tenant_id, id, name, time_zone) VALUES ($1, $2, $3, $4) \
             RETURNING tenant_id, id, name, time_zone, version",
        )
        .bind(self.tenant_id)
        .bind(LocationId::new())
        .bind(name.as_str())
        .bind(time_zone)
        .fetch_one(&mut *self.tx)
        .await?)
    }

    pub async fn locations_page(
        &mut self,
        after: Option<LocationId>,
        limit: i64,
    ) -> Result<Vec<Location>, CoreError> {
        Ok(sqlx::query_as(
            "SELECT tenant_id, id, name, time_zone, version FROM core.locations \
             WHERE tenant_id = $1 AND ($2::uuid IS NULL OR id > $2) ORDER BY id LIMIT $3",
        )
        .bind(self.tenant_id)
        .bind(after)
        .bind(limit.clamp(1, 100) + 1)
        .fetch_all(&mut *self.tx)
        .await?)
    }

    pub async fn commit(self) -> Result<(), CoreError> {
        self.tx.commit().await?;
        Ok(())
    }

    pub async fn rename_organization(
        &mut self,
        name: &Name,
        expected_version: i32,
    ) -> Result<Organization, CoreError> {
        self.check_edit(expected_version)?;
        let result = sqlx::query_as(
            "UPDATE core.organizations SET name = $1, version = version + 1 \
             WHERE id = $2 AND version = $3 RETURNING id, name, version",
        )
        .bind(name.as_str())
        .bind(self.tenant_id)
        .bind(expected_version)
        .fetch_optional(&mut *self.tx)
        .await?;
        result.ok_or(CoreError::VersionConflict)
    }

    pub async fn update_location(
        &mut self,
        id: LocationId,
        name: &Name,
        time_zone: &str,
        expected_version: i32,
    ) -> Result<Location, CoreError> {
        self.check_edit(expected_version)?;
        let known: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM core.time_zones WHERE name = $1)")
                .bind(time_zone)
                .fetch_one(&mut *self.tx)
                .await?;
        if !known {
            return Err(CoreError::InvalidTimeZone);
        }
        let result = sqlx::query_as(
            "UPDATE core.locations SET name = $1, time_zone = $2, version = version + 1 \
             WHERE tenant_id = $3 AND id = $4 AND version = $5 \
             RETURNING tenant_id, id, name, time_zone, version",
        )
        .bind(name.as_str())
        .bind(time_zone)
        .bind(self.tenant_id)
        .bind(id)
        .bind(expected_version)
        .fetch_optional(&mut *self.tx)
        .await?;
        if let Some(location) = result {
            return Ok(location);
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM core.locations WHERE tenant_id = $1 AND id = $2)",
        )
        .bind(self.tenant_id)
        .bind(id)
        .fetch_one(&mut *self.tx)
        .await?;
        Err(if exists {
            CoreError::VersionConflict
        } else {
            CoreError::ReferenceNotFound
        })
    }

    fn check_edit(&self, expected_version: i32) -> Result<(), CoreError> {
        if !self.role.can_manage_locations() {
            return Err(CoreError::Forbidden);
        }
        if expected_version < 1 {
            return Err(CoreError::InvalidVersion);
        }
        Ok(())
    }
}
