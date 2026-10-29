use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            Hash,
            Serialize,
            Deserialize,
            sqlx::Type,
            utoipa::ToSchema,
        )]
        #[serde(transparent)]
        #[sqlx(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                value.parse().map(Self)
            }
        }
    };
}

id_type!(TenantId);
id_type!(UserId);
id_type!(LocationId);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum Role {
    Owner,
    Manager,
    Operator,
    Employee,
}

impl Role {
    pub fn can_manage_locations(self) -> bool {
        matches!(self, Self::Owner | Self::Manager)
    }
}

impl FromStr for Role {
    type Err = &'static str;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "owner" => Ok(Self::Owner),
            "manager" => Ok(Self::Manager),
            "operator" => Ok(Self::Operator),
            "employee" => Ok(Self::Employee),
            _ => Err("role must be owner, manager, operator or employee"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Name(String);

impl Name {
    pub fn parse(value: &str) -> Result<Self, CoreError> {
        let value = value.trim();
        if value.is_empty() || value.chars().count() > 120 || value.chars().any(char::is_control) {
            return Err(CoreError::InvalidName);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Organization {
    pub id: TenantId,
    pub name: String,
    pub version: i32,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct User {
    pub id: UserId,
    pub display_name: String,
}

#[derive(Debug, Serialize)]
pub struct ProvisionedOrganization {
    pub organization: Organization,
    pub owner: User,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Membership {
    pub tenant_id: TenantId,
    pub user_id: UserId,
    pub role: Role,
}

#[derive(Debug, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Location {
    pub tenant_id: TenantId,
    pub id: LocationId,
    pub name: String,
    pub time_zone: String,
    pub version: i32,
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("name must contain 1..120 characters and no control characters")]
    InvalidName,
    #[error("time zone must be a known IANA time zone")]
    InvalidTimeZone,
    #[error("expected version must be positive")]
    InvalidVersion,
    #[error("record has changed since it was read")]
    VersionConflict,
    #[error("organization access denied")]
    AccessDenied,
    #[error("operation requires owner or manager role")]
    Forbidden,
    #[error("operation requires the dedicated provisioning database role")]
    ProvisioningRoleRequired,
    #[error("tenant operations require the restricted runtime database role")]
    RuntimeRoleRequired,
    #[error("membership already exists")]
    MembershipExists,
    #[error("organization or user does not exist")]
    ReferenceNotFound,
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_and_count_unicode_characters() {
        assert_eq!(Name::parse("  Кофейня  ").unwrap().as_str(), "Кофейня");
        assert!(Name::parse(&"я".repeat(120)).is_ok());
        assert!(Name::parse(&"я".repeat(121)).is_err());
        assert!(Name::parse(" \t ").is_err());
        assert!(Name::parse("bad\nname").is_err());
    }

    #[test]
    fn location_management_is_limited_to_owners_and_managers() {
        assert!(Role::Owner.can_manage_locations());
        assert!(Role::Manager.can_manage_locations());
        assert!(!Role::Operator.can_manage_locations());
        assert!(!Role::Employee.can_manage_locations());
        assert!("admin".parse::<Role>().is_err());
    }
}
