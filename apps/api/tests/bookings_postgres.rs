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
    auth::login::{self, Email},
    bookings::{
        self, BookingCommand as Command, BookingError, BookingResource, OfferState, Timing,
    },
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
