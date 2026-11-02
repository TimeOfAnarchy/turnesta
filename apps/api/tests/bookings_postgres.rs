use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header::SET_COOKIE},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::time::Duration;
use tower::ServiceExt;
use turnesta_api::{
    appointments::{self, AppointmentCommand as AppointmentCmd, AppointmentResource, ClientInput},
    auth::login::{self, Email},
    bookings::{
        self, BookingCommand as Command, BookingError, BookingResource, OfferState, Timing,
    },
    bookings_batch::{self as batch, BatchReceipt, BatchRequest, BatchSource},
    bookings_public::{
        self as public_bookings, BookingFieldKind, BookingForm, BookingFormField,
        PublicBookingError as PubError, PublicBookingInput,
    },
    bookings_widget::{self as widget, WidgetConfigure, WidgetError},
    core::{CoreError, LocationId, Name, Provisioner, Role, TenantId, TenantSession, UserId},
    http::{AppState, router},
};
use uuid::Uuid;

async fn pool(key: &str, size: u32) -> PgPool {
    let pool = PgPoolOptions::new()
        .max_connections(size)
        .acquire_timeout(Duration::from_secs(3))
        .connect(&std::env::var(key).expect("test URL required"))
        .await
        .unwrap();
    let name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(name.ends_with("_test"));
    pool
}

