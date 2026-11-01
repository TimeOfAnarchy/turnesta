use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, types::Json};
use utoipa::ToSchema;
use uuid::Uuid;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn client_validation_preserves_optional_contacts_and_rejects_controls() {
        let mut input = ClientInput {
            client_name: " Client ".into(),
            phone: String::new(),
            email: String::new(),
            note: " Line one\r\nLine two ".into(),
        };
        input.normalize().unwrap();
        assert_eq!(input.client_name, "Client");
        assert_eq!(input.note, "Line one\nLine two");
        for (phone, email, note) in [
            ("call me", "", ""),
            ("", "bad", ""),
            ("", "", "hidden\0control"),
        ] {
            let mut invalid = ClientInput {
                client_name: "Client".into(),
                phone: phone.into(),
                email: email.into(),
                note: note.into(),
            };
            assert!(matches!(
                invalid.normalize(),
                Err(BookingError::InvalidClient)
            ));
        }
    }
}

use crate::{
    auth::login::Email,
    bookings::{self, BookingError},
    core::{CoreError, LocationId, Name, Role, TenantId, TenantSession, UserId},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ClientInput {
    pub client_name: String,
    pub phone: String,
    pub email: String,
    pub note: String,
}
impl ClientInput {
    fn normalize(&mut self) -> Result<(), BookingError> {
        self.client_name = Name::parse(&self.client_name)
            .map_err(|_| BookingError::InvalidClient)?
            .as_str()
            .to_owned();
        self.phone = self.phone.trim().to_owned();
        if self.phone.chars().count() > 64
            || self.phone.chars().any(char::is_control)
            || (!self.phone.is_empty()
                && (!self
                    .phone
                    .chars()
                    .all(|c| c.is_ascii_digit() || "+()- .#xX".contains(c))
                    || self.phone.bytes().filter(u8::is_ascii_digit).count() < 3))
        {
            return Err(BookingError::InvalidClient);
        }
        self.email = if self.email.trim().is_empty() {
            String::new()
        } else {
            Email::parse(&self.email)
                .map_err(|_| BookingError::InvalidClient)?
                .as_str()
                .to_owned()
        };
        self.note = self.note.replace("\r\n", "\n").trim().to_owned();
        if self.note.chars().count() > 1000
            || self
                .note
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
        {
            return Err(BookingError::InvalidClient);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AppointmentCommand {
    RecordAppointment {
        id: Uuid,
        specialist_id: Uuid,
        offer_id: Uuid,
        expected_offer_version: i32,
        client: ClientInput,
    },
    UpdateClient {
        id: Uuid,
        specialist_id: Uuid,
        expected_version: i32,
        client: ClientInput,
    },
    MoveAppointment {
        id: Uuid,
        specialist_id: Uuid,
        offer_id: Uuid,
        expected_offer_version: i32,
        expected_version: i32,
    },
    CancelAppointment {
        id: Uuid,
        specialist_id: Uuid,
        expected_version: i32,
    },
}
impl AppointmentCommand {
    fn normalize(&mut self) -> Result<(), BookingError> {
        match self {
            Self::RecordAppointment {
                expected_offer_version,
                client,
                ..
            } => {
                if *expected_offer_version < 1 {
                    return Err(BookingError::Invalid);
                }
                client.normalize()?;
            }
            Self::UpdateClient {
                expected_version,
                client,
                ..
            } => {
                if *expected_version < 1 {
                    return Err(BookingError::Invalid);
                }
                client.normalize()?;
            }
            Self::MoveAppointment {
                expected_offer_version,
                expected_version,
                ..
            } => {
                if *expected_offer_version < 1 || *expected_version < 1 {
                    return Err(BookingError::Invalid);
                }
            }
            Self::CancelAppointment {
                expected_version, ..
            } => {
                if *expected_version < 1 {
                    return Err(BookingError::Invalid);
                }
            }
        }
        Ok(())
    }
    fn identity(&self) -> (Uuid, Uuid) {
        match self {
            Self::RecordAppointment {
                id, specialist_id, ..
            }
            | Self::UpdateClient {
                id, specialist_id, ..
            }
            | Self::MoveAppointment {
                id, specialist_id, ..
            }
            | Self::CancelAppointment {
                id, specialist_id, ..
            } => (*id, *specialist_id),
        }
    }
}
#[derive(Debug, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct Appointment {
    pub id: Uuid,
    pub specialist_id: Uuid,
    pub offer_id: Uuid,
    pub client_name: String,
    pub phone: String,
    pub email: String,
    pub note: String,
    pub status: String,
    pub source: String,
    pub title: String,
    pub specialist_name: String,
    pub mode: String,
    pub date: Option<String>,
    pub starts_local: Option<String>,
    pub ends_local: Option<String>,
    pub starts_at: Option<String>,
    pub ends_at: Option<String>,
    pub time_zone: String,
    pub created_by: UserId,
    pub updated_by: UserId,
    pub created_at: String,
    pub updated_at: String,
    pub cancelled_at: Option<String>,
    pub version: i32,
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "item", rename_all = "snake_case")]
pub enum AppointmentResource {
    Appointment(Appointment),
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AppointmentReceipt {
    pub resource: AppointmentResource,
}

pub struct AppointmentPage<'a> {
    pub specialist: Uuid,
    pub date: &'a str,
    pub after: Option<Uuid>,
    pub limit: i64,
    pub include_cancelled: bool,
}
pub async fn get(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    location: LocationId,
    id: Uuid,
) -> Result<Appointment, BookingError> {
    let mut s = TenantSession::begin(pool, user, tenant).await?;
    if s.role() == Role::Employee {
        return Err(CoreError::Forbidden.into());
    }
    let row=sqlx::query_as("SELECT * FROM bookings.appointment_details WHERE tenant_id=$1 AND location_id=$2 AND id=$3").bind(tenant).bind(location).bind(id).fetch_optional(s.connection()).await?.ok_or(BookingError::NotFound)?;
    s.commit().await?;
    Ok(row)
}
pub async fn list(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    location: LocationId,
    page: AppointmentPage<'_>,
) -> Result<Vec<Appointment>, BookingError> {
    let mut s = TenantSession::begin(pool, user, tenant).await?;
    if s.role() == Role::Employee {
        return Err(CoreError::Forbidden.into());
    }
    sqlx::query("SELECT bookings.valid_date($1)")
        .bind(page.date)
        .execute(s.connection())
        .await?;
    let rows=sqlx::query_as("SELECT * FROM bookings.appointment_details WHERE tenant_id=$1 AND location_id=$2 AND specialist_id=$3 AND (date=$4 OR date IS NULL) AND ($5::uuid IS NULL OR id>$5) AND ($6 OR status='confirmed') ORDER BY id LIMIT $7")
        .bind(tenant).bind(location).bind(page.specialist).bind(page.date).bind(page.after).bind(page.include_cancelled).bind(page.limit.clamp(1,100)+1).fetch_all(s.connection()).await?;
    s.commit().await?;
    Ok(rows)
}
pub async fn execute(
    pool: &PgPool,
    user: UserId,
    tenant: TenantId,
    location: LocationId,
    key: Uuid,
    mut command: AppointmentCommand,
) -> Result<AppointmentReceipt, BookingError> {
    command.normalize()?;
    let fingerprint =
        Sha256::digest(serde_json::to_vec(&command).map_err(|_| BookingError::Invalid)?);
    let mut s = TenantSession::begin(pool, user, tenant).await?;
    if s.role() == Role::Employee {
        return Err(CoreError::Forbidden.into());
    }
    sqlx::query("SET LOCAL lock_timeout='2s'")
        .execute(s.connection())
        .await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM core.locations WHERE tenant_id=$1 AND id=$2)",
    )
    .bind(tenant)
    .bind(location)
    .fetch_one(s.connection())
    .await?;
    if !exists {
        return Err(BookingError::NotFound);
    }
    let claimed=sqlx::query("INSERT INTO bookings.appointment_commands(tenant_id,location_id,actor_id,idempotency_key,fingerprint) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
        .bind(tenant).bind(location).bind(user).bind(key).bind(fingerprint.as_slice()).execute(s.connection()).await?.rows_affected();
    if claimed == 0 {
        let (previous,Json(receipt)):(Vec<u8>,Json<AppointmentReceipt>)=sqlx::query_as("SELECT fingerprint,result FROM bookings.appointment_commands WHERE tenant_id=$1 AND location_id=$2 AND actor_id=$3 AND idempotency_key=$4")
            .bind(tenant).bind(location).bind(user).bind(key).fetch_one(s.connection()).await?;
        if previous.as_slice() != fingerprint.as_slice() {
            return Err(BookingError::IdempotencyConflict);
        }
        s.commit().await?;
        return Ok(receipt);
    }
    let (id, specialist) = command.identity();
    // A separate statement before reading occupancy sees the committed winner
    // after a competing transaction releases this calendar lock.
    bookings::lock_calendar(&mut s, location, specialist).await?;
    if let AppointmentCommand::RecordAppointment {
        offer_id,
        expected_offer_version,
        ..
    }
    | AppointmentCommand::MoveAppointment {
        offer_id,
        expected_offer_version,
        ..
    } = &command
    {
        let version:i32=sqlx::query_scalar("SELECT version FROM bookings.offers WHERE tenant_id=$1 AND location_id=$2 AND specialist_id=$3 AND id=$4")
            .bind(tenant).bind(location).bind(specialist).bind(offer_id).fetch_optional(s.connection()).await?.ok_or(BookingError::NotFound)?;
        if version != *expected_offer_version {
            return Err(CoreError::VersionConflict.into());
        }
    }
    let changed=match command {
        AppointmentCommand::RecordAppointment { offer_id,client,.. } => sqlx::query("INSERT INTO bookings.appointments(tenant_id,location_id,specialist_id,offer_id,id,client_name,phone,email,note,status) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'confirmed') ON CONFLICT DO NOTHING")
            .bind(tenant).bind(location).bind(specialist).bind(offer_id).bind(id).bind(client.client_name).bind(client.phone).bind(client.email).bind(client.note).execute(s.connection()).await?.rows_affected(),
        AppointmentCommand::UpdateClient { expected_version,client,.. } => sqlx::query("UPDATE bookings.appointments SET client_name=$5,phone=$6,email=$7,note=$8,version=version+1 WHERE tenant_id=$1 AND location_id=$2 AND specialist_id=$3 AND id=$4 AND version=$9 AND status='confirmed'")
            .bind(tenant).bind(location).bind(specialist).bind(id).bind(client.client_name).bind(client.phone).bind(client.email).bind(client.note).bind(expected_version).execute(s.connection()).await?.rows_affected(),
        AppointmentCommand::MoveAppointment { offer_id,expected_version,.. } => sqlx::query("UPDATE bookings.appointments SET offer_id=$5,version=version+1 WHERE tenant_id=$1 AND location_id=$2 AND specialist_id=$3 AND id=$4 AND version=$6 AND status='confirmed' AND offer_id<>$5")
            .bind(tenant).bind(location).bind(specialist).bind(id).bind(offer_id).bind(expected_version).execute(s.connection()).await?.rows_affected(),
        AppointmentCommand::CancelAppointment { expected_version,.. } => sqlx::query("UPDATE bookings.appointments SET status='cancelled',version=version+1 WHERE tenant_id=$1 AND location_id=$2 AND specialist_id=$3 AND id=$4 AND version=$5 AND status='confirmed'")
            .bind(tenant).bind(location).bind(specialist).bind(id).bind(expected_version).execute(s.connection()).await?.rows_affected(),
    };
    if changed != 1 {
        return Err(CoreError::VersionConflict.into());
    }
    let appointment =
        sqlx::query_as("SELECT * FROM bookings.appointment_details WHERE tenant_id=$1 AND id=$2")
            .bind(tenant)
            .bind(id)
            .fetch_one(s.connection())
            .await?;
    let receipt = AppointmentReceipt {
        resource: AppointmentResource::Appointment(appointment),
    };
    sqlx::query("UPDATE bookings.appointment_commands SET result=$5 WHERE tenant_id=$1 AND location_id=$2 AND actor_id=$3 AND idempotency_key=$4")
        .bind(tenant).bind(location).bind(user).bind(key).bind(Json(&receipt)).execute(s.connection()).await?;
    s.commit().await?;
    Ok(receipt)
}
