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
pub use tenant::TenantSession;