fn client(name: &str) -> ClientInput {
    ClientInput {
        client_name: name.into(),
        phone: " +7 (900) 000-00-00 ".into(),
        email: " CLIENT@EXAMPLE.TEST ".into(),
        note: " Позвонить перед визитом\r\nПосле 10:00 ".into(),
    }
}
fn record(specialist: Uuid, offer: Uuid, version: i32, name: &str) -> AppointmentCmd {
    AppointmentCmd::RecordAppointment {
        id: Uuid::new_v4(),
        specialist_id: specialist,
        offer_id: offer,
        expected_offer_version: version,
        client: client(name),
    }
}
async fn appointment_execute(
    f: &Fixture,
    user: UserId,
    key: Uuid,
    command: AppointmentCmd,
) -> Result<appointments::Appointment, BookingError> {
    let receipt = appointments::execute(&f.runtime, user, f.tenant, f.point, key, command).await?;
    let AppointmentResource::Appointment(item) = receipt.resource;
    Ok(item)
}
async fn saved_offer(f: &Fixture, specialist: Uuid, timing: Timing) -> bookings::Offer {
    let mut command = offer(specialist, timing);
    let receipt = match bookings::execute(
        &f.runtime,
        f.owner,
        f.tenant,
        f.point,
        Uuid::new_v4(),
        command.clone(),
    )
    .await
    {
        Ok(receipt) => receipt,
        Err(BookingError::Overlap(notice)) => {
            if let Command::SaveOffer { conflict_token, .. } = &mut command {
                *conflict_token = Some(notice.token);
            }
            bookings::execute(
                &f.runtime,
                f.owner,
                f.tenant,
                f.point,
                Uuid::new_v4(),
                command,
            )
            .await
            .unwrap()
        }
        Err(e) => panic!("offer creation failed: {e:?}"),
    };
    let BookingResource::Offer(item) = receipt.resource else {
        panic!("offer expected")
    };
    item
}
fn appointment_post(
    f: &Fixture,
    login: &(String, String),
    key: Uuid,
    body: Value,
) -> Request<Body> {
    Request::post(f.url("appointments/commands"))
        .header("cookie", &login.0)
        .header("origin", "http://127.0.0.1:8080")
        .header("x-csrf-token", &login.1)
        .header("idempotency-key", key.to_string())
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[test]
fn appointment_payload_rejects_spoofed_snapshot_and_transition_fields() {
    let command = record(Uuid::new_v4(), Uuid::new_v4(), 1, "Client");
    for field in ["created_by", "status", "starts_at", "source"] {
        let mut body = serde_json::to_value(&command).unwrap();
        body[field] = json!("spoofed");
        assert!(serde_json::from_value::<AppointmentCmd>(body).is_err());
    }
    let mut body = serde_json::to_value(command).unwrap();
    body["client"]["diagnosis"] = json!("unknown field");
    assert!(serde_json::from_value::<AppointmentCmd>(body).is_err());
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn appointments_schedule_edit_race_has_no_silent_client_rescheduling() {
    let f = Fixture::new("Europe/Moscow").await;
    let spec = f.specialist().await;
    let o = saved_offer(
        &f,
        spec.id,
        interval("2026-11-16T10:00", "2026-11-16T11:00"),
    )
    .await;
    let edit = Command::SaveOffer {
        id: o.id,
        specialist_id: spec.id,
        time_zone: o.time_zone.clone(),
        title: o.title.clone(),
        timing: interval("2026-11-16T12:00", "2026-11-16T13:00"),
        state: OfferState::Ready,
        expected_version: 1,
        conflict_token: None,
    };
    let (edited, booked) = tokio::join!(
        bookings::execute(&f.runtime, f.owner, f.tenant, f.point, Uuid::new_v4(), edit),
        appointment_execute(
            &f,
            f.owner,
            Uuid::new_v4(),
            record(spec.id, o.id, 1, "Race")
        )
    );
    assert_eq!(usize::from(edited.is_ok()) + usize::from(booked.is_ok()), 1);
    if let Ok(booked) = booked {
        assert_eq!(booked.starts_at, o.starts_at);
        assert!(matches!(edited, Err(BookingError::OfferOccupied)));
    } else {
        assert!(matches!(
            booked,
            Err(BookingError::Core(CoreError::VersionConflict))
        ));
    }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn appointments_database_guards_prevent_direct_capacity_and_snapshot_bypass() {
    let f = Fixture::new("Europe/Moscow").await;
    let spec = f.specialist().await;
    let o = saved_offer(
        &f,
        spec.id,
        interval("2026-11-17T10:00", "2026-11-17T11:00"),
    )
    .await;
    let operator = f.member(Role::Operator).await;
    let a = appointment_execute(
        &f,
        operator,
        Uuid::new_v4(),
        record(spec.id, o.id, 1, "Existing"),
    )
    .await
    .unwrap();
    let mut s = TenantSession::begin(&f.runtime, operator, f.tenant)
        .await
        .unwrap();
    let error=sqlx::query("INSERT INTO bookings.appointments(tenant_id,location_id,specialist_id,offer_id,id,client_name,phone,email,note,status,starts_at) VALUES($1,$2,$3,$4,$5,'Direct','','','','confirmed','2099-01-01Z')")
        .bind(f.tenant).bind(f.point).bind(spec.id).bind(o.id).bind(Uuid::new_v4()).execute(s.connection()).await.unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("B2104")
    );
    drop(s);
    let mut s = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    let error=sqlx::query("UPDATE bookings.offers SET starts_local='2026-11-17 12:00',starts_at='2026-11-17 09:00Z',ends_local='2026-11-17 13:00',ends_at='2026-11-17 10:00Z' WHERE tenant_id=$1 AND id=$2").bind(f.tenant).bind(o.id).execute(s.connection()).await.unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("B2106")
    );
    drop(s);
    let mut s = TenantSession::begin(&f.runtime, operator, f.tenant)
        .await
        .unwrap();
    let error = sqlx::query(
        "UPDATE bookings.appointments SET starts_at='2099-01-01Z' WHERE tenant_id=$1 AND id=$2",
    )
    .bind(f.tenant)
    .bind(a.id)
    .execute(s.connection())
    .await
    .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    drop(s);
    assert!(matches!(
        bookings::execute(
            &f.runtime,
            operator,
            f.tenant,
            f.point,
            Uuid::new_v4(),
            Command::SaveSpecialist {
                id: spec.id,
                name: "Wrong".into(),
                description: String::new(),
                enabled: false,
                expected_version: 1
            }
        )
        .await,
        Err(BookingError::Core(CoreError::Forbidden))
    ));
    let unchanged = appointments::get(&f.runtime, f.owner, f.tenant, f.point, a.id)
        .await
        .unwrap();
    assert_eq!(unchanged.starts_at, o.starts_at);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn appointments_modes_contacts_snapshots_and_idempotent_history() {
    let f = Fixture::new("Europe/Moscow").await;
    let spec = f.specialist().await;
    let timed = saved_offer(
        &f,
        spec.id,
        interval("2026-11-10T10:00", "2026-11-10T10:30"),
    )
    .await;
    let command = record(spec.id, timed.id, timed.version, "  Client One  ");
    let key = Uuid::new_v4();
    let item = appointment_execute(&f, f.owner, key, command.clone())
        .await
        .unwrap();
    assert_eq!(item.client_name, "Client One");
    assert_eq!(item.phone, "+7 (900) 000-00-00");
    assert_eq!(item.email, "client@example.test");
    assert_eq!(item.note, "Позвонить перед визитом\nПосле 10:00");
    assert_eq!(item.created_by, Some(f.owner));
    assert_eq!(item.starts_at, timed.starts_at);
    assert_eq!(item.specialist_name, "Doctor");
    let cancelled = appointment_execute(
        &f,
        f.owner,
        Uuid::new_v4(),
        AppointmentCmd::CancelAppointment {
            id: item.id,
            specialist_id: spec.id,
            expected_version: 1,
        },
    )
    .await
    .unwrap();
    assert_eq!(cancelled.status, "cancelled");
    assert!(cancelled.cancelled_at.is_some());
    let replay = appointment_execute(&f, f.owner, key, command.clone())
        .await
        .unwrap();
    assert_eq!(replay.status, "confirmed");
    assert_eq!(replay.version, 1);
    assert_eq!(replay.created_at, item.created_at);
    let mut different = command.clone();
    if let AppointmentCmd::RecordAppointment { client, .. } = &mut different {
        client.client_name = "Other".into();
    }
    assert!(matches!(
        appointment_execute(&f, f.owner, key, different).await,
        Err(BookingError::IdempotencyConflict)
    ));
    let start = saved_offer(
        &f,
        spec.id,
        Timing::StartOnly {
            starts_local: "2026-11-10T11:00".into(),
        },
    )
    .await;
    let a = appointment_execute(
        &f,
        f.owner,
        Uuid::new_v4(),
        record(spec.id, start.id, 1, "No end"),
    )
    .await
    .unwrap();
    assert!(a.ends_at.is_none());
    let agreement = saved_offer(
        &f,
        spec.id,
        Timing::ByAgreement {
            date: None,
            places: 2,
        },
    )
    .await;
    let a = appointment_execute(
        &f,
        f.owner,
        Uuid::new_v4(),
        record(spec.id, agreement.id, 1, "No date"),
    )
    .await
    .unwrap();
    assert!(a.date.is_none() && a.starts_at.is_none());
    let offers = f.offers(spec.id, "2026-11-10").await;
    assert_eq!(
        offers
            .iter()
            .find(|o| o.id == agreement.id)
            .unwrap()
            .reserved_places,
        Some(1)
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn appointments_last_place_and_overlapping_alternatives_serialize() {
    let f = Fixture::new("Europe/Moscow").await;
    let spec = f.specialist().await;
    let agreement = saved_offer(
        &f,
        spec.id,
        Timing::ByAgreement {
            date: Some("2026-11-11".into()),
            places: 2,
        },
    )
    .await;
    appointment_execute(
        &f,
        f.owner,
        Uuid::new_v4(),
        record(spec.id, agreement.id, 1, "First"),
    )
    .await
    .unwrap();
    let (a, b) = tokio::join!(
        appointment_execute(
            &f,
            f.owner,
            Uuid::new_v4(),
            record(spec.id, agreement.id, 1, "A")
        ),
        appointment_execute(
            &f,
            f.owner,
            Uuid::new_v4(),
            record(spec.id, agreement.id, 1, "B")
        )
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(a, Err(BookingError::OfferFull)) || matches!(b, Err(BookingError::OfferFull)));
    let x = saved_offer(
        &f,
        spec.id,
        interval("2026-11-11T10:00", "2026-11-11T11:00"),
    )
    .await;
    let y = saved_offer(
        &f,
        spec.id,
        interval("2026-11-11T10:30", "2026-11-11T11:30"),
    )
    .await;
    let (a, b) = tokio::join!(
        appointment_execute(&f, f.owner, Uuid::new_v4(), record(spec.id, x.id, 1, "X")),
        appointment_execute(&f, f.owner, Uuid::new_v4(), record(spec.id, y.id, 1, "Y"))
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(
        matches!(a, Err(BookingError::SpecialistBusy))
            || matches!(b, Err(BookingError::SpecialistBusy))
    );
    let winner = if a.is_ok() { x.id } else { y.id };
    let other = if winner == x.id { y.id } else { x.id };
    let offers = f.offers(spec.id, "2026-11-11").await;
    assert_eq!(
        offers
            .iter()
            .find(|o| o.id == other)
            .unwrap()
            .blocked_by_booking,
        Some(true)
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn appointments_start_only_checks_both_directions_and_half_open_boundaries() {
    let f = Fixture::new("Europe/Moscow").await;
    let spec = f.specialist().await;
    let point = saved_offer(
        &f,
        spec.id,
        Timing::StartOnly {
            starts_local: "2026-11-12T10:30".into(),
        },
    )
    .await;
    let range = saved_offer(
        &f,
        spec.id,
        interval("2026-11-12T10:00", "2026-11-12T11:00"),
    )
    .await;
    let end = saved_offer(
        &f,
        spec.id,
        Timing::StartOnly {
            starts_local: "2026-11-12T11:00".into(),
        },
    )
    .await;
    let point_a = appointment_execute(
        &f,
        f.owner,
        Uuid::new_v4(),
        record(spec.id, point.id, 1, "Point"),
    )
    .await
    .unwrap();
    assert!(matches!(
        appointment_execute(
            &f,
            f.owner,
            Uuid::new_v4(),
            record(spec.id, range.id, 1, "Range")
        )
        .await,
        Err(BookingError::SpecialistBusy)
    ));
    appointment_execute(
        &f,
        f.owner,
        Uuid::new_v4(),
        AppointmentCmd::CancelAppointment {
            id: point_a.id,
            specialist_id: spec.id,
            expected_version: 1,
        },
    )
    .await
    .unwrap();
    appointment_execute(
        &f,
        f.owner,
        Uuid::new_v4(),
        record(spec.id, range.id, 1, "Range"),
    )
    .await
    .unwrap();
    assert!(matches!(
        appointment_execute(
            &f,
            f.owner,
            Uuid::new_v4(),
            record(spec.id, point.id, 1, "Point")
        )
        .await,
        Err(BookingError::SpecialistBusy)
    ));
    appointment_execute(
        &f,
        f.owner,
        Uuid::new_v4(),
        record(spec.id, end.id, 1, "Adjacent"),
    )
    .await
    .unwrap();
    let unknown = saved_offer(
        &f,
        spec.id,
        Timing::StartOnly {
            starts_local: "2026-11-12T11:05".into(),
        },
    )
    .await;
    appointment_execute(
        &f,
        f.owner,
        Uuid::new_v4(),
        record(spec.id, unknown.id, 1, "Unknown duration"),
    )
    .await
    .unwrap();
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn appointments_move_is_atomic_and_cancel_move_race_uses_versions() {
    let f = Fixture::new("Europe/Moscow").await;
    let spec = f.specialist().await;
    let x = saved_offer(
        &f,
        spec.id,
        interval("2026-11-13T10:00", "2026-11-13T10:30"),
    )
    .await;
    let y = saved_offer(
        &f,
        spec.id,
        interval("2026-11-13T11:00", "2026-11-13T11:30"),
    )
    .await;
    let a = appointment_execute(
        &f,
        f.owner,
        Uuid::new_v4(),
        record(spec.id, x.id, 1, "Moving"),
    )
    .await
    .unwrap();
    let move_cmd = AppointmentCmd::MoveAppointment {
        id: a.id,
        specialist_id: spec.id,
        offer_id: y.id,
        expected_offer_version: 1,
        expected_version: 1,
    };
    let (moved, booked) = tokio::join!(
        appointment_execute(&f, f.owner, Uuid::new_v4(), move_cmd.clone()),
        appointment_execute(
            &f,
            f.owner,
            Uuid::new_v4(),
            record(spec.id, y.id, 1, "Competitor")
        )
    );
    assert_eq!(usize::from(moved.is_ok()) + usize::from(booked.is_ok()), 1);
    let current = appointments::get(&f.runtime, f.owner, f.tenant, f.point, a.id)
        .await
        .unwrap();
    if moved.is_ok() {
        assert_eq!(current.offer_id, y.id);
        assert_eq!(current.id, a.id);
    } else {
        assert_eq!(current.offer_id, x.id);
        assert_eq!(current.version, 1);
    }
    let z = saved_offer(
        &f,
        spec.id,
        Timing::ByAgreement {
            date: None,
            places: 3,
        },
    )
    .await;
    let (cancel, moved) = tokio::join!(
        appointment_execute(
            &f,
            f.owner,
            Uuid::new_v4(),
            AppointmentCmd::CancelAppointment {
                id: a.id,
                specialist_id: spec.id,
                expected_version: current.version
            }
        ),
        appointment_execute(
            &f,
            f.owner,
            Uuid::new_v4(),
            AppointmentCmd::MoveAppointment {
                id: a.id,
                specialist_id: spec.id,
                offer_id: z.id,
                expected_offer_version: 1,
                expected_version: current.version
            }
        )
    );
    assert_eq!(usize::from(cancel.is_ok()) + usize::from(moved.is_ok()), 1);
    assert!(
        matches!(cancel, Err(BookingError::Core(CoreError::VersionConflict)))
            || matches!(moved, Err(BookingError::Core(CoreError::VersionConflict)))
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn appointments_occupied_offer_guards_time_capacity_and_keeps_closed_bookings() {
    let f = Fixture::new("Europe/Moscow").await;
    let spec = f.specialist().await;
    let o = saved_offer(
        &f,
        spec.id,
        Timing::ByAgreement {
            date: Some("2026-11-14".into()),
            places: 3,
        },
    )
    .await;
    let a = appointment_execute(&f, f.owner, Uuid::new_v4(), record(spec.id, o.id, 1, "One"))
        .await
        .unwrap();
    appointment_execute(&f, f.owner, Uuid::new_v4(), record(spec.id, o.id, 1, "Two"))
        .await
        .unwrap();
    let edit = |places, date, state| Command::SaveOffer {
        id: o.id,
        specialist_id: spec.id,
        time_zone: o.time_zone.clone(),
        title: "Renamed".into(),
        timing: Timing::ByAgreement {
            date: Some(date),
            places,
        },
        state,
        expected_version: 1,
        conflict_token: None,
    };
    for cmd in [
        edit(1, "2026-11-14".into(), OfferState::Ready),
        edit(3, "2026-11-15".into(), OfferState::Ready),
    ] {
        assert!(matches!(
            bookings::execute(&f.runtime, f.owner, f.tenant, f.point, Uuid::new_v4(), cmd).await,
            Err(BookingError::OfferOccupied)
        ));
    }
    f.execute(edit(4, "2026-11-14".into(), OfferState::Withdrawn))
        .await;
    let visible = bookings::offers(
        &f.runtime,
        f.owner,
        f.tenant,
        f.point,
        bookings::OfferPage {
            specialist: spec.id,
            date: "2026-11-14",
            after: None,
            limit: 10,
            include_withdrawn: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].state, OfferState::Withdrawn);
    assert!(matches!(
        appointment_execute(
            &f,
            f.owner,
            Uuid::new_v4(),
            record(spec.id, o.id, 2, "Three")
        )
        .await,
        Err(BookingError::OfferUnavailable)
    ));
    let old = appointments::get(&f.runtime, f.owner, f.tenant, f.point, a.id)
        .await
        .unwrap();
    assert_eq!(old.title, "Visit");
    assert_eq!(old.date.as_deref(), Some("2026-11-14"));
    let updated = appointment_execute(
        &f,
        f.owner,
        Uuid::new_v4(),
        AppointmentCmd::UpdateClient {
            id: a.id,
            specialist_id: spec.id,
            expected_version: 1,
            client: client("Updated"),
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.title, "Visit");
    assert_eq!(updated.client_name, "Updated");
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn appointments_operator_permissions_rls_scope_and_replay_revocation() {
    let f = Fixture::new("Europe/Moscow").await;
    let spec = f.specialist().await;
    let operator = f.member(Role::Operator).await;
    let employee = f.member(Role::Employee).await;
    let o = saved_offer(
        &f,
        spec.id,
        Timing::ByAgreement {
            date: None,
            places: 3,
        },
    )
    .await;
    let cmd = record(spec.id, o.id, 1, "Private Client");
    let key = Uuid::new_v4();
    let a = appointment_execute(&f, operator, key, cmd.clone())
        .await
        .unwrap();
    assert_eq!(a.created_by, Some(operator));
    assert!(matches!(
        appointments::get(&f.runtime, employee, f.tenant, f.point, a.id).await,
        Err(BookingError::Core(CoreError::Forbidden))
    ));
    assert!(matches!(
        appointment_execute(
            &f,
            employee,
            Uuid::new_v4(),
            record(spec.id, o.id, 1, "Forbidden")
        )
        .await,
        Err(BookingError::Core(CoreError::Forbidden))
    ));
    let employee_offers = bookings::offers(
        &f.runtime,
        employee,
        f.tenant,
        f.point,
        bookings::OfferPage {
            specialist: spec.id,
            date: "2026-11-14",
            after: None,
            limit: 10,
            include_withdrawn: false,
        },
    )
    .await
    .unwrap();
    assert!(
        employee_offers[0].reserved_places.is_none()
            && employee_offers[0].blocked_by_booking.is_none()
    );
    let mut s = TenantSession::begin(&f.runtime, employee, f.tenant)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM bookings.appointments")
        .fetch_one(s.connection())
        .await
        .unwrap();
    assert_eq!(count, 0);
    s.commit().await.unwrap();
    let other = Fixture::new("Europe/Moscow").await;
    assert!(matches!(
        appointments::get(&f.runtime, f.owner, f.tenant, other.point, a.id).await,
        Err(BookingError::NotFound)
    ));
    sqlx::query("UPDATE core.memberships SET role='employee' WHERE tenant_id=$1 AND user_id=$2")
        .bind(f.tenant)
        .bind(operator)
        .execute(&f.migrator)
        .await
        .unwrap();
    assert!(matches!(
        appointment_execute(&f, operator, key, cmd).await,
        Err(BookingError::Core(CoreError::Forbidden))
    ));
    other.close().await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn appointments_http_private_roles_csrf_strict_payload_pagination_and_validation() {
    let f = Fixture::new("Europe/Moscow").await;
    let spec = f.specialist().await;
    let o = saved_offer(
        &f,
        spec.id,
        Timing::ByAgreement {
            date: None,
            places: 5,
        },
    )
    .await;
    let operator = f.member(Role::Operator).await;
    let login = f.login(operator).await;
    let employee = f.member(Role::Employee).await;
    let employee_login = f.login(employee).await;
    let body = serde_json::to_value(record(spec.id, o.id, 1, "HTTP client")).unwrap();
    let (status, _) = f
        .json(
            Request::post(f.url("appointments/commands"))
                .header("origin", "http://127.0.0.1:8080")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let mut missing = appointment_post(&f, &login, Uuid::new_v4(), body.clone());
    missing.headers_mut().remove("x-csrf-token");
    assert_eq!(f.json(missing).await.0, StatusCode::FORBIDDEN);
    let mut spoof = body.clone();
    spoof["status"] = json!("confirmed");
    assert_eq!(
        f.json(appointment_post(&f, &login, Uuid::new_v4(), spoof))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let mut invalid = body.clone();
    invalid["client"]["email"] = json!("bad");
    let (status, result) = f
        .json(appointment_post(&f, &login, Uuid::new_v4(), invalid))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(result["code"], "invalid_client");
    for _ in 0..3 {
        let body = serde_json::to_value(record(spec.id, o.id, 1, "HTTP client")).unwrap();
        assert_eq!(
            f.json(appointment_post(&f, &login, Uuid::new_v4(), body))
                .await
                .0,
            StatusCode::OK
        );
    }
    let path = format!(
        "appointments?specialist_id={}&date=2026-11-15&limit=1",
        spec.id
    );
    let (status, page) = f
        .json(
            Request::get(f.url(&path))
                .header("cookie", &login.0)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert!(page["next_cursor"].is_string());
    let (status, employee_page) = f
        .json(
            Request::get(f.url(&path))
                .header("cookie", &employee_login.0)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!employee_page.to_string().contains("HTTP client"));
    let id = page["items"][0]["id"].as_str().unwrap();
    assert_eq!(
        f.json(
            Request::get(f.url(&format!("appointments/{id}")))
                .header("cookie", &login.0)
                .body(Body::empty())
                .unwrap()
        )
        .await
        .0,
        StatusCode::OK
    );
    let bad = format!("{path}&private=true");
    assert_eq!(
        f.json(
            Request::get(f.url(&bad))
                .header("cookie", &login.0)
                .body(Body::empty())
                .unwrap()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    f.close().await;
}
struct Fixture {
    runtime: PgPool,
    admin: PgPool,
    migrator: PgPool,
    tenant: TenantId,
    owner: UserId,
    point: LocationId,
    app: Router,
}
impl Fixture {
    async fn new(zone: &str) -> Self {
        let runtime = pool("TEST_DATABASE_URL", 5).await;
        let admin = pool("TEST_ADMIN_DATABASE_URL", 2).await;
        let migrator = pool("TEST_MIGRATION_DATABASE_URL", 1).await;
        let org = Provisioner::new(admin.clone())
            .create_organization(
                &Name::parse("Booking test").unwrap(),
                &Name::parse("Owner").unwrap(),
            )
            .await
            .unwrap();
        let mut session = TenantSession::begin(&runtime, org.owner.id, org.organization.id)
            .await
            .unwrap();
        let point = session
            .create_location(&Name::parse("Clinic").unwrap(), zone)
            .await
            .unwrap()
            .id;
        session.commit().await.unwrap();
        let app = router(AppState::new(runtime.clone(), Duration::from_secs(1)));
        Self {
            runtime,
            admin,
            migrator,
            tenant: org.organization.id,
            owner: org.owner.id,
            point,
            app,
        }
    }
    async fn execute(&self, command: Command) -> bookings::BookingReceipt {
        bookings::execute(
            &self.runtime,
            self.owner,
            self.tenant,
            self.point,
            Uuid::new_v4(),
            command,
        )
        .await
        .unwrap()
    }
    async fn specialist(&self) -> bookings::Specialist {
        let receipt = self
            .execute(Command::SaveSpecialist {
                id: Uuid::new_v4(),
                name: " Doctor ".into(),
                description: "Care".into(),
                enabled: true,
                expected_version: 0,
            })
            .await;
        match receipt.resource {
            BookingResource::Specialist(p) => p,
            _ => panic!("specialist expected"),
        }
    }
    async fn offers(&self, specialist: Uuid, date: &str) -> Vec<bookings::Offer> {
        bookings::offers(
            &self.runtime,
            self.owner,
            self.tenant,
            self.point,
            bookings::OfferPage {
                specialist,
                date,
                after: None,
                limit: 100,
                include_withdrawn: true,
            },
        )
        .await
        .unwrap()
    }
    async fn member(&self, role: Role) -> UserId {
        let provisioner = Provisioner::new(self.admin.clone());
        let user = provisioner
            .create_user(&Name::parse("Member").unwrap())
            .await
            .unwrap()
            .id;
        provisioner
            .add_membership(self.tenant, user, role)
            .await
            .unwrap();
        user
    }
    async fn login(&self, user: UserId) -> (String, String) {
        let email = Email::parse(&format!("booking-{}@example.test", Uuid::new_v4())).unwrap();
        login::bind_email(&self.admin, user, &email).await.unwrap();
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes).unwrap();
        let code = hex::encode(bytes);
        let hash: [u8; 32] = Sha256::digest(bytes).into();
        login::issue_login_code(&self.admin, &email, &hash, 600)
            .await
            .unwrap();
        let response = self
            .app
            .clone()
            .oneshot(
                Request::post("/auth/login")
                    .header("origin", "http://127.0.0.1:8080")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"code":code}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let cookies: Vec<_> = response
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .map(|c| c.to_str().unwrap().split(';').next().unwrap().to_owned())
            .collect();
        let csrf = cookies
            .iter()
            .find_map(|c| c.strip_prefix("turnesta_csrf="))
            .unwrap()
            .to_owned();
        (cookies.join("; "), csrf)
    }
    fn url(&self, path: &str) -> String {
        format!(
            "/api/organizations/{}/locations/{}/bookings/{path}",
            self.tenant, self.point
        )
    }
    async fn json(&self, request: Request<Body>) -> (StatusCode, Value) {
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }
    async fn close(self) {
        self.runtime.close().await;
        self.admin.close().await;
        self.migrator.close().await;
    }
}
fn offer(specialist_id: Uuid, timing: Timing) -> Command {
    Command::SaveOffer {
        id: Uuid::new_v4(),
        specialist_id,
        time_zone: "Europe/Moscow".into(),
        title: "Visit".into(),
        timing,
        state: OfferState::Ready,
        expected_version: 0,
        conflict_token: None,
    }
}
fn interval(start: &str, end: &str) -> Timing {
    Timing::Interval {
        starts_local: start.into(),
        ends_local: end.into(),
    }
}

#[test]
fn booking_timing_rejects_incoherent_payloads_and_allows_no_date() {
    for invalid in [
        json!({"mode":"start_only","starts_local":"2026-11-05T10:00","ends_local":"2026-11-05T11:00"}),
        json!({"mode":"by_agreement","places":2,"starts_local":"2026-11-05T10:00"}),
        json!({"mode":"interval","starts_local":"2026-11-05T10:00"}),
    ] {
        assert!(serde_json::from_value::<Timing>(invalid).is_err());
    }
    assert_eq!(
        serde_json::from_value::<Timing>(json!({"mode":"by_agreement","places":10})).unwrap(),
        Timing::ByAgreement {
            date: None,
            places: 10
        }
    );
}

#[test]
fn booking_commands_require_timezone_and_reject_actor_spoofing() {
    let original = offer(
        Uuid::new_v4(),
        Timing::ByAgreement {
            date: None,
            places: 1,
        },
    );
    let mut body = serde_json::to_value(&original).unwrap();
    body.as_object_mut().unwrap().remove("time_zone");
    assert!(serde_json::from_value::<Command>(body).is_err());
    let mut body = serde_json::to_value(&original).unwrap();
    body["actor_id"] = json!(Uuid::new_v4());
    assert!(serde_json::from_value::<Command>(body).is_err());
}
fn post(f: &Fixture, login: &(String, String), key: Uuid, body: Value) -> Request<Body> {
    Request::post(f.url("commands"))
        .header("cookie", &login.0)
        .header("origin", "http://127.0.0.1:8080")
        .header("x-csrf-token", &login.1)
        .header("idempotency-key", key.to_string())
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn booking_manual_modes_presets_and_timezone_snapshots_are_independent() {
    let f = Fixture::new("Europe/Moscow").await;
    let specialist = f.specialist().await;
    assert_eq!(specialist.name, "Doctor");
    let preset_id = Uuid::new_v4();
    f.execute(Command::SavePreset {
        id: preset_id,
        name: "Procedure".into(),
        duration_minutes: Some(90),
        enabled: true,
        expected_version: 0,
    })
    .await;
    f.execute(offer(
        specialist.id,
        interval("2026-11-05T10:00", "2026-11-05T11:30"),
    ))
    .await;
    f.execute(offer(
        specialist.id,
        Timing::StartOnly {
            starts_local: "2026-11-05T12:00".into(),
        },
    ))
    .await;
    f.execute(offer(
        specialist.id,
        Timing::ByAgreement {
            date: None,
            places: 10,
        },
    ))
    .await;
    f.execute(offer(
        specialist.id,
        Timing::ByAgreement {
            date: Some("2026-11-05".into()),
            places: 3,
        },
    ))
    .await;
    f.execute(Command::SavePreset {
        id: preset_id,
        name: "Renamed".into(),
        duration_minutes: Some(30),
        enabled: false,
        expected_version: 1,
    })
    .await;
    let rows = f.offers(specialist.id, "2026-11-05").await;
    assert_eq!(rows.len(), 4);
    let timed = rows.iter().find(|v| v.mode == "interval").unwrap();
    assert_eq!(timed.starts_at.as_deref(), Some("2026-11-05T07:00:00Z"));
    assert_eq!(timed.ends_local.as_deref(), Some("2026-11-05T11:30"));
    assert!(
        rows.iter()
            .find(|v| v.mode == "start_only")
            .unwrap()
            .ends_at
            .is_none()
    );
    assert_eq!(
        f.offers(specialist.id, "2026-11-06").await.len(),
        1,
        "undated offers remain discoverable on every day"
    );
    let mut session = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    let point = session
        .locations()
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.id == f.point)
        .unwrap();
    session
        .update_location(
            f.point,
            &Name::parse(&point.name).unwrap(),
            "America/New_York",
            point.version,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let mut edit = offer(
        specialist.id,
        interval("2026-11-05T10:00", "2026-11-05T11:30"),
    );
    if let Command::SaveOffer {
        id,
        expected_version,
        ..
    } = &mut edit
    {
        *id = timed.id;
        *expected_version = timed.version;
    }
    let result = f.execute(edit).await;
    let BookingResource::Offer(result) = result.resource else {
        panic!()
    };
    assert_eq!(result.time_zone, "Europe/Moscow");
    assert_eq!(result.starts_at, timed.starts_at);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn booking_dst_gaps_folds_and_invalid_combinations_reject_without_partial_writes() {
    let f = Fixture::new("America/New_York").await;
    let s = f.specialist().await;
    let local_offer = |specialist, timing| {
        let mut command = offer(specialist, timing);
        if let Command::SaveOffer { time_zone, .. } = &mut command {
            *time_zone = "America/New_York".into();
        }
        command
    };
    for (time, code) in [
        ("2026-03-08T02:30", "gap"),
        ("2026-11-01T01:30", "fold"),
        ("2026-02-30T12:00", "invalid"),
        ("2026-11-05T24:00", "invalid"),
    ] {
        let result = bookings::execute(
            &f.runtime,
            f.owner,
            f.tenant,
            f.point,
            Uuid::new_v4(),
            local_offer(
                s.id,
                Timing::StartOnly {
                    starts_local: time.into(),
                },
            ),
        )
        .await;
        assert!(
            matches!(
                (&result, code),
                (Err(BookingError::TimeGap), "gap")
                    | (Err(BookingError::TimeFold), "fold")
                    | (Err(BookingError::Invalid), "invalid")
            ),
            "{time}: {result:?}"
        );
    }
    for timing in [
        interval("2026-11-05T12:00", "2026-11-05T11:00"),
        Timing::ByAgreement {
            date: Some("2026-02-30".into()),
            places: 1,
        },
        Timing::ByAgreement {
            date: None,
            places: 0,
        },
    ] {
        assert!(matches!(
            bookings::execute(
                &f.runtime,
                f.owner,
                f.tenant,
                f.point,
                Uuid::new_v4(),
                local_offer(s.id, timing)
            )
            .await,
            Err(BookingError::Invalid)
        ));
    }
    assert!(f.offers(s.id, "2026-11-05").await.is_empty());
    let mut checked = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM bookings.commands")
            .fetch_one(checked.connection())
            .await
            .unwrap(),
        1,
        "only the specialist command committed"
    );
    checked.commit().await.unwrap();
    f.execute(local_offer(
        s.id,
        interval("2026-11-05T23:30", "2026-11-06T00:30"),
    ))
    .await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn booking_overlap_requires_current_ack_and_preserves_alternative_offers() {
    let f = Fixture::new("Europe/Moscow").await;
    let s = f.specialist().await;
    f.execute(offer(
        s.id,
        interval("2026-11-05T10:00", "2026-11-05T11:00"),
    ))
    .await;
    let mut second = offer(
        s.id,
        Timing::StartOnly {
            starts_local: "2026-11-05T10:30".into(),
        },
    );
    let err = bookings::execute(
        &f.runtime,
        f.owner,
        f.tenant,
        f.point,
        Uuid::new_v4(),
        second.clone(),
    )
    .await
    .unwrap_err();
    let BookingError::Overlap(first) = err else {
        panic!("overlap expected")
    };
    assert_eq!(first.total, 1);
    if let Command::SaveOffer { conflict_token, .. } = &mut second {
        *conflict_token = Some(first.token);
    }
    // Even unrelated changes of this specialist invalidate the prior acknowledgement.
    f.execute(offer(
        s.id,
        Timing::ByAgreement {
            date: None,
            places: 2,
        },
    ))
    .await;
    let err = bookings::execute(
        &f.runtime,
        f.owner,
        f.tenant,
        f.point,
        Uuid::new_v4(),
        second.clone(),
    )
    .await
    .unwrap_err();
    let BookingError::Overlap(current) = err else {
        panic!("fresh overlap expected")
    };
    if let Command::SaveOffer { conflict_token, .. } = &mut second {
        *conflict_token = Some(current.token);
    }
    f.execute(second).await;
    // Half-open endpoint is not a conflict, nor are different unknown durations.
    f.execute(offer(
        s.id,
        Timing::StartOnly {
            starts_local: "2026-11-05T11:00".into(),
        },
    ))
    .await;
    f.execute(offer(
        s.id,
        Timing::StartOnly {
            starts_local: "2026-11-05T12:00".into(),
        },
    ))
    .await;
    assert_eq!(f.offers(s.id, "2026-11-05").await.len(), 5);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn booking_concurrent_overlap_and_idempotent_commands_serialize() {
    let f = Fixture::new("Europe/Moscow").await;
    let s = f.specialist().await;
    let a = offer(s.id, interval("2026-11-05T10:00", "2026-11-05T11:00"));
    let b = offer(s.id, interval("2026-11-05T10:30", "2026-11-05T11:30"));
    let (a, b) = tokio::join!(
        bookings::execute(&f.runtime, f.owner, f.tenant, f.point, Uuid::new_v4(), a),
        bookings::execute(&f.runtime, f.owner, f.tenant, f.point, Uuid::new_v4(), b)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(
        matches!(a, Err(BookingError::Overlap(_))) || matches!(b, Err(BookingError::Overlap(_)))
    );
    let key = Uuid::new_v4();
    let command = offer(
        s.id,
        Timing::ByAgreement {
            date: None,
            places: 10,
        },
    );
    let (a, b) = tokio::join!(
        bookings::execute(&f.runtime, f.owner, f.tenant, f.point, key, command.clone()),
        bookings::execute(&f.runtime, f.owner, f.tenant, f.point, key, command.clone())
    );
    let original = a.unwrap();
    assert_eq!(
        serde_json::to_value(&original).unwrap(),
        serde_json::to_value(b.unwrap()).unwrap()
    );
    let mut changed = command.clone();
    if let Command::SaveOffer { title, .. } = &mut changed {
        *title = "Changed".into();
    }
    assert!(matches!(
        bookings::execute(&f.runtime, f.owner, f.tenant, f.point, key, changed).await,
        Err(BookingError::IdempotencyConflict)
    ));
    let BookingResource::Offer(created) = original.resource else {
        panic!()
    };
    let id = created.id;
    let edit = |title: &str| Command::SaveOffer {
        id,
        specialist_id: s.id,
        time_zone: "Europe/Moscow".into(),
        title: title.into(),
        timing: Timing::ByAgreement {
            date: None,
            places: 10,
        },
        state: OfferState::Draft,
        expected_version: 1,
        conflict_token: None,
    };
    let (a, b) = tokio::join!(
        bookings::execute(
            &f.runtime,
            f.owner,
            f.tenant,
            f.point,
            Uuid::new_v4(),
            edit("A")
        ),
        bookings::execute(
            &f.runtime,
            f.owner,
            f.tenant,
            f.point,
            Uuid::new_v4(),
            edit("B")
        )
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(
        matches!(a, Err(BookingError::Core(CoreError::VersionConflict)))
            || matches!(b, Err(BookingError::Core(CoreError::VersionConflict)))
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn booking_rls_roles_composite_links_and_disabled_specialist_are_enforced() {
    let f = Fixture::new("Europe/Moscow").await;
    let other = Fixture::new("Europe/Moscow").await;
    let s = f.specialist().await;
    let foreign = other.specialist().await;
    for role in [Role::Operator, Role::Employee] {
        let user = f.member(role).await;
        assert_eq!(
            bookings::specialists(&f.runtime, user, f.tenant, f.point, None, 50)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(matches!(
            bookings::execute(
                &f.runtime,
                user,
                f.tenant,
                f.point,
                Uuid::new_v4(),
                offer(
                    s.id,
                    Timing::ByAgreement {
                        date: None,
                        places: 1
                    }
                )
            )
            .await,
            Err(BookingError::Core(CoreError::Forbidden))
        ));
        let mut tx = TenantSession::begin(&f.runtime, user, f.tenant)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query("UPDATE bookings.specialists SET enabled=false")
                .execute(tx.connection())
                .await
                .unwrap()
                .rows_affected(),
            0
        );
        tx.commit().await.unwrap();
    }
    let mut tx = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.tenant_id',$1,true)")
        .bind(other.tenant.to_string())
        .execute(tx.connection())
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM bookings.specialists")
            .fetch_one(tx.connection())
            .await
            .unwrap(),
        0
    );
    tx.commit().await.unwrap();
    assert!(matches!(
        bookings::execute(
            &f.runtime,
            f.owner,
            f.tenant,
            f.point,
            Uuid::new_v4(),
            offer(
                foreign.id,
                Timing::ByAgreement {
                    date: None,
                    places: 1
                }
            )
        )
        .await,
        Err(BookingError::NotFound)
    ));
    let mut tx = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    let error=sqlx::query("INSERT INTO bookings.offers(tenant_id,location_id,specialist_id,id,title,mode,time_zone,places,state) VALUES($1,$2,$3,$4,'','by_agreement','Europe/Moscow',1,'ready')").bind(f.tenant).bind(f.point).bind(foreign.id).bind(Uuid::new_v4()).execute(tx.connection()).await.unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23503")
    );
    drop(tx);
    f.execute(Command::SaveSpecialist {
        id: s.id,
        name: s.name,
        description: s.description,
        enabled: false,
        expected_version: 1,
    })
    .await;
    assert!(matches!(
        bookings::execute(
            &f.runtime,
            f.owner,
            f.tenant,
            f.point,
            Uuid::new_v4(),
            offer(
                s.id,
                Timing::ByAgreement {
                    date: None,
                    places: 1
                }
            )
        )
        .await,
        Err(BookingError::SpecialistDisabled)
    ));
    let mut draft = offer(
        s.id,
        Timing::ByAgreement {
            date: None,
            places: 1,
        },
    );
    if let Command::SaveOffer { state, .. } = &mut draft {
        *state = OfferState::Draft;
    }
    f.execute(draft).await;
    other.close().await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn booking_http_auth_csrf_strict_payloads_overlap_and_pagination() {
    let f = Fixture::new("Europe/Moscow").await;
    let s = f.specialist().await;
    let login = f.login(f.owner).await;
    assert_eq!(
        f.json(
            Request::get(f.url("specialists"))
                .body(Body::empty())
                .unwrap()
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let cmd = offer(s.id, interval("2026-11-05T10:00", "2026-11-05T11:00"));
    let body = serde_json::to_value(&cmd).unwrap();
    let mut no_origin = post(&f, &login, Uuid::new_v4(), body.clone());
    no_origin.headers_mut().remove("origin");
    assert_eq!(f.json(no_origin).await.0, StatusCode::FORBIDDEN);
    let mut extra = body.clone();
    extra["actor_id"] = json!(f.owner);
    assert_eq!(
        f.json(post(&f, &login, Uuid::new_v4(), extra)).await.0,
        StatusCode::BAD_REQUEST
    );
    let (status, receipt) = f.json(post(&f, &login, Uuid::new_v4(), body.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(receipt["resource"]["kind"], "offer");
    let mut second = body;
    second["id"] = json!(Uuid::new_v4());
    let (status, error) = f
        .json(post(&f, &login, Uuid::new_v4(), second.clone()))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["code"], "booking_overlap");
    assert_eq!(error["details"]["total"], 1);
    second["conflict_token"] = error["details"]["token"].clone();
    assert_eq!(
        f.json(post(&f, &login, Uuid::new_v4(), second)).await.0,
        StatusCode::OK
    );
    for suffix in ["?limit=0", "?limit=1&limit=2", "?unknown=1"] {
        assert_eq!(
            f.json(
                Request::get(f.url(&format!("specialists{suffix}")))
                    .header("cookie", &login.0)
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let url = f.url(&format!(
        "offers?specialist_id={}&date=2026-11-05&limit=1",
        s.id
    ));
    let (status, page) = f
        .json(
            Request::get(&url)
                .header("cookie", &login.0)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let next = page["next_cursor"].as_str().unwrap();
    let (_, page2) = f
        .json(
            Request::get(format!("{url}&after={next}"))
                .header("cookie", &login.0)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_ne!(page["items"][0]["id"], page2["items"][0]["id"]);
    assert!(page2["next_cursor"].is_null());
    let invalid = f.url(&format!("offers?specialist_id={}&date=2026-02-30", s.id));
    assert_eq!(
        f.json(
            Request::get(invalid)
                .header("cookie", &login.0)
                .body(Body::empty())
                .unwrap()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn booking_duration_uses_elapsed_time_and_changed_timezone_requires_review() {
    let f = Fixture::new("America/New_York").await;
    let result = bookings::suggest_time(
        &f.runtime,
        f.owner,
        f.tenant,
        f.point,
        bookings::TimeRequest {
            starts_local: "2026-03-08T01:30",
            minutes: 120,
            time_zone: "America/New_York",
            offer_id: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        result.ends_local, "2026-03-08T04:30",
        "two elapsed hours across spring clock change"
    );
    assert!(
        matches!(
            bookings::suggest_time(
                &f.runtime,
                f.owner,
                f.tenant,
                f.point,
                bookings::TimeRequest {
                    starts_local: "2026-11-01T00:30",
                    minutes: 60,
                    time_zone: "America/New_York",
                    offer_id: None
                }
            )
            .await,
            Err(BookingError::TimeFold)
        ),
        "ambiguous suggested end is not silently selected"
    );
    let s = f.specialist().await;
    assert!(matches!(
        bookings::execute(
            &f.runtime,
            f.owner,
            f.tenant,
            f.point,
            Uuid::new_v4(),
            offer(
                s.id,
                Timing::StartOnly {
                    starts_local: "2026-11-05T12:00".into()
                }
            )
        )
        .await,
        Err(BookingError::TimeZoneChanged)
    ));
    let login = f.login(f.owner).await;
    let url = f.url(
        "suggest-time?starts_local=2026-03-08T01%3A30&minutes=120&time_zone=America%2FNew_York",
    );
    let (status, body) = f
        .json(
            Request::get(url)
                .header("cookie", &login.0)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ends_local"], "2026-03-08T04:30");
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn booking_database_mode_constraints_and_independent_specialists() {
    let f = Fixture::new("Europe/Moscow").await;
    let a = f.specialist().await;
    let b = f.specialist().await;
    let mut held = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM bookings.specialists WHERE tenant_id=$1 AND id=$2 FOR UPDATE")
        .bind(f.tenant)
        .bind(a.id)
        .execute(held.connection())
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(1),
        bookings::execute(
            &f.runtime,
            f.owner,
            f.tenant,
            f.point,
            Uuid::new_v4(),
            offer(
                b.id,
                Timing::StartOnly {
                    starts_local: "2026-11-05T12:00".into(),
                },
            ),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    held.commit().await.unwrap();
    let mut tx = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    let error=sqlx::query("INSERT INTO bookings.offers(tenant_id,location_id,specialist_id,id,title,mode,time_zone,places,state) VALUES($1,$2,$3,$4,'','interval','Europe/Moscow',1,'ready')")
        .bind(f.tenant).bind(f.point).bind(a.id).bind(Uuid::new_v4()).execute(tx.connection()).await.unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    drop(tx);
    let mut tx = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    let other = tx
        .create_location(&Name::parse("Another point").unwrap(), "Europe/Moscow")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(matches!(
        bookings::execute(
            &f.runtime,
            f.owner,
            f.tenant,
            other.id,
            Uuid::new_v4(),
            offer(
                a.id,
                Timing::ByAgreement {
                    date: None,
                    places: 1
                }
            )
        )
        .await,
        Err(BookingError::NotFound)
    ));
    let denied:bool=sqlx::query_scalar("SELECT NOT has_schema_privilege('turnesta_worker','bookings','USAGE') AND NOT has_table_privilege('turnesta_runtime','bookings.offers','DELETE')").fetch_one(&f.runtime).await.unwrap();
    assert!(denied);
    f.close().await;
}

fn future_day(zone: &str, days: i64) -> String {
    jiff::Timestamp::now()
        .checked_add(jiff::SignedDuration::from_hours(days * 24))
        .unwrap()
        .in_tz(zone)
        .unwrap()
        .strftime("%Y-%m-%d")
        .to_string()
}
fn public_input(offer: &bookings::Offer) -> PublicBookingInput {
    PublicBookingInput {
        offer_id: offer.id,
        offer_version: offer.version,
        specialist_version: 1,
        form_version: 1,
        time_zone: offer.time_zone.clone(),
        answers: std::collections::BTreeMap::from([
            ("client_name".into(), "Private Customer".into()),
            ("phone".into(), "+7 (900) 000-00-00".into()),
            ("email".into(), "private@example.test".into()),
            ("note".into(), "Private note".into()),
        ]),
    }
}
async fn publish(f: &Fixture, form: BookingForm) -> Uuid {
    public_bookings::configure(&f.runtime, f.owner, f.tenant, f.point, true, form, 0)
        .await
        .unwrap()
        .public_slug
        .unwrap()
}
async fn public_day(
    f: &Fixture,
    slug: Uuid,
    date: &str,
    specialist: Option<Uuid>,
    after: Option<Uuid>,
    version: Option<&str>,
    limit: i32,
) -> Result<public_bookings::BookingAvailability, PubError> {
    public_bookings::availability(
        &f.runtime,
        slug,
        public_bookings::AvailabilityQuery {
            month: &date[..7],
            date,
            specialist,
            after,
            version,
            limit,
        },
    )
    .await
}
fn public_post(slug: Uuid, token: &str, body: Value) -> Request<Body> {
    Request::post(format!("/api/public/bookings/{slug}/receipt"))
        .header("origin", "http://127.0.0.1:8080")
        .header("x-booking-access", token)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[test]
fn public_booking_input_has_no_actor_or_client_status_fields() {
    let body = json!({"offer_id":Uuid::new_v4(),"offer_version":1,"specialist_version":1,"form_version":1,"time_zone":"UTC","answers":{"client_name":"Client"}});
    assert!(serde_json::from_value::<PublicBookingInput>(body.clone()).is_ok());
    for field in ["source", "status", "created_by", "tenant_id", "location_id"] {
        let mut bad = body.clone();
        bad[field] = json!(Uuid::new_v4());
        assert!(serde_json::from_value::<PublicBookingInput>(bad).is_err());
    }
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn public_bookings_opt_in_roles_versions_and_form_validation() {
    let f = Fixture::new("Europe/Moscow").await;
    let initial = public_bookings::settings(&f.runtime, f.owner, f.tenant, f.point)
        .await
        .unwrap();
    assert!(!initial.enabled && initial.public_slug.is_none());
    assert_eq!(initial.version, 0);
    assert!(matches!(
        public_bookings::profile(&f.runtime, Uuid::new_v4()).await,
        Err(PubError::NotFound)
    ));
    let operator = f.member(Role::Operator).await;
    assert!(matches!(
        public_bookings::configure(
            &f.runtime,
            operator,
            f.tenant,
            f.point,
            true,
            BookingForm::default(),
            0
        )
        .await,
        Err(PubError::Core(CoreError::Forbidden))
    ));
    for fields in [
        vec![],
        vec![BookingFormField {
            key: "client_name".into(),
            label: "Name".into(),
            kind: BookingFieldKind::ClientName,
            required: false,
            options: vec![],
        }],
        vec![BookingFormField {
            key: "field_custom".into(),
            label: "Choice".into(),
            kind: BookingFieldKind::Select,
            required: true,
            options: vec![],
        }],
    ] {
        assert!(matches!(
            public_bookings::configure(
                &f.runtime,
                f.owner,
                f.tenant,
                f.point,
                true,
                BookingForm { fields },
                0
            )
            .await,
            Err(PubError::Invalid)
        ));
    }
    let slug = publish(&f, BookingForm::default()).await;
    assert!(
        public_bookings::profile(&f.runtime, slug)
            .await
            .unwrap()
            .enabled
    );
    assert!(matches!(
        public_bookings::configure(
            &f.runtime,
            f.owner,
            f.tenant,
            f.point,
            false,
            BookingForm::default(),
            0
        )
        .await,
        Err(PubError::Core(CoreError::VersionConflict))
    ));
    let paused = public_bookings::configure(
        &f.runtime,
        f.owner,
        f.tenant,
        f.point,
        false,
        BookingForm::default(),
        1,
    )
    .await
    .unwrap();
    assert_eq!(paused.public_slug, Some(slug));
    let p = public_bookings::profile(&f.runtime, slug).await.unwrap();
    assert!(!p.enabled && p.form.is_none() && p.specialists.is_empty());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn public_bookings_calendar_is_chronological_scoped_and_version_guarded() {
    let f = Fixture::new("Europe/Moscow").await;
    let spec = f.specialist().await;
    let other = f.specialist().await;
    let day = future_day("Europe/Moscow", 3);
    let late = saved_offer(
        &f,
        spec.id,
        interval(&format!("{day}T15:00"), &format!("{day}T15:30")),
    )
    .await;
    let early = saved_offer(
        &f,
        spec.id,
        interval(&format!("{day}T10:00"), &format!("{day}T10:30")),
    )
    .await;
    saved_offer(
        &f,
        other.id,
        Timing::StartOnly {
            starts_local: format!("{day}T11:00"),
        },
    )
    .await;
    saved_offer(
        &f,
        spec.id,
        Timing::ByAgreement {
            date: None,
            places: 4,
        },
    )
    .await;
    let slug = publish(&f, BookingForm::default()).await;
    let page = public_day(&f, slug, &day, Some(spec.id), None, None, 1)
        .await
        .unwrap();
    assert_eq!(page.items[0].id, early.id);
    assert_eq!(page.days[0].count, 2);
    let next = public_day(
        &f,
        slug,
        &day,
        Some(spec.id),
        page.next_cursor,
        Some(&page.version),
        1,
    )
    .await
    .unwrap();
    assert_eq!(next.items[0].id, late.id);
    let all = public_day(&f, slug, &day, None, None, None, 100)
        .await
        .unwrap();
    assert_eq!(all.items.len(), 4);
    public_bookings::book(&f.runtime, slug, &[1; 32], public_input(&early))
        .await
        .unwrap();
    assert!(matches!(
        public_day(
            &f,
            slug,
            &day,
            Some(spec.id),
            page.next_cursor,
            Some(&page.version),
            1
        )
        .await,
        Err(PubError::Changed)
    ));
    assert!(matches!(
        public_day(&f, slug, &day, Some(Uuid::new_v4()), None, None, 50).await,
        Err(PubError::Changed)
    ));
    assert!(matches!(
        public_bookings::availability(
            &f.runtime,
            slug,
            public_bookings::AvailabilityQuery {
                month: "2026-99",
                date: "2026-99-01",
                specialist: None,
                after: None,
                version: None,
                limit: 50
            }
        )
        .await,
        Err(PubError::Invalid)
    ));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn public_bookings_snapshot_answers_privacy_and_replay_after_pause() {
    let f = Fixture::new("Europe/Moscow").await;
    let spec = f.specialist().await;
    let o = saved_offer(
        &f,
        spec.id,
        Timing::ByAgreement {
            date: None,
            places: 4,
        },
    )
    .await;
    let mut form = BookingForm::default();
    form.fields.push(BookingFormField {
        key: "field_visit".into(),
        label: "Цель визита".into(),
        kind: BookingFieldKind::Select,
        required: true,
        options: vec!["Первый визит".into(), "Повторный".into()],
    });
    let slug = publish(&f, form.clone()).await;
    let mut input = public_input(&o);
    input
        .answers
        .insert("field_visit".into(), "Первый визит".into());
    let receipt = public_bookings::book(&f.runtime, slug, &[2; 32], input.clone())
        .await
        .unwrap();
    assert_eq!(receipt.status, "confirmed");
    let raw = serde_json::to_string(&receipt).unwrap();
    for private in [
        "Private Customer",
        "private@example.test",
        "Private note",
        "Первый визит",
        "client_name",
        "phone",
        "answers",
        "tenant_id",
    ] {
        assert!(!raw.contains(private));
    }
    let a = appointments::list(
        &f.runtime,
        f.owner,
        f.tenant,
        f.point,
        appointments::AppointmentPage {
            specialist: spec.id,
            date: &future_day("Europe/Moscow", 1),
            after: None,
            limit: 50,
            include_cancelled: true,
        },
    )
    .await
    .unwrap()
    .remove(0);
    assert_eq!(a.source, "public");
    assert_eq!(a.created_by, None);
    assert_eq!(a.answers.0["field_visit"], "Первый визит");
    assert!(a.form_snapshot.is_some());
    form.fields.push(BookingFormField {
        key: "field_new".into(),
        label: "Новое поле".into(),
        kind: BookingFieldKind::Text,
        required: true,
        options: vec![],
    });
    public_bookings::configure(&f.runtime, f.owner, f.tenant, f.point, false, form, 1)
        .await
        .unwrap();
    let replay = public_bookings::book(&f.runtime, slug, &[2; 32], input.clone())
        .await
        .unwrap();
    assert_eq!(replay.reference, receipt.reference);
    let mut changed = input;
    changed
        .answers
        .insert("client_name".into(), "Different".into());
    assert!(matches!(
        public_bookings::book(&f.runtime, slug, &[2; 32], changed).await,
        Err(PubError::IdempotencyConflict)
    ));
    appointment_execute(
        &f,
        f.owner,
        Uuid::new_v4(),
        AppointmentCmd::UpdateClient {
            id: a.id,
            specialist_id: spec.id,
            expected_version: 1,
            client: client("Updated by staff"),
        },
    )
    .await
    .unwrap();
    let updated = appointments::get(&f.runtime, f.owner, f.tenant, f.point, a.id)
        .await
        .unwrap();
    assert_eq!(updated.source, "public");
    assert_eq!(updated.created_by, None);
    assert_eq!(updated.updated_by, Some(f.owner));
    assert_eq!(updated.answers.0["client_name"], "Private Customer");
    appointment_execute(
        &f,
        f.owner,
        Uuid::new_v4(),
        AppointmentCmd::CancelAppointment {
            id: a.id,
            specialist_id: spec.id,
            expected_version: 2,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        public_bookings::receipt(&f.runtime, slug, &[2; 32])
            .await
            .unwrap()
            .status,
        "cancelled"
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn public_bookings_staff_race_and_alternative_share_occupancy() {
    let f = Fixture::new("Europe/Moscow").await;
    let spec = f.specialist().await;
    let day = future_day("Europe/Moscow", 4);
    let x = saved_offer(
        &f,
        spec.id,
        interval(&format!("{day}T10:00"), &format!("{day}T11:00")),
    )
    .await;
    let y = saved_offer(
        &f,
        spec.id,
        Timing::StartOnly {
            starts_local: format!("{day}T10:30"),
        },
    )
    .await;
    let slug = publish(&f, BookingForm::default()).await;
    let (public, staff) = tokio::join!(
        public_bookings::book(&f.runtime, slug, &[3; 32], public_input(&x)),
        appointment_execute(
            &f,
            f.owner,
            Uuid::new_v4(),
            record(spec.id, y.id, 1, "Staff")
        )
    );
    assert_eq!(usize::from(public.is_ok()) + usize::from(staff.is_ok()), 1);
    assert!(
        matches!(public, Err(PubError::Busy)) || matches!(staff, Err(BookingError::SpecialistBusy))
    );
    let available = public_day(&f, slug, &day, None, None, None, 100)
        .await
        .unwrap();
    assert!(available.items.is_empty());
    let agreement = saved_offer(
        &f,
        spec.id,
        Timing::ByAgreement {
            date: None,
            places: 1,
        },
    )
    .await;
    let (a, b) = tokio::join!(
        public_bookings::book(&f.runtime, slug, &[4; 32], public_input(&agreement)),
        public_bookings::book(&f.runtime, slug, &[5; 32], public_input(&agreement))
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(a, Err(PubError::Full)) || matches!(b, Err(PubError::Full)));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn public_bookings_reject_stale_versions_unknown_answers_and_past_offers() {
    let f = Fixture::new("Europe/Moscow").await;
    let spec = f.specialist().await;
    let o = saved_offer(
        &f,
        spec.id,
        Timing::ByAgreement {
            date: None,
            places: 10,
        },
    )
    .await;
    let slug = publish(&f, BookingForm::default()).await;
    let mut stale = public_input(&o);
    stale.form_version = 2;
    assert!(matches!(
        public_bookings::book(&f.runtime, slug, &[6; 32], stale).await,
        Err(PubError::Changed)
    ));
    let mut stale = public_input(&o);
    stale.specialist_version = 2;
    assert!(matches!(
        public_bookings::book(&f.runtime, slug, &[6; 32], stale).await,
        Err(PubError::Changed)
    ));
    for (key, value) in [
        ("client_name", ""),
        ("phone", "letters"),
        ("email", "invalid"),
        ("field_unknown", "unexpected"),
    ] {
        let mut invalid = public_input(&o);
        invalid.answers.insert(key.into(), value.into());
        assert!(matches!(
            public_bookings::book(&f.runtime, slug, &[6; 32], invalid).await,
            Err(PubError::Invalid)
        ));
    }
    let old = future_day("Europe/Moscow", -3);
    let past = saved_offer(
        &f,
        spec.id,
        Timing::StartOnly {
            starts_local: format!("{old}T10:00"),
        },
    )
    .await;
    assert!(matches!(
        public_bookings::book(&f.runtime, slug, &[6; 32], public_input(&past)).await,
        Err(PubError::Unavailable)
    ));
    let foreign = Fixture::new("Europe/Moscow").await;
    let p = foreign.specialist().await;
    let other = saved_offer(
        &foreign,
        p.id,
        Timing::ByAgreement {
            date: None,
            places: 1,
        },
    )
    .await;
    assert!(matches!(
        public_bookings::book(&f.runtime, slug, &[6; 32], public_input(&other)).await,
        Err(PubError::Unavailable)
    ));
    foreign.close().await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn public_bookings_quota_does_not_block_replay_or_staff() {
    let f = Fixture::new("Europe/Moscow").await;
    let spec = f.specialist().await;
    let o = saved_offer(
        &f,
        spec.id,
        Timing::ByAgreement {
            date: None,
            places: 50,
        },
    )
    .await;
    let slug = publish(&f, BookingForm::default()).await;
    for n in 1..=30 {
        public_bookings::book(&f.runtime, slug, &[n; 32], public_input(&o))
            .await
            .unwrap();
    }
    assert!(matches!(
        public_bookings::book(&f.runtime, slug, &[31; 32], public_input(&o)).await,
        Err(PubError::RateLimit)
    ));
    public_bookings::book(&f.runtime, slug, &[1; 32], public_input(&o))
        .await
        .unwrap();
    appointment_execute(
        &f,
        f.owner,
        Uuid::new_v4(),
        record(spec.id, o.id, 1, "Staff unaffected"),
    )
    .await
    .unwrap();
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn public_bookings_owner_capability_boundary_cannot_be_spoofed_by_runtime() {
    let f = Fixture::new("Europe/Moscow").await;
    let spec = f.specialist().await;
    let o = saved_offer(
        &f,
        spec.id,
        Timing::ByAgreement {
            date: None,
            places: 2,
        },
    )
    .await;
    let mut s = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.public_booking','true',true)")
        .execute(s.connection())
        .await
        .unwrap();
    let error=sqlx::query("INSERT INTO bookings.appointments(tenant_id,location_id,specialist_id,offer_id,id,client_name,phone,email,note,status,source,form_snapshot) VALUES($1,$2,$3,$4,$5,'Spoof','','','','confirmed','public',$6)").bind(f.tenant).bind(f.point).bind(spec.id).bind(o.id).bind(Uuid::new_v4()).bind(sqlx::types::Json(BookingForm::default())).execute(s.connection()).await.unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    drop(s);
    let permissions:(bool,bool,bool)=sqlx::query_as("SELECT has_table_privilege('turnesta_api','bookings.public_claims','SELECT'),has_function_privilege('turnesta_admin','bookings.public_book(uuid,bytea,uuid,integer,integer,integer,text,jsonb)','EXECUTE'),has_table_privilege('turnesta_worker','bookings.appointments','SELECT')").fetch_one(&f.migrator).await.unwrap();
    assert_eq!(permissions, (false, false, false));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn public_bookings_http_capability_origin_body_and_private_projection() {
    let f = Fixture::new("Europe/Moscow").await;
    let spec = f.specialist().await;
    let o = saved_offer(
        &f,
        spec.id,
        Timing::ByAgreement {
            date: None,
            places: 4,
        },
    )
    .await;
    let slug = publish(&f, BookingForm::default()).await;
    let token = hex::encode([7; 32]);
    let body = serde_json::to_value(public_input(&o)).unwrap();
    let mut request = public_post(slug, &token, body.clone());
    request.headers_mut().remove("x-booking-access");
    assert_eq!(f.json(request).await.0, StatusCode::UNAUTHORIZED);
    let mut request = public_post(slug, &token, body.clone());
    request
        .headers_mut()
        .insert("origin", "https://foreign.example".parse().unwrap());
    assert_eq!(f.json(request).await.0, StatusCode::FORBIDDEN);
    let mut bad = body.clone();
    bad["created_by"] = json!(f.owner);
    assert_eq!(
        f.json(public_post(slug, &token, bad)).await.0,
        StatusCode::BAD_REQUEST
    );
    let (status, receipt) = f.json(public_post(slug, &token, body)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(receipt["reference"].is_string());
    assert!(receipt.get("client_name").is_none() && receipt.get("answers").is_none());
    let path = format!("/api/public/bookings/{slug}/receipt");
    assert_eq!(
        f.json(Request::get(&path).body(Body::empty()).unwrap())
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let response = f
        .app
        .clone()
        .oneshot(
            Request::get(&path)
                .header("x-booking-access", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(
        response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'")
    );
    let point = future_day("Europe/Moscow", 1);
    let path = format!(
        "/api/public/bookings/{slug}/availability?month={}&date={point}&private=true",
        &point[..7]
    );
    assert_eq!(
        f.json(Request::get(path).body(Body::empty()).unwrap())
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn public_bookings_calendar_uses_point_timezone_and_receipt_survives_staff_move() {
    let f = Fixture::new("Europe/Moscow").await;
    let spec = f.specialist().await;
    let day = future_day("Europe/Moscow", 4);
    let o = saved_offer(
        &f,
        spec.id,
        interval(&format!("{day}T10:00"), &format!("{day}T11:00")),
    )
    .await;
    let mut s = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    s.update_location(
        f.point,
        &Name::parse("Clinic").unwrap(),
        "America/New_York",
        1,
    )
    .await
    .unwrap();
    s.commit().await.unwrap();
    let slug = publish(&f, BookingForm::default()).await;
    let page = public_day(&f, slug, &day, Some(spec.id), None, None, 50)
        .await
        .unwrap();
    assert_eq!(page.items[0].time_zone, "America/New_York");
    assert_eq!(page.items[0].starts_at, o.starts_at);
    assert_ne!(page.items[0].starts_local, o.starts_local);
    let mut input = public_input(&o);
    input.time_zone = "America/New_York".into();
    let r = public_bookings::book(&f.runtime, slug, &[8; 32], input)
        .await
        .unwrap();
    assert_eq!(r.starts_local, page.items[0].starts_local);
    let appointment = appointments::list(
        &f.runtime,
        f.owner,
        f.tenant,
        f.point,
        appointments::AppointmentPage {
            specialist: spec.id,
            date: &day,
            after: None,
            limit: 10,
            include_cancelled: false,
        },
    )
    .await
    .unwrap()
    .remove(0);
    let BookingResource::Offer(next) = f
        .execute(Command::SaveOffer {
            id: Uuid::new_v4(),
            specialist_id: spec.id,
            time_zone: "America/New_York".into(),
            title: "Next".into(),
            timing: Timing::StartOnly {
                starts_local: format!("{day}T13:00"),
            },
            state: OfferState::Ready,
            expected_version: 0,
            conflict_token: None,
        })
        .await
        .resource
    else {
        panic!("offer expected")
    };
    appointment_execute(
        &f,
        f.owner,
        Uuid::new_v4(),
        AppointmentCmd::MoveAppointment {
            id: appointment.id,
            specialist_id: spec.id,
            offer_id: next.id,
            expected_offer_version: 1,
            expected_version: 1,
        },
    )
    .await
    .unwrap();
    let changed = public_bookings::receipt(&f.runtime, slug, &[8; 32])
        .await
        .unwrap();
    assert_eq!(changed.reference, r.reference);
    assert_eq!(changed.version, 2);
    assert_eq!(changed.time_zone, "America/New_York");
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn booking_widget_settings_are_scoped_versioned_and_manager_only() {
    let f = Fixture::new("Europe/Moscow").await;
    let initial = widget::settings(&f.runtime, f.owner, f.tenant, f.point)
        .await
        .unwrap();
    assert_eq!(initial.version, 0);
    assert!(initial.allowed_origins.is_empty());
    let slug = publish(&f, BookingForm::default()).await;
    let input = || WidgetConfigure {
        allowed_origins: vec![
            "HTTPS://Clinic.Example:443/".into(),
            "https://clinic.example".into(),
            "http://[::1]:8000/".into(),
        ],
        expected_version: 0,
    };
    let operator = f.member(Role::Operator).await;
    assert!(matches!(
        widget::configure(
            &f.runtime,
            operator,
            f.tenant,
            f.point,
            "http://127.0.0.1:8080",
            input()
        )
        .await,
        Err(WidgetError::Core(CoreError::Forbidden))
    ));
    let saved = widget::configure(
        &f.runtime,
        f.owner,
        f.tenant,
        f.point,
        "http://127.0.0.1:8080",
        input(),
    )
    .await
    .unwrap();
    assert_eq!(saved.version, 1);
    assert_eq!(
        saved.allowed_origins,
        vec!["http://[::1]:8000", "https://clinic.example"]
    );
    assert!(matches!(
        widget::configure(
            &f.runtime,
            f.owner,
            f.tenant,
            f.point,
            "http://127.0.0.1:8080",
            input()
        )
        .await,
        Err(WidgetError::Core(CoreError::VersionConflict))
    ));
    assert_eq!(
        public_bookings::profile(&f.runtime, slug)
            .await
            .unwrap()
            .version,
        1
    );
    assert!(
        widget::permits(
            &f.runtime,
            slug,
            "https://clinic.example",
            "http://127.0.0.1:8080"
        )
        .await
        .unwrap()
    );
    assert!(
        !widget::permits(
            &f.runtime,
            slug,
            "https://www.clinic.example",
            "http://127.0.0.1:8080"
        )
        .await
        .unwrap()
    );
    let foreign = Fixture::new("Europe/Moscow").await;
    assert!(
        widget::settings(&f.runtime, f.owner, foreign.tenant, foreign.point)
            .await
            .is_err()
    );
    let mut session = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM bookings.widget_settings WHERE location_id=$1"
        )
        .bind(foreign.point)
        .fetch_one(session.connection())
        .await
        .unwrap(),
        0
    );
    drop(session);
    for origin in [
        "https://clinic.example/path",
        "http://clinic.example",
        "https://*.example",
        "http://127.0.0.1:8080",
        "https://clinic.example; script-src *",
    ] {
        assert!(
            widget::configure(
                &f.runtime,
                f.owner,
                f.tenant,
                f.point,
                "http://127.0.0.1:8080",
                WidgetConfigure {
                    allowed_origins: vec![origin.into()],
                    expected_version: 1
                }
            )
            .await
            .is_err()
        );
    }
    let permissions:(bool,bool,bool)=sqlx::query_as("SELECT has_table_privilege('turnesta_worker','bookings.widget_settings','SELECT'),has_function_privilege('turnesta_admin','bookings.widget_origins(uuid)','EXECUTE'),has_table_privilege('turnesta_runtime','bookings.widget_settings','DELETE')").fetch_one(&f.migrator).await.unwrap();
    assert_eq!(permissions, (false, false, false));
    let mut session = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    let error=sqlx::query("UPDATE bookings.widget_settings SET allowed_origins=ARRAY['https://x; frame-ancestors *'] WHERE tenant_id=$1 AND location_id=$2").bind(f.tenant).bind(f.point).execute(session.connection()).await.unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    drop(session);
    foreign.close().await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn booking_widget_csp_is_route_scoped_and_revocation_denies_new_loads() {
    let f = Fixture::new("Europe/Moscow").await;
    let slug = publish(&f, BookingForm::default()).await;
    let channel = Uuid::new_v4();
    let embed =
        format!("/embed/book/{slug}?parent_origin=https%3A%2F%2Fclinic.example&channel={channel}");
    let response = f
        .app
        .clone()
        .oneshot(Request::get(&embed).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(
        response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'")
    );
    widget::configure(
        &f.runtime,
        f.owner,
        f.tenant,
        f.point,
        "http://127.0.0.1:8080",
        WidgetConfigure {
            allowed_origins: vec!["https://clinic.example".into()],
            expected_version: 0,
        },
    )
    .await
    .unwrap();
    for path in [
        embed.clone(),
        format!("/embed/book/{slug}?channel={channel}&parent_origin=https%3A%2F%2Fclinic.example"),
    ] {
        let response = f
            .app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get_all("content-security-policy")
                .iter()
                .count(),
            1
        );
        assert!(
            response.headers()["content-security-policy"]
                .to_str()
                .unwrap()
                .contains("frame-ancestors https://clinic.example;")
        );
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["referrer-policy"], "no-referrer");
        let body = String::from_utf8(
            to_bytes(response.into_body(), 128 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(body.contains("data-parent-origin=\"https://clinic.example\""));
        assert!(!body.contains("answers") && !body.contains("client_name"));
    }
    for path in [
        format!("/book/{slug}"),
        format!("/api/public/bookings/{slug}"),
        "/login".into(),
        "/health".into(),
        f.url("widget"),
    ] {
        let response = f
            .app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(
            response.headers()["content-security-policy"]
                .to_str()
                .unwrap()
                .contains("frame-ancestors 'none'")
        );
    }
    for suffix in [
        format!("parent_origin=https://www.clinic.example&channel={channel}"),
        format!("parent_origin=https://clinic.example:444&channel={channel}"),
        "parent_origin=https://clinic.example&channel=bad".to_owned(),
        format!("parent_origin=https://clinic.example&channel={channel}&x=1"),
    ] {
        let response = f
            .app
            .clone()
            .oneshot(
                Request::get(format!("/embed/book/{slug}?{suffix}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_client_error());
        assert!(
            response.headers()["content-security-policy"]
                .to_str()
                .unwrap()
                .contains("frame-ancestors 'none'")
        );
    }
    public_bookings::configure(
        &f.runtime,
        f.owner,
        f.tenant,
        f.point,
        false,
        BookingForm::default(),
        1,
    )
    .await
    .unwrap();
    assert_eq!(
        f.app
            .clone()
            .oneshot(Request::get(&embed).body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    widget::configure(
        &f.runtime,
        f.owner,
        f.tenant,
        f.point,
        "http://127.0.0.1:8080",
        WidgetConfigure {
            allowed_origins: vec![],
            expected_version: 1,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        f.app
            .clone()
            .oneshot(Request::get(&embed).body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn booking_widget_http_requires_session_csrf_and_rechecks_membership() {
    let f = Fixture::new("Europe/Moscow").await;
    assert_eq!(
        f.json(Request::get(f.url("widget")).body(Body::empty()).unwrap())
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let login = f.login(f.owner).await;
    let make = || {
        Request::put(f.url("widget"))
            .header("origin", "http://127.0.0.1:8080")
            .header("cookie", &login.0)
            .header("x-csrf-token", &login.1)
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"allowed_origins":["https://clinic.example"],"expected_version":0})
                    .to_string(),
            ))
            .unwrap()
    };
    let mut request = make();
    request.headers_mut().remove("x-csrf-token");
    assert_eq!(f.json(request).await.0, StatusCode::FORBIDDEN);
    let (status, value) = f.json(make()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["version"], 1);
    assert!(value["public_url"].is_null());
    let operator = f.member(Role::Operator).await;
    let op_login = f.login(operator).await;
    let (status, value) = f
        .json(
            Request::get(f.url("widget"))
                .header("cookie", &op_login.0)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["allowed_origins"], json!(["https://clinic.example"]));
    let mut request = make();
    request
        .headers_mut()
        .insert("cookie", op_login.0.parse().unwrap());
    request
        .headers_mut()
        .insert("x-csrf-token", op_login.1.parse().unwrap());
    assert_eq!(f.json(request).await.0, StatusCode::FORBIDDEN);
    sqlx::query("DELETE FROM core.memberships WHERE tenant_id=$1 AND user_id=$2")
        .bind(f.tenant)
        .bind(operator)
        .execute(&f.migrator)
        .await
        .unwrap();
    assert_eq!(
        f.json(
            Request::get(f.url("widget"))
                .header("cookie", &op_login.0)
                .body(Body::empty())
                .unwrap()
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn pilot_owner_operator_visitor_booking_move_cancel_and_access_revoke() {
    let f = Fixture::new("Europe/Moscow").await;
    let owner = f.login(f.owner).await;
    let operator = f.member(Role::Operator).await;
    let staff = f.login(operator).await;
    let doctor = Uuid::new_v4();
    let preset = Uuid::new_v4();
    let day = future_day("Europe/Moscow", 4);
    for command in [
        json!({"type":"save_specialist","id":doctor,"name":"Pilot doctor","description":"Synthetic rehearsal","enabled":true,"expected_version":0}),
        json!({"type":"save_preset","id":preset,"name":"Pilot consultation","duration_minutes":30,"enabled":true,"expected_version":0}),
    ] {
        assert_eq!(
            f.json(post(&f, &owner, Uuid::new_v4(), command)).await.0,
            StatusCode::OK
        );
    }
    let plan = BatchRequest {
        id: Uuid::new_v4(),
        specialist_id: doctor,
        time_zone: "Europe/Moscow".into(),
        state: OfferState::Ready,
        source: BatchSource::PresetSeries {
            preset_id: preset,
            dates: vec![day.clone()],
            start_time: "09:00".into(),
            end_time: "10:00".into(),
            gap_minutes: 0,
        },
    };
    let preview_request = Request::post(f.url("batch-preview"))
        .header("cookie", &owner.0)
        .header("origin", "http://127.0.0.1:8080")
        .header("x-csrf-token", &owner.1)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&plan).unwrap()))
        .unwrap();
    let (status, preview) = f.json(preview_request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(preview["rows"].as_array().unwrap().len(), 2);
    let command = json!({"type":"save_batch","request":plan,"preview_token":preview["token"],"allow_alternatives":false});
    let batch_key = Uuid::new_v4();
    let (_, first) = f.json(post(&f, &owner, batch_key, command.clone())).await;
    let (_, replay) = f.json(post(&f, &owner, batch_key, command)).await;
    assert_eq!(first, replay);
    let mut form = BookingForm::default();
    form.fields.push(BookingFormField {
        key: "field_visit".into(),
        label: "Visit type".into(),
        kind: BookingFieldKind::Select,
        required: true,
        options: vec!["First".into(), "Repeat".into()],
    });
    let configure = |path: &str, body: Value| {
        Request::put(f.url(path))
            .header("cookie", &owner.0)
            .header("origin", "http://127.0.0.1:8080")
            .header("x-csrf-token", &owner.1)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let (status, access) = f
        .json(configure(
            "public-access",
            json!({"enabled":true,"form":form,"expected_version":0}),
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    let slug: Uuid = access["public_slug"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        f.json(configure(
            "widget",
            json!({"allowed_origins":["http://localhost:8094"],"expected_version":0})
        ))
        .await
        .0,
        StatusCode::OK
    );
    let embed = f
        .app
        .clone()
        .oneshot(
            Request::get(format!(
                "/embed/book/{slug}?parent_origin=http%3A%2F%2Flocalhost%3A8094&channel={}",
                Uuid::new_v4()
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(embed.status(), StatusCode::OK);
    assert!(
        embed.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-ancestors http://localhost:8094")
    );
    let availability = format!(
        "/api/public/bookings/{slug}/availability?month={}&date={day}&limit=50",
        &day[..7]
    );
    let (status, public) = f
        .json(Request::get(&availability).body(Body::empty()).unwrap())
        .await;
    assert_eq!(status, StatusCode::OK);
    let offers = public["items"].as_array().unwrap();
    assert_eq!(offers.len(), 2);
    let body = json!({"offer_id":offers[0]["id"],"offer_version":offers[0]["version"],"specialist_version":offers[0]["specialist_version"],"form_version":1,"time_zone":"Europe/Moscow",
        "answers":{"client_name":"Pilot visitor","phone":"+7 900 000 00 00","email":"pilot-visitor@example.test","note":"Synthetic only","field_visit":"First"}});
    let mut secret = [0u8; 32];
    getrandom::fill(&mut secret).unwrap();
    let token = hex::encode(secret);
    let (status, receipt) = f.json(public_post(slug, &token, body.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(receipt["status"], "confirmed");
    let (status, replay) = f.json(public_post(slug, &token, body.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(receipt["reference"], replay["reference"]);
    for private in [
        "Pilot visitor",
        "pilot-visitor@example.test",
        "Synthetic only",
        "answers",
        "field_visit",
    ] {
        assert!(!receipt.to_string().contains(private));
    }
    let duplicate = record(
        doctor,
        offers[0]["id"].as_str().unwrap().parse().unwrap(),
        1,
        "Duplicate",
    );
    assert_eq!(
        f.json(appointment_post(
            &f,
            &staff,
            Uuid::new_v4(),
            serde_json::to_value(duplicate).unwrap()
        ))
        .await
        .0,
        StatusCode::CONFLICT
    );
    let staff_record = record(
        doctor,
        offers[1]["id"].as_str().unwrap().parse().unwrap(),
        1,
        "Pilot phone visitor",
    );
    let (status, recorded) = f
        .json(appointment_post(
            &f,
            &staff,
            Uuid::new_v4(),
            serde_json::to_value(staff_record).unwrap(),
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    let staff_id: Uuid = recorded["resource"]["item"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let clients_url = f.url(&format!(
        "appointments?specialist_id={doctor}&date={day}&limit=50"
    ));
    let (status, clients) = f
        .json(
            Request::get(&clients_url)
                .header("cookie", &staff.0)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(clients["items"].as_array().unwrap().len(), 2);
    let guest = clients["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["source"] == "public")
        .unwrap();
    let guest_id: Uuid = guest["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(guest["answers"]["field_visit"], "First");
    let employee = f.member(Role::Employee).await;
    let employee_login = f.login(employee).await;
    assert_eq!(
        f.json(
            Request::get(&clients_url)
                .header("cookie", &employee_login.0)
                .body(Body::empty())
                .unwrap()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let move_guest = AppointmentCmd::MoveAppointment {
        id: guest_id,
        specialist_id: doctor,
        offer_id: offers[1]["id"].as_str().unwrap().parse().unwrap(),
        expected_offer_version: 1,
        expected_version: 1,
    };
    assert_eq!(
        f.json(appointment_post(
            &f,
            &staff,
            Uuid::new_v4(),
            serde_json::to_value(&move_guest).unwrap()
        ))
        .await
        .0,
        StatusCode::CONFLICT
    );
    let cancel = AppointmentCmd::CancelAppointment {
        id: staff_id,
        specialist_id: doctor,
        expected_version: 1,
    };
    assert_eq!(
        f.json(appointment_post(
            &f,
            &staff,
            Uuid::new_v4(),
            serde_json::to_value(cancel).unwrap()
        ))
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.json(appointment_post(
            &f,
            &staff,
            Uuid::new_v4(),
            serde_json::to_value(move_guest).unwrap()
        ))
        .await
        .0,
        StatusCode::OK
    );
    let (_, moved) = f
        .json(
            Request::get(format!("/api/public/bookings/{slug}/receipt"))
                .header("x-booking-access", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(moved["starts_local"], offers[1]["starts_local"]);
    let cancel = AppointmentCmd::CancelAppointment {
        id: guest_id,
        specialist_id: doctor,
        expected_version: 2,
    };
    assert_eq!(
        f.json(appointment_post(
            &f,
            &staff,
            Uuid::new_v4(),
            serde_json::to_value(cancel).unwrap()
        ))
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.json(configure(
            "public-access",
            json!({"enabled":false,"form":form,"expected_version":1})
        ))
        .await
        .0,
        StatusCode::OK
    );
    let (_, cancelled) = f.json(public_post(slug, &token, body)).await;
    assert_eq!(cancelled["status"], "cancelled");
    assert_eq!(cancelled["reference"], receipt["reference"]);
    let revoke=Request::post(format!("/api/organizations/{}/team/commands",f.tenant)).header("cookie",&owner.0).header("origin","http://127.0.0.1:8080").header("x-csrf-token",&owner.1).header("idempotency-key",Uuid::new_v4().to_string()).header("content-type","application/json")
        .body(Body::from(json!({"type":"set_member","user_id":operator,"role":"operator","active":false,"expected_version":1}).to_string())).unwrap();
    assert_eq!(f.json(revoke).await.0, StatusCode::OK);
    assert_eq!(
        f.json(
            Request::get(&clients_url)
                .header("cookie", &staff.0)
                .body(Body::empty())
                .unwrap()
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM bookings.appointments WHERE tenant_id=$1")
            .bind(f.tenant)
            .fetch_one(&f.migrator)
            .await
            .unwrap();
    assert_eq!(count, 2);
    f.close().await;
}

fn batch_copy(spec: Uuid, source: &str, targets: &[&str], zone: &str) -> BatchRequest {
    BatchRequest {
        id: Uuid::new_v4(),
        specialist_id: spec,
        time_zone: zone.into(),
        state: OfferState::Ready,
        source: BatchSource::CopyDay {
            source_date: source.into(),
            target_dates: targets.iter().map(|s| (*s).into()).collect(),
        },
    }
}
fn batch_series(spec: Uuid, preset: Uuid, dates: &[&str], zone: &str) -> BatchRequest {
    BatchRequest {
        id: Uuid::new_v4(),
        specialist_id: spec,
        time_zone: zone.into(),
        state: OfferState::Ready,
        source: BatchSource::PresetSeries {
            preset_id: preset,
            dates: dates.iter().map(|s| (*s).into()).collect(),
            start_time: "09:00".into(),
            end_time: "12:00".into(),
            gap_minutes: 15,
        },
    }
}
async fn save_batch(
    f: &Fixture,
    key: Uuid,
    request: BatchRequest,
    token: String,
    allow: bool,
) -> Result<BatchReceipt, BookingError> {
    let result = bookings::execute(
        &f.runtime,
        f.owner,
        f.tenant,
        f.point,
        key,
        Command::SaveBatch {
            request,
            preview_token: token,
            allow_alternatives: allow,
        },
    )
    .await?;
    let BookingResource::Batch(receipt) = result.resource else {
        panic!("batch receipt expected")
    };
    Ok(receipt)
}
async fn batch_preset(f: &Fixture, minutes: i32) -> Uuid {
    let id = Uuid::new_v4();
    f.execute(Command::SavePreset {
        id,
        name: "Procedure".into(),
        duration_minutes: Some(minutes),
        enabled: true,
        expected_version: 0,
    })
    .await;
    id
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn batch_copy_is_atomic_keeps_all_timing_modes_and_never_copies_clients() {
    let f = Fixture::new("Europe/Moscow").await;
    let spec = f.specialist().await;
    let first = saved_offer(
        &f,
        spec.id,
        interval("2030-05-01T23:30", "2030-05-02T01:00"),
    )
    .await;
    saved_offer(
        &f,
        spec.id,
        Timing::StartOnly {
            starts_local: "2030-05-01T10:00".into(),
        },
    )
    .await;
    saved_offer(
        &f,
        spec.id,
        Timing::ByAgreement {
            date: Some("2030-05-01".into()),
            places: 4,
        },
    )
    .await;
    saved_offer(
        &f,
        spec.id,
        Timing::ByAgreement {
            date: None,
            places: 8,
        },
    )
    .await;
    let mut withdrawn = offer(
        spec.id,
        Timing::ByAgreement {
            date: Some("2030-05-01".into()),
            places: 2,
        },
    );
    if let Command::SaveOffer { state, .. } = &mut withdrawn {
        *state = OfferState::Withdrawn;
    }
    f.execute(withdrawn).await;
    appointment_execute(
        &f,
        f.owner,
        Uuid::new_v4(),
        record(spec.id, first.id, 1, "PRIVATE-SOURCE-CLIENT"),
    )
    .await
    .unwrap();
    let request = batch_copy(
        spec.id,
        "2030-05-01",
        &["2030-05-20", "2030-05-21"],
        "Europe/Moscow",
    );
    let preview = batch::preview(&f.runtime, f.owner, f.tenant, f.point, request.clone())
        .await
        .unwrap();
    assert_eq!(preview.rows.len(), 6);
    assert!(preview.conflicts.is_empty());
    let json = serde_json::to_string(&preview).unwrap();
    assert!(
        !json.contains("PRIVATE-SOURCE-CLIENT")
            && !json.contains("phone")
            && !json.contains("client_name")
    );
    assert!(
        preview
            .rows
            .iter()
            .any(|r| r.ends_local.as_deref() == Some("2030-05-21T01:00"))
    );
    let key = Uuid::new_v4();
    let receipt = save_batch(&f, key, request.clone(), preview.token.clone(), false)
        .await
        .unwrap();
    assert_eq!(receipt.offer_ids.len(), 6);
    let clients: i64 =
        sqlx::query_scalar("SELECT count(*) FROM bookings.appointments WHERE tenant_id=$1")
            .bind(f.tenant)
            .fetch_one(&f.migrator)
            .await
            .unwrap();
    assert_eq!(clients, 1);
    let copied:Vec<bookings::Offer>=sqlx::query_as("SELECT *,NULL::bigint AS reserved_places,NULL::boolean AS blocked_by_booking FROM bookings.offer_details WHERE tenant_id=$1 AND id=ANY($2)").bind(f.tenant).bind(&receipt.offer_ids).fetch_all(&f.migrator).await.unwrap();
    assert_eq!(copied.len(), 6);
    assert_eq!(
        copied
            .iter()
            .filter(|o| o.mode == "by_agreement" && o.places == 4)
            .count(),
        2
    );
    assert!(copied.iter().all(|o| o.state == OfferState::Ready));
    f.execute(Command::SaveSpecialist {
        id: spec.id,
        name: "Renamed".into(),
        description: String::new(),
        enabled: false,
        expected_version: 1,
    })
    .await;
    let replay = save_batch(&f, key, request.clone(), preview.token.clone(), false)
        .await
        .unwrap();
    assert_eq!(replay.offer_ids, receipt.offer_ids);
    assert!(matches!(
        save_batch(&f, key, request, preview.token, true).await,
        Err(BookingError::IdempotencyConflict)
    ));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn batch_series_uses_preset_window_gap_and_rechecks_source_version() {
    let f = Fixture::new("Europe/Moscow").await;
    let spec = f.specialist().await;
    let preset = batch_preset(&f, 30).await;
    let request = batch_series(
        spec.id,
        preset,
        &["2030-05-20", "2030-05-21"],
        "Europe/Moscow",
    );
    let preview = batch::preview(&f.runtime, f.owner, f.tenant, f.point, request.clone())
        .await
        .unwrap();
    assert_eq!(preview.rows.len(), 8);
    assert_eq!(
        preview.rows[1].starts_local.as_deref(),
        Some("2030-05-20T09:45")
    );
    assert_eq!(
        preview.rows[3].ends_local.as_deref(),
        Some("2030-05-20T11:45")
    );
    f.execute(Command::SavePreset {
        id: preset,
        name: "Changed".into(),
        duration_minutes: Some(45),
        enabled: true,
        expected_version: 1,
    })
    .await;
    assert!(matches!(
        save_batch(&f, Uuid::new_v4(), request.clone(), preview.token, false).await,
        Err(BookingError::BatchChanged)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM bookings.offers WHERE tenant_id=$1")
            .bind(f.tenant)
            .fetch_one(&f.migrator)
            .await
            .unwrap(),
        0
    );
    let preview = batch::preview(&f.runtime, f.owner, f.tenant, f.point, request.clone())
        .await
        .unwrap();
    assert_eq!(preview.rows.len(), 6);
    let receipt = save_batch(&f, Uuid::new_v4(), request, preview.token, false)
        .await
        .unwrap();
    assert_eq!(receipt.offer_ids.len(), 6);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn batch_duplicate_policy_is_distinct_from_alternatives_and_known_booked_time() {
    let f = Fixture::new("Europe/Moscow").await;
    let spec = f.specialist().await;
    saved_offer(
        &f,
        spec.id,
        interval("2030-05-01T09:30", "2030-05-01T10:30"),
    )
    .await;
    saved_offer(
        &f,
        spec.id,
        Timing::StartOnly {
            starts_local: "2030-05-01T10:00".into(),
        },
    )
    .await;
    let occupied = saved_offer(
        &f,
        spec.id,
        interval("2030-05-20T10:00", "2030-05-20T11:00"),
    )
    .await;
    appointment_execute(
        &f,
        f.owner,
        Uuid::new_v4(),
        record(spec.id, occupied.id, 1, "Existing client"),
    )
    .await
    .unwrap();
    let request = batch_copy(spec.id, "2030-05-01", &["2030-05-20"], "Europe/Moscow");
    let preview = batch::preview(&f.runtime, f.owner, f.tenant, f.point, request.clone())
        .await
        .unwrap();
    assert!(!preview.has_duplicates);
    for kind in ["internal", "booked", "overlap"] {
        assert!(preview.conflicts.iter().any(|c| c.kind == kind));
    }
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM bookings.offers WHERE tenant_id=$1")
        .bind(f.tenant)
        .fetch_one(&f.migrator)
        .await
        .unwrap();
    assert!(matches!(
        save_batch(
            &f,
            Uuid::new_v4(),
            request.clone(),
            preview.token.clone(),
            false
        )
        .await,
        Err(BookingError::BatchConsent)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM bookings.offers WHERE tenant_id=$1")
            .bind(f.tenant)
            .fetch_one(&f.migrator)
            .await
            .unwrap(),
        before
    );
    let saved = save_batch(&f, Uuid::new_v4(), request.clone(), preview.token, true)
        .await
        .unwrap();
    assert_eq!(saved.offer_ids.len(), 2);
    assert!(matches!(
        appointment_execute(
            &f,
            f.owner,
            Uuid::new_v4(),
            record(spec.id, saved.offer_ids[0], 1, "Second client")
        )
        .await,
        Err(BookingError::SpecialistBusy)
    ));
    let preview = batch::preview(&f.runtime, f.owner, f.tenant, f.point, request.clone())
        .await
        .unwrap();
    assert!(preview.has_duplicates);
    assert!(matches!(
        save_batch(&f, Uuid::new_v4(), request, preview.token, true).await,
        Err(BookingError::BatchDuplicate)
    ));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn batch_concurrent_saves_and_new_appointment_invalidate_old_preview() {
    let f = Fixture::new("Europe/Moscow").await;
    let spec = f.specialist().await;
    let preset = batch_preset(&f, 30).await;
    let anchor = saved_offer(
        &f,
        spec.id,
        interval("2030-06-01T17:00", "2030-06-01T17:30"),
    )
    .await;
    let request = batch_series(spec.id, preset, &["2030-05-20"], "Europe/Moscow");
    let preview = batch::preview(&f.runtime, f.owner, f.tenant, f.point, request.clone())
        .await
        .unwrap();
    appointment_execute(
        &f,
        f.owner,
        Uuid::new_v4(),
        record(spec.id, anchor.id, 1, "Changes calendar revision"),
    )
    .await
    .unwrap();
    assert!(matches!(
        save_batch(&f, Uuid::new_v4(), request.clone(), preview.token, false).await,
        Err(BookingError::BatchChanged)
    ));
    let preview = batch::preview(&f.runtime, f.owner, f.tenant, f.point, request.clone())
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        save_batch(
            &f,
            Uuid::new_v4(),
            request.clone(),
            preview.token.clone(),
            false
        ),
        save_batch(&f, Uuid::new_v4(), request, preview.token, false)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let error = if a.is_err() {
        a.unwrap_err()
    } else {
        b.unwrap_err()
    };
    assert!(matches!(error, BookingError::BatchChanged));
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM bookings.offers WHERE tenant_id=$1 AND local_date='2030-05-20'",
    )
    .bind(f.tenant)
    .fetch_one(&f.migrator)
    .await
    .unwrap();
    assert_eq!(count, 4);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn batch_dst_limits_timezone_roles_and_http_validation_fail_without_writes() {
    let f = Fixture::new("America/New_York").await;
    let spec = f.specialist().await;
    let mut source = offer(
        spec.id,
        Timing::StartOnly {
            starts_local: "2026-03-01T02:30".into(),
        },
    );
    if let Command::SaveOffer { time_zone, .. } = &mut source {
        *time_zone = "America/New_York".into();
    }
    f.execute(source).await;
    let gap_request = batch_copy(spec.id, "2026-03-01", &["2026-03-08"], "America/New_York");
    assert!(matches!(
        batch::preview(&f.runtime, f.owner, f.tenant, f.point, gap_request.clone()).await,
        Err(BookingError::BatchTime(_))
    ));
    let preset = batch_preset(&f, 60).await;
    let mut fold = batch_series(spec.id, preset, &["2026-11-01"], "America/New_York");
    if let BatchSource::PresetSeries {
        start_time,
        end_time,
        ..
    } = &mut fold.source
    {
        *start_time = "00:30".into();
        *end_time = "03:30".into();
    }
    assert!(matches!(
        batch::preview(&f.runtime, f.owner, f.tenant, f.point, fold).await,
        Err(BookingError::BatchTime(_))
    ));
    let tiny = batch_preset(&f, 1).await;
    let mut huge = batch_series(spec.id, tiny, &["2030-05-20"], "America/New_York");
    if let BatchSource::PresetSeries {
        start_time,
        end_time,
        gap_minutes,
        ..
    } = &mut huge.source
    {
        *start_time = "00:00".into();
        *end_time = "23:59".into();
        *gap_minutes = 0;
    }
    assert!(matches!(
        batch::preview(&f.runtime, f.owner, f.tenant, f.point, huge).await,
        Err(BookingError::InvalidBatch)
    ));
    let operator = f.member(Role::Operator).await;
    assert!(matches!(
        batch::preview(&f.runtime, operator, f.tenant, f.point, gap_request.clone()).await,
        Err(BookingError::Core(CoreError::Forbidden))
    ));
    let login = f.login(f.owner).await;
    let make = |body: Value| {
        Request::post(f.url("batch-preview"))
            .header("cookie", &login.0)
            .header("origin", "http://127.0.0.1:8080")
            .header("x-csrf-token", &login.1)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let body = serde_json::to_value(&gap_request).unwrap();
    let mut no_csrf = make(body.clone());
    no_csrf.headers_mut().remove("x-csrf-token");
    assert_eq!(f.json(no_csrf).await.0, StatusCode::FORBIDDEN);
    let (status, error) = f.json(make(body.clone())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "batch_time");
    assert_eq!(error["details"]["date"], "2026-03-08");
    let mut spoofed = body;
    spoofed["created_by"] = json!(f.owner);
    assert_eq!(f.json(make(spoofed)).await.0, StatusCode::BAD_REQUEST);
    let mut wrong_zone = batch_series(spec.id, preset, &["2030-05-20"], "UTC");
    assert!(matches!(
        batch::preview(&f.runtime, f.owner, f.tenant, f.point, wrong_zone.clone()).await,
        Err(BookingError::TimeZoneChanged)
    ));
    wrong_zone.time_zone = "America/New_York".into();
    let preview = batch::preview(&f.runtime, f.owner, f.tenant, f.point, wrong_zone.clone())
        .await
        .unwrap();
    let mut session = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    let point = session
        .locations()
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.id == f.point)
        .unwrap();
    session
        .update_location(
            f.point,
            &Name::parse(&point.name).unwrap(),
            "UTC",
            point.version,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert!(matches!(
        save_batch(&f, Uuid::new_v4(), wrong_zone, preview.token, false).await,
        Err(BookingError::TimeZoneChanged)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM bookings.offers WHERE tenant_id=$1")
            .bind(f.tenant)
            .fetch_one(&f.migrator)
            .await
            .unwrap(),
        1
    );
    f.close().await;
}
