//! Core persistence. User IDs passed to TenantSession must come from a trusted
//! identity boundary. No public HTTP identity mechanism is exposed yet.

pub mod cli;
mod model;
mod provisioning;
mod tenant;

pub use model::{
    CoreError, Location, LocationId, Membership, Name, Organization, ProvisionedOrganization, Role,
    TenantId, User, UserId,
};
pub use provisioning::Provisioner;
pub(crate) use tenant::ApiPool;
pub use tenant::{TenantPool, TenantSession};

pub(crate) async fn check_location<E: From<sqlx::Error>>(
    session: &mut TenantSession,
    location: LocationId,
    not_found: E,
) -> Result<(), E> {
    let tenant = session.tenant_id();
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM core.locations WHERE tenant_id=$1 AND id=$2)",
    )
    .bind(tenant)
    .bind(location)
    .fetch_one(session.connection())
    .await?;
    if exists { Ok(()) } else { Err(not_found) }
}
