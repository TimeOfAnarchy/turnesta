use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header::SET_COOKIE},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use turnesta_api::{
    auth::login::{self, Email},
    core::{CoreError, Name, Provisioner, Role, TenantId, TenantSession, UserId},
    http::{AppState, router},
    queue::{self, Command, TicketStatus},
    queue_live::LiveHub,
    queue_public::{self as public, PublicQueueError},
};
use uuid::Uuid;

const ORIGIN: &str = "http://127.0.0.1:8080";

async fn service(f: &Fixture, name: &str) -> turnesta_api::catalog::Service {
    turnesta_api::catalog::save(
        &f.runtime,
        f.owner,
        f.tenant,
        f.queue,
        None,
        turnesta_api::catalog::ServiceDraft {
            name: &Name::parse(name).unwrap(),
            description: "What to bring",
            enabled: true,
        },
    )
    .await
    .unwrap()
}
fn service_write(
    f: &Fixture,
    login: &(String, String),
    id: Option<Uuid>,
    body: Value,
) -> Request<Body> {
    let base = format!(
        "/api/organizations/{}/queues/{}/services",
        f.tenant, f.queue
    );
    Request::builder()
        .method(if id.is_some() { "PATCH" } else { "POST" })
        .uri(id.map(|id| format!("{base}/{id}")).unwrap_or(base))
        .header("cookie", &login.0)
        .header("origin", ORIGIN)
        .header("x-csrf-token", &login.1)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn catalog_roles_rls_versions_and_point_links_are_enforced() {
    use turnesta_api::catalog::{self, ServiceDraft};
    let f = Fixture::new().await;
    let other = Fixture::new().await;
    let s = service(&f, "Repair").await;
    let foreign = service(&other, "Foreign").await;
    for role in [Role::Operator, Role::Employee] {
        let user = f.member(role).await;
        assert_eq!(
            catalog::list(&f.runtime, user, f.tenant, f.queue, None, 50)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            catalog::save(
                &f.runtime,
                user,
                f.tenant,
                f.queue,
                Some((s.id, 1)),
                ServiceDraft {
                    name: &Name::parse("Changed").unwrap(),
                    description: "",
                    enabled: false
                }
            )
            .await
            .is_err()
        );
        let mut tx = TenantSession::begin(&f.runtime, user, f.tenant)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query("UPDATE catalog.services SET enabled=false")
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
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM catalog.services")
            .fetch_one(tx.connection())
            .await
            .unwrap(),
        0
    );
    tx.commit().await.unwrap();
    let error=sqlx::query("INSERT INTO queue.service_links (tenant_id,queue_id,location_id,service_id) SELECT tenant_id,id,location_id,$3 FROM queue.queues WHERE tenant_id=$1 AND id=$2")
        .bind(f.tenant).bind(f.queue).bind(foreign.id).execute(&f.migrator).await.unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23503")
    );
    // Keep an owned Name for the asynchronous draft borrow.
    let name = Name::parse("New name").unwrap();
    let manager = f.member(Role::Manager).await;
    let (a, b) = tokio::join!(
        catalog::save(
            &f.runtime,
            manager,
            f.tenant,
            f.queue,
            Some((s.id, 1)),
            ServiceDraft {
                name: &name,
                description: "",
                enabled: false
            }
        ),
        catalog::save(
            &f.runtime,
            f.owner,
            f.tenant,
            f.queue,
            Some((s.id, 1)),
            ServiceDraft {
                name: &name,
                description: "",
                enabled: false
            }
        )
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(
        catalog::list(&f.runtime, f.owner, f.tenant, f.queue, None, 50)
            .await
            .unwrap()[0]
            .version,
        2
    );
    other.close().await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn services_preserve_ticket_snapshots_staff_replay_and_legacy_json() {
    use turnesta_api::catalog::{self, ServiceDraft};
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let s = service(&f, "Original name").await;
    let key = Uuid::new_v4();
    let command = Command::Issue {
        service_id: Some(s.id),
    };
    let original = queue::execute(&f.runtime, f.owner, f.tenant, f.queue, key, command.clone())
        .await
        .unwrap();
    let (_, hash) = token();
    let guest = public::issue_for_service(&f.runtime, slug, &hash, Some(s.id))
        .await
        .unwrap();
    assert_eq!(guest.service_name.as_deref(), Some("Original name"));
    let mut legacy = serde_json::to_value(&original).unwrap();
    legacy["ticket"]
        .as_object_mut()
        .unwrap()
        .remove("service_id");
    legacy["ticket"]
        .as_object_mut()
        .unwrap()
        .remove("service_name");
    let restored: queue::CommandResult = serde_json::from_value(legacy).unwrap();
    assert!(restored.ticket.unwrap().service_name.is_none());
    let legacy_command: Command = serde_json::from_value(json!({"type":"issue"})).unwrap();
    assert_eq!(
        serde_json::to_value(&legacy_command).unwrap(),
        json!({"type":"issue"})
    );
    catalog::save(
        &f.runtime,
        f.owner,
        f.tenant,
        f.queue,
        Some((s.id, 1)),
        ServiceDraft {
            name: &Name::parse("Renamed").unwrap(),
            description: "",
            enabled: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        queue::execute(&f.runtime, f.owner, f.tenant, f.queue, key, command)
            .await
            .unwrap(),
        original
    );
    assert!(matches!(
        queue::execute(
            &f.runtime,
            f.owner,
            f.tenant,
            f.queue,
            key,
            Command::Issue { service_id: None }
        )
        .await,
        Err(queue::QueueError::IdempotencyConflict)
    ));
    assert!(matches!(
        queue::execute(
            &f.runtime,
            f.owner,
            f.tenant,
            f.queue,
            Uuid::new_v4(),
            Command::Issue {
                service_id: Some(s.id)
            }
        )
        .await,
        Err(queue::QueueError::ServiceUnavailable)
    ));
    public::configure(&f.runtime, f.owner, f.tenant, f.queue, false, 1)
        .await
        .unwrap();
    assert_eq!(
        public::issue_for_service(&f.runtime, slug, &hash, Some(Uuid::new_v4()))
            .await
            .unwrap(),
        guest
    );
    assert_eq!(
        public::ticket(&f.runtime, slug, &hash)
            .await
            .unwrap()
            .service_name
            .as_deref(),
        Some("Original name")
    );
    let list = queue::tickets(
        &f.runtime,
        f.owner,
        f.tenant,
        f.queue,
        queue::TicketPage {
            limit: 50,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        list.iter()
            .all(|t| t.service_name.as_deref() == Some("Original name"))
    );
    assert_eq!(
        f.staff(Command::Issue { service_id: None })
            .await
            .ticket
            .unwrap()
            .service_name,
        None
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn public_service_race_has_one_visit_and_different_queue_cannot_be_selected() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let a = service(&f, "A").await;
    let b = service(&f, "B").await;
    let (_, hash) = token();
    let (first, second) = tokio::join!(
        public::issue_for_service(&f.runtime, slug, &hash, Some(a.id)),
        public::issue_for_service(&f.runtime, slug, &hash, Some(b.id))
    );
    let first = first.unwrap();
    assert_eq!(first, second.unwrap());
    assert_eq!(first.number, 1);
    assert!([Some("A"), Some("B")].contains(&first.service_name.as_deref()));
    let point = queue::get(&f.runtime, f.owner, f.tenant, f.queue)
        .await
        .unwrap()
        .location_id;
    let other_queue = queue::create(
        &f.runtime,
        f.owner,
        f.tenant,
        point,
        &Name::parse("Other queue").unwrap(),
    )
    .await
    .unwrap();
    let other_slug = public::configure(&f.runtime, f.owner, f.tenant, other_queue.id, true, 0)
        .await
        .unwrap()
        .public_slug
        .unwrap();
    let (_, invalid) = token();
    assert!(matches!(
        public::issue_for_service(&f.runtime, other_slug, &invalid, Some(a.id)).await,
        Err(PublicQueueError::ServiceUnavailable)
    ));
    assert_eq!(
        public::issue(&f.runtime, other_slug, &invalid)
            .await
            .unwrap()
            .number,
        1,
        "failed service issuance consumed a number"
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM queue.guest_claims WHERE tenant_id=$1 AND queue_id=$2",
    )
    .bind(f.tenant)
    .bind(f.queue)
    .fetch_one(&f.migrator)
    .await
    .unwrap();
    assert_eq!(count, 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn service_archive_serializes_before_public_issue_without_leaving_a_claim() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let s = service(&f, "Repair").await;
    let mut tx = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM queue.queues WHERE tenant_id=$1 AND id=$2 FOR UPDATE")
        .bind(f.tenant)
        .bind(f.queue)
        .execute(tx.connection())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE catalog.services SET enabled=false,version=version+1 WHERE tenant_id=$1 AND id=$2",
    )
    .bind(f.tenant)
    .bind(s.id)
    .execute(tx.connection())
    .await
    .unwrap();
    let pool = f.runtime.clone();
    let (_, hash) = token();
    let mut task =
        tokio::spawn(
            async move { public::issue_for_service(&pool, slug, &hash, Some(s.id)).await },
        );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut task)
            .await
            .is_err()
    );
    tx.commit().await.unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(PublicQueueError::ServiceUnavailable)
    ));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM queue.guest_claims WHERE tenant_id=$1")
            .bind(f.tenant)
            .fetch_one(&f.migrator)
            .await
            .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        public::issue(&f.runtime, slug, &hash).await.unwrap().number,
        1
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn services_http_checks_management_origin_strict_payloads_and_public_projection() {
    let f = Fixture::new().await;
    let login = f.login(f.owner).await;
    let payload =
        json!({"name":"Consultation <script>","description":"Bring a document","enabled":true});
    let mut request = service_write(&f, &login, None, payload.clone());
    request.headers_mut().remove("x-csrf-token");
    assert_eq!(f.json(request).await.0, StatusCode::FORBIDDEN);
    for role in [Role::Operator, Role::Employee] {
        let member = f.member(role).await;
        let login = f.login(member).await;
        assert_eq!(
            f.json(service_write(&f, &login, None, payload.clone()))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }
    for invalid in [
        json!({"name":"","description":"","enabled":true}),
        json!({"name":"Name","description":"bad\nline","enabled":true}),
        json!({"name":"Name","description":"","enabled":true,"extra":true}),
    ] {
        assert_eq!(
            f.json(service_write(&f, &login, None, invalid)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let (status, saved) = f.json(service_write(&f, &login, None, payload)).await;
    assert_eq!(status, StatusCode::CREATED);
    let id = Uuid::parse_str(saved["id"].as_str().unwrap()).unwrap();
    let slug = f.publish().await;
    let url = format!("/api/public/queues/{slug}/services");
    let (status, page) = f
        .json(Request::get(&url).body(Body::empty()).unwrap())
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"][0].as_object().unwrap().len(), 3);
    for suffix in [
        "?limit=0",
        "?limit=101",
        "?after=bad",
        "?extra=1",
        "?limit=1&limit=2",
    ] {
        assert_eq!(
            f.json(
                Request::get(format!("{url}{suffix}"))
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        f.json(
            Request::get(format!("/api/public/queues/{}/services", Uuid::new_v4()))
                .body(Body::empty())
                .unwrap()
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (secret, _) = token();
    let mut issue = guest_request(slug, &secret, "POST");
    *issue.body_mut() = Body::from(json!({"service_id":id}).to_string());
    let (_, ticket) = f.json(issue).await;
    assert_eq!(ticket["service_name"], "Consultation <script>");
    assert!(ticket.get("service_id").is_none());
    let changed = json!({"name":"New name","description":"","enabled":false,"expected_version":1});
    assert_eq!(
        f.json(service_write(&f, &login, Some(id), changed.clone()))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        f.json(service_write(&f, &login, Some(id), changed)).await.0,
        StatusCode::CONFLICT
    );
    assert!(
        f.json(Request::get(url).body(Body::empty()).unwrap())
            .await
            .1["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.json(guest_request(slug, &secret, "GET")).await.1["service_name"],
        "Consultation <script>"
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn services_keyset_pages_and_sse_revision_do_not_publish_service_on_display() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let s = service(&f, "A").await;
    service(&f, "B").await;
    let first = public::services(&f.runtime, slug, None, 1).await.unwrap();
    assert_eq!(first.len(), 2);
    let next = public::services(&f.runtime, slug, Some(first[0].id), 1)
        .await
        .unwrap();
    assert_eq!(next.len(), 1);
    assert_ne!(first[0].id, next[0].id);
    let (app, hub, task) = live_app(&f);
    let response = app
        .oneshot(
            Request::get(format!("/api/public/queues/{slug}/events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    let before = until_event(&mut stream, "refresh").await;
    turnesta_api::catalog::save(
        &f.runtime,
        f.owner,
        f.tenant,
        f.queue,
        Some((s.id, 1)),
        turnesta_api::catalog::ServiceDraft {
            name: &Name::parse("Changed service").unwrap(),
            description: "",
            enabled: false,
        },
    )
    .await
    .unwrap();
    let after = until_event(&mut stream, "refresh").await;
    assert_ne!(before, after);
    assert!(!after.contains(&s.id.to_string()));
    assert!(!after.contains("Changed service"));
    public::configure_display(&f.runtime, f.owner, f.tenant, f.queue, true, 1)
        .await
        .unwrap();
    let active = public::services(&f.runtime, slug, None, 50).await.unwrap();
    let (_, hash) = token();
    public::issue_for_service(&f.runtime, slug, &hash, Some(active[0].id))
        .await
        .unwrap();
    assert!(
        f.staff(Command::CallNext { station_id: None })
            .await
            .ticket
            .is_some()
    );
    let display =
        serde_json::to_value(public::display(&f.runtime, slug, 0).await.unwrap()).unwrap();
    for called in display["called"].as_array().unwrap() {
        assert_eq!(called.as_object().unwrap().len(), 3);
    }
    drop(stream);
    stop_live(hub, task).await;
    f.close().await;
}

async fn pool(variable: &str, connections: u32) -> PgPool {
    let pool = PgPoolOptions::new()
        .max_connections(connections)
        .acquire_timeout(Duration::from_secs(3))
        .connect(&std::env::var(variable).expect("test database URL required"))
        .await
        .unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        database.ends_with("_test"),
        "requires a dedicated *_test database"
    );
    pool
}

fn token() -> (String, [u8; 32]) {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).unwrap();
    (hex::encode(bytes), Sha256::digest(bytes).into())
}

struct Fixture {
    runtime: PgPool,
    admin: PgPool,
    migrator: PgPool,
    tenant: TenantId,
    owner: UserId,
    queue: Uuid,
    app: Router,
}

async fn station(f: &Fixture, name: &str) -> turnesta_api::queue_stations::Station {
    turnesta_api::queue_stations::save(
        &f.runtime,
        f.owner,
        f.tenant,
        f.queue,
        None,
        turnesta_api::queue_stations::StationDraft {
            name: &Name::parse(name).unwrap(),
            enabled: true,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn station_concurrent_calls_reserve_once_and_release_after_close() {
    use turnesta_api::queue_stations;
    let f = Fixture::new().await;
    let s = station(&f, "Window 1").await;
    let other = station(&f, "Window 2").await;
    let operator = f.member(Role::Operator).await;
    let first = f
        .staff(Command::Issue { service_id: None })
        .await
        .ticket
        .unwrap();
    let second = f
        .staff(Command::Issue { service_id: None })
        .await
        .ticket
        .unwrap();
    let call = Command::CallNext {
        station_id: Some(s.id),
    };
    let (a, b) = tokio::join!(
        queue::execute(
            &f.runtime,
            f.owner,
            f.tenant,
            f.queue,
            Uuid::new_v4(),
            call.clone()
        ),
        queue::execute(
            &f.runtime,
            operator,
            f.tenant,
            f.queue,
            Uuid::new_v4(),
            call
        )
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let (called, loser) = match (a, b) {
        (Ok(result), Err(queue::QueueError::StationBusy)) => (result.ticket.unwrap(), operator),
        (Err(queue::QueueError::StationBusy), Ok(result)) => (result.ticket.unwrap(), f.owner),
        result => panic!("unexpected concurrent calls: {result:?}"),
    };
    assert_eq!(called.id, first.id);
    let called2 = queue::execute(
        &f.runtime,
        loser,
        f.tenant,
        f.queue,
        Uuid::new_v4(),
        Command::CallNext {
            station_id: Some(other.id),
        },
    )
    .await
    .unwrap()
    .ticket
    .unwrap();
    assert_eq!(called2.id, second.id);
    let states = queue_stations::list(&f.runtime, f.owner, f.tenant, f.queue, None, 50)
        .await
        .unwrap();
    assert!(states.iter().all(|s| s.busy));
    let mut raw = f.migrator.begin().await.unwrap();
    let error = sqlx::query("UPDATE queue.tickets SET station_id=$4,station_name='Collision' WHERE tenant_id=$1 AND queue_id=$2 AND id=$3")
        .bind(f.tenant).bind(f.queue).bind(called2.id).bind(s.id).execute(&mut *raw).await.unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().constraint(),
        Some("tickets_station_active_idx")
    );
    raw.rollback().await.unwrap();
    f.staff(Command::Complete {
        ticket_id: called.id,
    })
    .await;
    f.staff(Command::Cancel {
        ticket_id: called2.id,
    })
    .await;
    let third = f
        .staff(Command::Issue { service_id: None })
        .await
        .ticket
        .unwrap();
    assert_eq!(
        f.staff(Command::CallNext {
            station_id: Some(s.id)
        })
        .await
        .ticket
        .unwrap()
        .id,
        third.id
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn station_archive_serializes_with_call_and_leaves_waiting_ticket_untouched() {
    let f = Fixture::new().await;
    let s = station(&f, "Window 1").await;
    let ticket = f
        .staff(Command::Issue { service_id: None })
        .await
        .ticket
        .unwrap();
    let mut tx = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM queue.queues WHERE tenant_id=$1 AND id=$2 FOR UPDATE")
        .bind(f.tenant)
        .bind(f.queue)
        .execute(tx.connection())
        .await
        .unwrap();
    sqlx::query("UPDATE queue.stations SET enabled=false,version=version+1 WHERE tenant_id=$1 AND queue_id=$2 AND id=$3")
        .bind(f.tenant).bind(f.queue).bind(s.id).execute(tx.connection()).await.unwrap();
    let pool = f.runtime.clone();
    let owner = f.owner;
    let tenant = f.tenant;
    let queue = f.queue;
    let mut task = tokio::spawn(async move {
        queue::execute(
            &pool,
            owner,
            tenant,
            queue,
            Uuid::new_v4(),
            Command::CallNext {
                station_id: Some(s.id),
            },
        )
        .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut task)
            .await
            .is_err()
    );
    tx.commit().await.unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(queue::QueueError::StationUnavailable)
    ));
    let result = f
        .staff(Command::CallNext { station_id: None })
        .await
        .ticket
        .unwrap();
    assert_eq!(result.id, ticket.id);
    assert!(result.station_name.is_none());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn station_changes_wake_sse_with_only_opaque_revision_hints() {
    use turnesta_api::queue_stations::{self, StationDraft};
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let (app, hub, task) = live_app(&f);
    let response = app
        .oneshot(
            Request::get(format!("/api/public/queues/{slug}/events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    let initial = until_event(&mut stream, "refresh").await;
    let s = station(&f, "Window private-to-stream").await;
    let created = until_event(&mut stream, "refresh").await;
    assert_ne!(initial, created);
    assert!(!created.contains(&s.id.to_string()));
    assert!(!created.contains("Window"));
    queue_stations::save(
        &f.runtime,
        f.owner,
        f.tenant,
        f.queue,
        Some((s.id, s.version)),
        StationDraft {
            name: &Name::parse("Renamed").unwrap(),
            enabled: false,
        },
    )
    .await
    .unwrap();
    let updated = until_event(&mut stream, "refresh").await;
    assert_ne!(created, updated);
    assert!(!updated.contains("Renamed"));
    drop(stream);
    hub.shutdown();
    task.await.unwrap();
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn station_snapshot_replay_disable_and_public_projections() {
    use turnesta_api::queue_stations::{self, StationDraft};
    let f = Fixture::new().await;
    let slug = f.publish().await;
    public::configure_display(&f.runtime, f.owner, f.tenant, f.queue, true, 1)
        .await
        .unwrap();
    let s = station(&f, "Window <1>").await;
    let (_, hash) = token();
    assert!(
        public::issue(&f.runtime, slug, &hash)
            .await
            .unwrap()
            .station_name
            .is_none()
    );
    let key = Uuid::new_v4();
    let command = Command::CallNext {
        station_id: Some(s.id),
    };
    let receipt = queue::execute(&f.runtime, f.owner, f.tenant, f.queue, key, command.clone())
        .await
        .unwrap();
    let saved = queue_stations::save(
        &f.runtime,
        f.owner,
        f.tenant,
        f.queue,
        Some((s.id, s.version)),
        StationDraft {
            name: &Name::parse("Renamed").unwrap(),
            enabled: false,
        },
    )
    .await
    .unwrap();
    assert!(saved.busy);
    let guest = public::ticket(&f.runtime, slug, &hash).await.unwrap();
    assert_eq!(guest.station_name.as_deref(), Some("Window <1>"));
    let guest_json = serde_json::to_value(&guest).unwrap();
    assert!(guest_json.get("station_id").is_none());
    let display =
        serde_json::to_value(public::display(&f.runtime, slug, 0).await.unwrap()).unwrap();
    assert_eq!(display["called"][0]["station_name"], "Window <1>");
    assert_eq!(display["called"][0].as_object().unwrap().len(), 3);
    assert!(!display.to_string().contains(&s.id.to_string()));
    assert_eq!(
        queue::execute(&f.runtime, f.owner, f.tenant, f.queue, key, command)
            .await
            .unwrap(),
        receipt
    );
    assert!(matches!(
        queue::execute(
            &f.runtime,
            f.owner,
            f.tenant,
            f.queue,
            key,
            Command::CallNext { station_id: None }
        )
        .await,
        Err(queue::QueueError::IdempotencyConflict)
    ));
    f.staff(Command::Complete {
        ticket_id: receipt.ticket.unwrap().id,
    })
    .await;
    assert!(matches!(
        queue::execute(
            &f.runtime,
            f.owner,
            f.tenant,
            f.queue,
            Uuid::new_v4(),
            Command::CallNext {
                station_id: Some(s.id)
            }
        )
        .await,
        Err(queue::QueueError::StationUnavailable)
    ));
    assert_eq!(
        public::ticket(&f.runtime, slug, &hash)
            .await
            .unwrap()
            .station_name
            .as_deref(),
        Some("Window <1>")
    );
    assert!(
        public::display(&f.runtime, slug, 0)
            .await
            .unwrap()
            .called
            .is_empty()
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn stations_roles_rls_fk_versions_and_pagination() {
    use turnesta_api::queue_stations::{self, StationDraft};
    let f = Fixture::new().await;
    let foreign = Fixture::new().await;
    let s = station(&f, "Window").await;
    let foreign_station = station(&foreign, "Foreign window").await;
    let operator = f.member(Role::Operator).await;
    let employee = f.member(Role::Employee).await;
    let manager = f.member(Role::Manager).await;
    for user in [operator, employee] {
        assert_eq!(
            queue_stations::list(&f.runtime, user, f.tenant, f.queue, None, 50)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(matches!(
            queue_stations::save(
                &f.runtime,
                user,
                f.tenant,
                f.queue,
                None,
                StationDraft {
                    name: &Name::parse("No").unwrap(),
                    enabled: true
                }
            )
            .await,
            Err(queue::QueueError::Forbidden)
        ));
        let mut tx = TenantSession::begin(&f.runtime, user, f.tenant)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query("UPDATE queue.stations SET enabled=false")
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
        .bind(foreign.tenant.to_string())
        .execute(tx.connection())
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM queue.stations")
            .fetch_one(tx.connection())
            .await
            .unwrap(),
        0
    );
    tx.commit().await.unwrap();
    assert!(matches!(
        queue::execute(
            &f.runtime,
            f.owner,
            f.tenant,
            f.queue,
            Uuid::new_v4(),
            Command::CallNext {
                station_id: Some(foreign_station.id)
            }
        )
        .await,
        Err(queue::QueueError::StationUnavailable)
    ));
    let name = Name::parse("Changed").unwrap();
    let (a, b) = tokio::join!(
        queue_stations::save(
            &f.runtime,
            manager,
            f.tenant,
            f.queue,
            Some((s.id, s.version)),
            StationDraft {
                name: &name,
                enabled: true
            }
        ),
        queue_stations::save(
            &f.runtime,
            f.owner,
            f.tenant,
            f.queue,
            Some((s.id, s.version)),
            StationDraft {
                name: &name,
                enabled: false
            }
        )
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(
        matches!(a, Err(queue::QueueError::Core(CoreError::VersionConflict)))
            || matches!(b, Err(queue::QueueError::Core(CoreError::VersionConflict)))
    );
    station(&f, "Second").await;
    let page = queue_stations::list(&f.runtime, f.owner, f.tenant, f.queue, None, 1)
        .await
        .unwrap();
    assert_eq!(page.len(), 2);
    let next = queue_stations::list(&f.runtime, f.owner, f.tenant, f.queue, Some(page[0].id), 1)
        .await
        .unwrap();
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].id, page[1].id);
    let ticket = f
        .staff(Command::Issue { service_id: None })
        .await
        .ticket
        .unwrap();
    let mut raw = f.migrator.begin().await.unwrap();
    let error=sqlx::query("UPDATE queue.tickets SET station_id=$4,station_name='Foreign',status='called',called_by=$5,called_at=clock_timestamp() WHERE tenant_id=$1 AND queue_id=$2 AND id=$3")
        .bind(f.tenant).bind(f.queue).bind(ticket.id).bind(foreign_station.id).bind(f.owner).execute(&mut *raw).await.unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23503")
    );
    raw.rollback().await.unwrap();
    let mut raw = f.migrator.begin().await.unwrap();
    let error = sqlx::query(
        "INSERT INTO queue.stations(tenant_id,queue_id,id,name) VALUES($1,$2,$3,'bad\nname')",
    )
    .bind(f.tenant)
    .bind(f.queue)
    .bind(Uuid::new_v4())
    .execute(&mut *raw)
    .await
    .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    raw.rollback().await.unwrap();
    foreign.close().await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn station_http_requires_management_csrf_and_strict_payloads() {
    let f = Fixture::new().await;
    let url = format!(
        "/api/organizations/{}/queues/{}/stations",
        f.tenant, f.queue
    );
    let owner = f.login(f.owner).await;
    let operator = f.member(Role::Operator).await;
    let operator = f.login(operator).await;
    let post = |login: &(String, String), body: Value| {
        Request::post(&url)
            .header("cookie", &login.0)
            .header("origin", ORIGIN)
            .header("x-csrf-token", &login.1)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let body = json!({"name":"Window 1","enabled":true});
    assert_eq!(
        f.json(post(&operator, body.clone())).await.0,
        StatusCode::FORBIDDEN
    );
    let mut no_origin = post(&owner, body.clone());
    no_origin.headers_mut().remove("origin");
    assert_eq!(f.json(no_origin).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        f.json(post(
            &owner,
            json!({"name":"Window","enabled":true,"queue_id":f.queue})
        ))
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.json(post(&owner, json!({"name":"\n","enabled":true})))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (status, s) = f.json(post(&owner, body)).await;
    assert_eq!(status, StatusCode::CREATED);
    let patch = || {
        Request::patch(format!("{url}/{}", s["id"].as_str().unwrap()))
            .header("cookie", &owner.0)
            .header("origin", ORIGIN)
            .header("x-csrf-token", &owner.1)
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"name":"Renamed","enabled":false,"expected_version":1}).to_string(),
            ))
            .unwrap()
    };
    assert_eq!(f.json(patch()).await.0, StatusCode::OK);
    assert_eq!(f.json(patch()).await.0, StatusCode::CONFLICT);
    for query in ["?unexpected=1", "?limit=0", "?limit=1&limit=2"] {
        assert_eq!(
            f.json(
                Request::get(format!("{url}{query}"))
                    .header("cookie", &owner.0)
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let (status, page) = f
        .json(
            Request::get(&url)
                .header("cookie", &operator.0)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"][0]["enabled"], false);
    let command_url = format!(
        "/api/organizations/{}/queues/{}/commands",
        f.tenant, f.queue
    );
    let request = Request::post(command_url)
        .header("cookie", &owner.0)
        .header("origin", ORIGIN)
        .header("x-csrf-token", &owner.1)
        .header("idempotency-key", Uuid::new_v4().to_string())
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"type":"call_next","station_id":s["id"]}).to_string(),
        ))
        .unwrap();
    let (status, error) = f.json(request).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["code"], "station_unavailable");
    f.close().await;
}

#[test]
fn legacy_call_and_receipt_decode_without_station() {
    let command: Command = serde_json::from_value(json!({"type":"call_next"})).unwrap();
    assert_eq!(command, Command::CallNext { station_id: None });
    assert_eq!(
        serde_json::to_value(&command).unwrap(),
        json!({"type":"call_next"})
    );
    let ticket:queue::Ticket=serde_json::from_value(json!({"id":Uuid::new_v4(),"queue_id":Uuid::new_v4(),"position":1,"service_date":"2026-11-01","number":1,"status":"called","called_by":Uuid::new_v4(),"issued_at":"x","called_at":"x","closed_at":null})).unwrap();
    assert!(ticket.station_id.is_none() && ticket.station_name.is_none());
}
impl Fixture {
    async fn new() -> Self {
        let runtime = pool("TEST_DATABASE_URL", 5).await;
        let admin = pool("TEST_ADMIN_DATABASE_URL", 2).await;
        let migrator = pool("TEST_MIGRATION_DATABASE_URL", 1).await;
        let org = Provisioner::new(admin.clone())
            .create_organization(
                &Name::parse("Public test").unwrap(),
                &Name::parse("Owner").unwrap(),
            )
            .await
            .unwrap();
        let tenant = org.organization.id;
        let owner = org.owner.id;
        let mut tx = TenantSession::begin(&runtime, owner, tenant).await.unwrap();
        let point = tx
            .create_location(&Name::parse("Point").unwrap(), "Pacific/Kiritimati")
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let queue = queue::create(
            &runtime,
            owner,
            tenant,
            point.id,
            &Name::parse("Очередь <script>alert(1)</script>").unwrap(),
        )
        .await
        .unwrap()
        .id;
        let app = router(AppState::new(runtime.clone(), Duration::from_secs(1)));
        Self {
            runtime,
            admin,
            migrator,
            tenant,
            owner,
            queue,
            app,
        }
    }
    async fn publish(&self) -> Uuid {
        public::configure(&self.runtime, self.owner, self.tenant, self.queue, true, 0)
            .await
            .unwrap()
            .public_slug
            .unwrap()
    }
    async fn member(&self, role: Role) -> UserId {
        let provisioner = Provisioner::new(self.admin.clone());
        let user = provisioner
            .create_user(&Name::parse("Member").unwrap())
            .await
            .unwrap();
        provisioner
            .add_membership(self.tenant, user.id, role)
            .await
            .unwrap();
        user.id
    }
    async fn staff(&self, command: Command) -> queue::CommandResult {
        queue::execute(
            &self.runtime,
            self.owner,
            self.tenant,
            self.queue,
            Uuid::new_v4(),
            command,
        )
        .await
        .unwrap()
    }
    async fn login(&self, user: UserId) -> (String, String) {
        let email = Email::parse(&format!("guest-{}@example.test", Uuid::new_v4())).unwrap();
        login::bind_email(&self.admin, user, &email).await.unwrap();
        let (code, hash) = token();
        login::issue_login_code(&self.admin, &email, &hash, 600)
            .await
            .unwrap();
        let response = self
            .app
            .clone()
            .oneshot(
                Request::post("/auth/login")
                    .header("origin", ORIGIN)
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"code":code}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let cookies: Vec<&str> = response
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .map(|c| c.to_str().unwrap().split(';').next().unwrap())
            .collect();
        let csrf = cookies
            .iter()
            .find_map(|c| c.strip_prefix("turnesta_csrf="))
            .unwrap()
            .to_owned();
        (cookies.join("; "), csrf)
    }
    fn settings_url(&self) -> String {
        format!(
            "/api/organizations/{}/queues/{}/public-access",
            self.tenant, self.queue
        )
    }
    async fn json(&self, request: Request<Body>) -> (StatusCode, Value) {
        let limit = if request.uri().path() == "/openapi.json" {
            256 * 1024
        } else {
            64 * 1024
        };
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), limit).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }
    async fn close(self) {
        self.runtime.close().await;
        self.admin.close().await;
        self.migrator.close().await;
    }
}

fn guest_request(slug: Uuid, secret: &str, method: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(format!("/api/public/queues/{slug}/ticket"))
        .header("origin", ORIGIN)
        .header("x-queue-ticket", secret)
        .header("content-type", "application/json")
        .body(if method == "POST" {
            Body::from("{}")
        } else {
            Body::empty()
        })
        .unwrap()
}

fn tip_configuration(f: &Fixture, login: &(String, String), body: Value) -> Request<Body> {
    Request::put(format!(
        "/api/organizations/{}/queues/{}/tips",
        f.tenant, f.queue
    ))
    .header("cookie", &login.0)
    .header("origin", ORIGIN)
    .header("x-csrf-token", &login.1)
    .header("content-type", "application/json")
    .body(Body::from(body.to_string()))
    .unwrap()
}

fn mock_app(f: &Fixture) -> Router {
    router(
        AppState::new(f.runtime.clone(), Duration::from_secs(1)).with_payments(
            turnesta_api::payments::PaymentConfig::mock(
                &"37".repeat(32),
                "http://127.0.0.1:8080/hooks/tips/mock",
            )
            .unwrap(),
        ),
    )
}

async fn queue_recipient(f: &Fixture, name: &str) -> turnesta_api::tips::Recipient {
    let point = queue::get(&f.runtime, f.owner, f.tenant, f.queue)
        .await
        .unwrap()
        .location_id;
    turnesta_api::tips::create_recipient(
        &f.runtime,
        f.owner,
        f.tenant,
        point,
        &Name::parse(name).unwrap(),
    )
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn queue_tips_settings_enforce_roles_location_tenant_and_optimistic_version() {
    use turnesta_api::queue_tips;
    let f = Fixture::new().await;
    let initial = queue_tips::settings(&f.runtime, f.owner, f.tenant, f.queue)
        .await
        .unwrap();
    assert_eq!(initial.version, 0);
    assert!(initial.recipient_id.is_none());
    let recipient = queue_recipient(&f, "Team").await;
    for role in [Role::Operator, Role::Employee] {
        let user = f.member(role).await;
        assert!(
            queue_tips::settings(&f.runtime, user, f.tenant, f.queue)
                .await
                .is_err()
        );
        assert!(
            queue_tips::configure(&f.runtime, user, f.tenant, f.queue, Some(recipient.id), 0)
                .await
                .is_err()
        );
        let mut tx = TenantSession::begin(&f.runtime, user, f.tenant)
            .await
            .unwrap();
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM queue.tip_settings")
            .fetch_one(tx.connection())
            .await
            .unwrap();
        assert_eq!(rows, 0);
        let error = sqlx::query("INSERT INTO queue.tip_settings (tenant_id,queue_id,location_id,recipient_id) SELECT tenant_id,id,location_id,$3 FROM queue.queues WHERE tenant_id=$1 AND id=$2")
            .bind(f.tenant).bind(f.queue).bind(recipient.id).execute(tx.connection()).await.unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("42501")
        );
    }
    let manager = f.member(Role::Manager).await;
    let (a, b) = tokio::join!(
        queue_tips::configure(
            &f.runtime,
            manager,
            f.tenant,
            f.queue,
            Some(recipient.id),
            0
        ),
        queue_tips::configure(
            &f.runtime,
            f.owner,
            f.tenant,
            f.queue,
            Some(recipient.id),
            0
        ),
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let actual = queue_tips::settings(&f.runtime, f.owner, f.tenant, f.queue)
        .await
        .unwrap();
    assert_eq!(actual.version, 1);
    assert_eq!(actual.public_slug, Some(recipient.public_slug));
    for role in [Role::Operator, Role::Employee] {
        let user = f.member(role).await;
        let mut tx = TenantSession::begin(&f.runtime, user, f.tenant)
            .await
            .unwrap();
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM queue.tip_settings")
            .fetch_one(tx.connection())
            .await
            .unwrap();
        assert_eq!(
            rows, 0,
            "private settings became visible after configuration"
        );
        let updated = sqlx::query("UPDATE queue.tip_settings SET recipient_id=NULL")
            .execute(tx.connection())
            .await
            .unwrap();
        assert_eq!(updated.rows_affected(), 0);
        tx.commit().await.unwrap();
    }
    let mut tx = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    let elsewhere = tx
        .create_location(&Name::parse("Other point").unwrap(), "UTC")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let wrong_point = turnesta_api::tips::create_recipient(
        &f.runtime,
        f.owner,
        f.tenant,
        elsewhere.id,
        &Name::parse("Elsewhere").unwrap(),
    )
    .await
    .unwrap();
    assert!(matches!(
        queue_tips::configure(
            &f.runtime,
            f.owner,
            f.tenant,
            f.queue,
            Some(wrong_point.id),
            1
        )
        .await,
        Err(PublicQueueError::NotFound)
    ));
    let other = Fixture::new().await;
    let foreign = queue_recipient(&other, "Foreign").await;
    assert!(matches!(
        queue_tips::configure(&f.runtime, f.owner, f.tenant, f.queue, Some(foreign.id), 1).await,
        Err(PublicQueueError::NotFound)
    ));
    assert!(
        queue_tips::settings(&f.runtime, other.owner, f.tenant, f.queue)
            .await
            .is_err()
    );
    let error =
        sqlx::query("UPDATE tips.recipients SET location_id=$3 WHERE tenant_id=$1 AND id=$2")
            .bind(f.tenant)
            .bind(recipient.id)
            .bind(elsewhere.id)
            .execute(&f.migrator)
            .await
            .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23503")
    );
    let mut tx = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    let error = sqlx::query(
        "UPDATE queue.tip_settings SET recipient_id=$3 WHERE tenant_id=$1 AND queue_id=$2",
    )
    .bind(f.tenant)
    .bind(f.queue)
    .bind(wrong_point.id)
    .execute(tx.connection())
    .await
    .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23503")
    );
    drop(tx);
    other.close().await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn completed_guest_tips_keep_capability_ttl_and_disabled_mode_boundaries() {
    use turnesta_api::queue_tips;
    let mut f = Fixture::new().await;
    let disabled_app = f.app.clone();
    f.app = mock_app(&f);
    let slug = f.publish().await;
    let recipient = queue_recipient(&f, "Team <script>").await;
    queue_tips::configure(
        &f.runtime,
        f.owner,
        f.tenant,
        f.queue,
        Some(recipient.id),
        0,
    )
    .await
    .unwrap();
    let (secret, hash) = token();
    let (_, waiting) = f.json(guest_request(slug, &secret, "POST")).await;
    assert!(waiting["tips"].is_null());
    let called = f
        .staff(Command::CallNext { station_id: None })
        .await
        .ticket
        .unwrap();
    assert!(f.json(guest_request(slug, &secret, "GET")).await.1["tips"].is_null());
    f.staff(Command::Complete {
        ticket_id: called.id,
    })
    .await;
    let (_, done) = f.json(guest_request(slug, &secret, "GET")).await;
    assert_eq!(
        done["tips"],
        json!({"public_slug":recipient.public_slug,"display_name":"Team <script>","environment":"mock"})
    );
    assert!(!done.to_string().contains(&secret));
    assert!(!done.to_string().contains(&recipient.id.to_string()));
    assert_eq!(f.json(guest_request(slug, &secret, "POST")).await.1, done);
    let disabled = disabled_app
        .oneshot(guest_request(slug, &secret, "GET"))
        .await
        .unwrap();
    let disabled: Value =
        serde_json::from_slice(&to_bytes(disabled.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert!(disabled["tips"].is_null());
    assert_eq!(
        f.json(guest_request(slug, &token().0, "GET")).await.0,
        StatusCode::NOT_FOUND
    );
    let replacement = queue_recipient(&f, "New team").await;
    queue_tips::configure(
        &f.runtime,
        f.owner,
        f.tenant,
        f.queue,
        Some(replacement.id),
        1,
    )
    .await
    .unwrap();
    assert_eq!(
        public::ticket(&f.runtime, slug, &hash)
            .await
            .unwrap()
            .tips
            .unwrap()
            .public_slug,
        replacement.public_slug
    );
    queue_tips::configure(&f.runtime, f.owner, f.tenant, f.queue, None, 2)
        .await
        .unwrap();
    assert!(f.json(guest_request(slug, &secret, "GET")).await.1["tips"].is_null());
    queue_tips::configure(
        &f.runtime,
        f.owner,
        f.tenant,
        f.queue,
        Some(recipient.id),
        3,
    )
    .await
    .unwrap();
    let (_, cancelled_hash) = token();
    public::issue(&f.runtime, slug, &cancelled_hash)
        .await
        .unwrap();
    let cancelled = f
        .staff(Command::CallNext { station_id: None })
        .await
        .ticket
        .unwrap();
    f.staff(Command::Cancel {
        ticket_id: cancelled.id,
    })
    .await;
    assert!(
        public::ticket(&f.runtime, slug, &cancelled_hash)
            .await
            .unwrap()
            .tips
            .is_none()
    );
    // Same terminal TTL as the ticket; tip access cannot extend it.
    sqlx::query("UPDATE queue.tickets SET closed_at=clock_timestamp()-interval '25 hours' WHERE tenant_id=$1 AND queue_id=$2 AND id=$3")
        .bind(f.tenant).bind(f.queue).bind(called.id).execute(&f.migrator).await.unwrap();
    assert_eq!(
        f.json(guest_request(slug, &secret, "GET")).await.0,
        StatusCode::GONE
    );
    assert_eq!(
        f.json(guest_request(slug, &secret, "POST")).await.0,
        StatusCode::GONE
    );
    let mut tx = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    let attempts: i64 = sqlx::query_scalar("SELECT count(*) FROM tips.attempts WHERE tenant_id=$1")
        .bind(f.tenant)
        .fetch_one(tx.connection())
        .await
        .unwrap();
    assert_eq!(
        attempts, 0,
        "reading or configuring a link created a payment attempt"
    );
    tx.commit().await.unwrap();
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn queue_tips_http_requires_management_csrf_and_explicit_nullable_recipient() {
    let mut f = Fixture::new().await;
    let disabled_app = f.app.clone();
    f.app = mock_app(&f);
    let spec = f
        .json(Request::get("/openapi.json").body(Body::empty()).unwrap())
        .await
        .1;
    assert!(
        spec["components"]["schemas"]["ConfigureTipsRequest"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("recipient_id"))
    );
    let login = f.login(f.owner).await;
    let recipient = queue_recipient(&f, "Team").await;
    let url = format!("/api/organizations/{}/queues/{}/tips", f.tenant, f.queue);
    assert_eq!(
        f.json(Request::get(&url).body(Body::empty()).unwrap())
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    for role in [Role::Operator, Role::Employee] {
        let user = f.member(role).await;
        let member = f.login(user).await;
        assert_eq!(
            f.json(
                Request::get(&url)
                    .header("cookie", &member.0)
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            f.json(tip_configuration(
                &f,
                &member,
                json!({"recipient_id":null,"expected_version":0})
            ))
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    let mut missing_csrf = tip_configuration(
        &f,
        &login,
        json!({"recipient_id":null,"expected_version":0}),
    );
    missing_csrf.headers_mut().remove("x-csrf-token");
    assert_eq!(f.json(missing_csrf).await.0, StatusCode::FORBIDDEN);
    let mut wrong_origin = tip_configuration(
        &f,
        &login,
        json!({"recipient_id":null,"expected_version":0}),
    );
    wrong_origin
        .headers_mut()
        .insert("origin", "https://wrong.example".parse().unwrap());
    assert_eq!(f.json(wrong_origin).await.0, StatusCode::FORBIDDEN);
    for body in [
        json!({"expected_version":0}),
        json!({"recipient_id":"bad","expected_version":0}),
        json!({"recipient_id":false,"expected_version":0}),
        json!({"recipient_id":null,"expected_version":-1}),
        json!({"recipient_id":null,"expected_version":0,"extra":true}),
    ] {
        assert_eq!(
            f.json(tip_configuration(&f, &login, body)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let (status, saved) = f
        .json(tip_configuration(
            &f,
            &login,
            json!({"recipient_id":recipient.id,"expected_version":0}),
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["version"], 1);
    assert_eq!(saved["recipient_name"], "Team");
    assert_eq!(
        f.json(tip_configuration(
            &f,
            &login,
            json!({"recipient_id":null,"expected_version":0})
        ))
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.json(tip_configuration(
            &f,
            &login,
            json!({"recipient_id":null,"expected_version":1})
        ))
        .await
        .1["recipient_id"],
        Value::Null
    );
    let disabled = disabled_app
        .oneshot(
            Request::get(url)
                .header("cookie", &login.0)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(disabled.status(), StatusCode::SERVICE_UNAVAILABLE);
    let acl: bool = sqlx::query_scalar("SELECT has_function_privilege('turnesta_runtime','queue.public_ticket_with_tips(uuid,bytea)','EXECUTE') AND NOT EXISTS (SELECT 1 FROM pg_proc p, LATERAL aclexplode(p.proacl) a WHERE p.oid='queue.public_ticket_with_tips(uuid,bytea)'::regprocedure AND a.grantee=0 AND a.privilege_type='EXECUTE')")
        .fetch_one(&f.migrator).await.unwrap();
    assert!(acl);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn queue_tips_settings_wake_sse_without_publishing_recipient_or_financial_data() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let recipient = queue_recipient(&f, "Team").await;
    let (app, hub, task) = live_app(&f);
    let response = app
        .oneshot(
            Request::get(format!("/api/public/queues/{slug}/events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    let initial = until_event(&mut stream, "refresh").await;
    turnesta_api::queue_tips::configure(
        &f.runtime,
        f.owner,
        f.tenant,
        f.queue,
        Some(recipient.id),
        0,
    )
    .await
    .unwrap();
    let changed = until_event(&mut stream, "refresh").await;
    assert_ne!(initial, changed);
    assert!(!changed.contains(&recipient.id.to_string()));
    assert!(!changed.contains(&recipient.public_slug.to_string()));
    let profile = f
        .json(
            Request::get(format!("/api/public/queues/{slug}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .1;
    assert!(profile.get("tips").is_none());
    assert!(
        !profile
            .to_string()
            .contains(&recipient.public_slug.to_string())
    );
    public::configure_display(&f.runtime, f.owner, f.tenant, f.queue, true, 1)
        .await
        .unwrap();
    assert!(
        !serde_json::to_string(&public::display(&f.runtime, slug, 0).await.unwrap())
            .unwrap()
            .contains(&recipient.public_slug.to_string())
    );
    drop(stream);
    stop_live(hub, task).await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn publication_requires_manager_and_keeps_optimistic_versions_and_tenant_isolation() {
    let f = Fixture::new().await;
    let initial = public::settings(&f.runtime, f.owner, f.tenant, f.queue)
        .await
        .unwrap();
    assert!(initial.public_slug.is_none());
    assert_eq!(initial.version, 0);
    for role in [Role::Operator, Role::Employee] {
        let user = f.member(role).await;
        assert!(matches!(
            public::configure(&f.runtime, user, f.tenant, f.queue, true, 0).await,
            Err(PublicQueueError::Core(_))
        ));
        let mut tx = TenantSession::begin(&f.runtime, user, f.tenant)
            .await
            .unwrap();
        let error = sqlx::query("INSERT INTO queue.public_settings (tenant_id, queue_id, public_slug, join_open) VALUES ($1,$2,$3,true)")
            .bind(f.tenant).bind(f.queue).bind(Uuid::new_v4()).execute(tx.connection()).await.unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("42501")
        );
    }
    let slug = f.publish().await;
    let manager = f.member(Role::Manager).await;
    let closed = public::configure(&f.runtime, manager, f.tenant, f.queue, false, 1)
        .await
        .unwrap();
    assert_eq!(closed.public_slug, Some(slug));
    assert_eq!(closed.version, 2);
    assert!(matches!(
        public::configure(&f.runtime, f.owner, f.tenant, f.queue, true, 1).await,
        Err(PublicQueueError::Core(_))
    ));
    assert!(matches!(
        public::settings(&f.runtime, f.owner, f.tenant, Uuid::new_v4()).await,
        Err(PublicQueueError::NotFound)
    ));
    let outsider = Provisioner::new(f.admin.clone())
        .create_user(&Name::parse("Outsider").unwrap())
        .await
        .unwrap();
    assert!(matches!(
        public::settings(&f.runtime, outsider.id, f.tenant, f.queue).await,
        Err(PublicQueueError::Core(_))
    ));
    assert!(!public::profile(&f.runtime, slug).await.unwrap().join_open);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn guest_replay_and_staff_share_fifo_numbers_and_current_private_ticket_state() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let staff = f
        .staff(Command::Issue { service_id: None })
        .await
        .ticket
        .unwrap();
    let (_, first) = token();
    let (_, second) = token();
    let (a, b) = tokio::join!(
        public::issue(&f.runtime, slug, &first),
        public::issue(&f.runtime, slug, &first)
    );
    let a = a.unwrap();
    assert_eq!(a, b.unwrap());
    assert_eq!(a.number, 2);
    assert_eq!(a.people_ahead, Some(1));
    let next = public::issue(&f.runtime, slug, &second).await.unwrap();
    assert_eq!(next.number, 3);
    assert_eq!(next.people_ahead, Some(2));
    let local_date: String = sqlx::query_scalar("SELECT to_char(($1::text::timestamptz AT TIME ZONE 'Pacific/Kiritimati')::date, 'YYYY-MM-DD')")
        .bind(&a.issued_at).fetch_one(&f.runtime).await.unwrap();
    assert_eq!(a.service_date, local_date);
    let called = f
        .staff(Command::CallNext { station_id: None })
        .await
        .ticket
        .unwrap();
    assert_eq!(called.id, staff.id);
    assert_eq!(
        public::ticket(&f.runtime, slug, &first)
            .await
            .unwrap()
            .people_ahead,
        Some(0)
    );
    f.staff(Command::Complete {
        ticket_id: called.id,
    })
    .await;
    let called = f
        .staff(Command::CallNext { station_id: None })
        .await
        .ticket
        .unwrap();
    assert_eq!(called.number, 2);
    let actual = public::issue(&f.runtime, slug, &first).await.unwrap();
    assert_eq!(actual.status, TicketStatus::Called);
    assert_eq!(actual.people_ahead, None);
    assert_eq!(actual.number, a.number);
    f.staff(Command::Complete {
        ticket_id: called.id,
    })
    .await;
    assert_eq!(
        public::ticket(&f.runtime, slug, &first)
            .await
            .unwrap()
            .status,
        TicketStatus::Completed
    );
    assert_eq!(
        public::ticket(&f.runtime, slug, &second)
            .await
            .unwrap()
            .people_ahead,
        Some(0)
    );
    assert_eq!(
        queue::get(&f.runtime, f.owner, f.tenant, f.queue)
            .await
            .unwrap()
            .version,
        8
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn closing_serializes_with_public_issuance_but_does_not_revoke_existing_ticket() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let (_, existing) = token();
    public::issue(&f.runtime, slug, &existing).await.unwrap();
    let mut tx = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM queue.queues WHERE tenant_id=$1 AND id=$2 FOR UPDATE")
        .bind(f.tenant)
        .bind(f.queue)
        .execute(tx.connection())
        .await
        .unwrap();
    sqlx::query("UPDATE queue.public_settings SET join_open=false, version=version+1 WHERE tenant_id=$1 AND queue_id=$2")
        .bind(f.tenant).bind(f.queue).execute(tx.connection()).await.unwrap();
    let guest_pool = pool("TEST_DATABASE_URL", 1).await;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&guest_pool)
        .await
        .unwrap();
    let (_, fresh) = token();
    let blocked_pool = guest_pool.clone();
    let request = tokio::spawn(async move { public::issue(&blocked_pool, slug, &fresh).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT coalesce(wait_event_type='Lock', false) FROM pg_stat_activity WHERE pid=$1",
            )
            .bind(pid)
            .fetch_one(&f.runtime)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("public issuance must wait on the queue lock");
    tx.commit().await.unwrap();
    assert!(matches!(
        request.await.unwrap(),
        Err(PublicQueueError::Closed)
    ));
    assert_eq!(
        public::issue(&f.runtime, slug, &existing)
            .await
            .unwrap()
            .number,
        1
    );
    assert_eq!(
        public::ticket(&f.runtime, slug, &existing)
            .await
            .unwrap()
            .status,
        TicketStatus::Waiting
    );
    assert!(!public::profile(&f.runtime, slug).await.unwrap().join_open);
    guest_pool.close().await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn public_budget_is_serialized_bounded_and_exempts_existing_credentials() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let (_, first) = token();
    public::issue(&f.runtime, slug, &first).await.unwrap();
    for _ in 0..28 {
        public::issue(&f.runtime, slug, &token().1).await.unwrap();
    }
    let (_, a) = token();
    let (_, b) = token();
    let (a, b) = tokio::join!(
        public::issue(&f.runtime, slug, &a),
        public::issue(&f.runtime, slug, &b)
    );
    assert!(matches!(
        (&a, &b),
        (Ok(_), Err(PublicQueueError::RateLimit)) | (Err(PublicQueueError::RateLimit), Ok(_))
    ));
    assert_eq!(
        public::issue(&f.runtime, slug, &first)
            .await
            .unwrap()
            .number,
        1
    );
    assert_eq!(
        queue::get(&f.runtime, f.owner, f.tenant, f.queue)
            .await
            .unwrap()
            .version,
        31
    );
    sqlx::query("UPDATE queue.guest_claims SET created_at=clock_timestamp()-interval '61 seconds' WHERE tenant_id=$1 AND queue_id=$2")
        .bind(f.tenant).bind(f.queue).execute(&f.migrator).await.unwrap();
    assert_eq!(
        public::issue(&f.runtime, slug, &token().1)
            .await
            .unwrap()
            .number,
        31
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn full_waiting_queue_rejects_new_public_ticket_without_consuming_number() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let mut tx = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    sqlx::query("INSERT INTO queue.tickets (tenant_id,queue_id,id,position,service_date,number,issued_at) SELECT $1,$2,gen_random_uuid(),n,current_date,n,clock_timestamp() FROM generate_series(1,500) n")
        .bind(f.tenant).bind(f.queue).execute(tx.connection()).await.unwrap();
    sqlx::query("UPDATE queue.queues SET next_position=500 WHERE tenant_id=$1 AND id=$2")
        .bind(f.tenant)
        .bind(f.queue)
        .execute(tx.connection())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(matches!(
        public::issue(&f.runtime, slug, &token().1).await,
        Err(PublicQueueError::Capacity)
    ));
    assert_eq!(
        queue::get(&f.runtime, f.owner, f.tenant, f.queue)
            .await
            .unwrap()
            .version,
        1
    );
    let claims: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM queue.guest_claims WHERE tenant_id=$1 AND queue_id=$2",
    )
    .bind(f.tenant)
    .bind(f.queue)
    .fetch_one(&f.migrator)
    .await
    .unwrap();
    assert_eq!(claims, 0);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn active_access_outlives_age_but_terminal_expiry_never_reuses_a_credential() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let (_, hash) = token();
    let first = public::issue(&f.runtime, slug, &hash).await.unwrap();
    sqlx::query("UPDATE queue.tickets SET issued_at=clock_timestamp()-interval '8 days' WHERE tenant_id=$1 AND queue_id=$2")
        .bind(f.tenant).bind(f.queue).execute(&f.migrator).await.unwrap();
    assert_eq!(
        public::ticket(&f.runtime, slug, &hash)
            .await
            .unwrap()
            .number,
        first.number
    );
    let called = f
        .staff(Command::CallNext { station_id: None })
        .await
        .ticket
        .unwrap();
    f.staff(Command::Complete {
        ticket_id: called.id,
    })
    .await;
    assert_eq!(
        public::ticket(&f.runtime, slug, &hash)
            .await
            .unwrap()
            .status,
        TicketStatus::Completed
    );
    sqlx::query("UPDATE queue.tickets SET closed_at=clock_timestamp()-interval '25 hours' WHERE tenant_id=$1 AND queue_id=$2")
        .bind(f.tenant).bind(f.queue).execute(&f.migrator).await.unwrap();
    assert!(matches!(
        public::ticket(&f.runtime, slug, &hash).await,
        Err(PublicQueueError::Expired)
    ));
    assert!(matches!(
        public::issue(&f.runtime, slug, &hash).await,
        Err(PublicQueueError::Expired)
    ));
    assert_eq!(
        public::issue(&f.runtime, slug, &token().1)
            .await
            .unwrap()
            .number,
        2
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn private_grants_and_narrow_functions_work_without_a_tenant_context() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let (_, hash) = token();
    public::issue(&f.runtime, slug, &hash).await.unwrap();
    let denied = sqlx::query("SELECT * FROM queue.guest_claims")
        .fetch_all(&f.runtime)
        .await
        .unwrap_err();
    assert_eq!(
        denied.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    let hidden: i64 = sqlx::query_scalar("SELECT count(*) FROM queue.tickets")
        .fetch_one(&f.runtime)
        .await
        .unwrap();
    assert_eq!(hidden, 0);
    let grants_safe: bool = sqlx::query_scalar("SELECT NOT EXISTS (SELECT 1 FROM pg_proc p CROSS JOIN LATERAL aclexplode(p.proacl) a WHERE p.pronamespace='queue'::regnamespace AND p.proname IN ('public_profile','public_ticket','public_issue','allocate_ticket','public_ticket_with_tips','public_ticket_with_service','public_services') AND a.grantee=0 AND a.privilege_type='EXECUTE')")
        .fetch_one(&f.runtime).await.unwrap();
    assert!(grants_safe);
    let functions_safe: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_proc WHERE pronamespace='queue'::regnamespace AND proname IN ('public_profile','public_ticket','public_issue','public_ticket_with_tips','public_ticket_with_service','public_services') AND prosecdef AND 'search_path=pg_catalog'=ANY(proconfig)")
        .fetch_one(&f.runtime).await.unwrap();
    assert_eq!(functions_safe, 7);
    let employee = f.member(Role::Employee).await;
    let mut tx = TenantSession::begin(&f.runtime, employee, f.tenant)
        .await
        .unwrap();
    assert!(
        sqlx::query_scalar::<_, Uuid>("SELECT queue.allocate_ticket($1,$2)")
            .bind(f.tenant)
            .bind(f.queue)
            .fetch_one(tx.connection())
            .await
            .is_err()
    );
    drop(tx);
    assert!(matches!(
        public::ticket(&f.runtime, slug, &token().1).await,
        Err(PublicQueueError::NotFound)
    ));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn anonymous_http_requires_capability_origin_and_strict_body_without_leaking_private_fields()
{
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let (secret, _) = token();
    let base = format!("/api/public/queues/{slug}");
    let response = f
        .app
        .clone()
        .oneshot(
            Request::get(format!("{base}/ticket"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(!response.headers().contains_key("location"));
    for bad in ["short", &"A".repeat(64)] {
        assert_eq!(
            f.json(guest_request(slug, bad, "GET")).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut foreign = guest_request(slug, &secret, "POST");
    foreign
        .headers_mut()
        .insert("origin", "https://foreign.test".parse().unwrap());
    assert_eq!(f.json(foreign).await.0, StatusCode::FORBIDDEN);
    let mut missing_origin = guest_request(slug, &secret, "POST");
    missing_origin.headers_mut().remove("origin");
    assert_eq!(f.json(missing_origin).await.0, StatusCode::FORBIDDEN);
    let mut extra = guest_request(slug, &secret, "POST");
    *extra.body_mut() = Body::from(json!({"called_by":f.owner}).to_string());
    assert_eq!(f.json(extra).await.0, StatusCode::BAD_REQUEST);
    let mut duplicated = guest_request(slug, &secret, "POST");
    duplicated
        .headers_mut()
        .append("x-queue-ticket", secret.parse().unwrap());
    assert_eq!(f.json(duplicated).await.0, StatusCode::BAD_REQUEST);
    let (status, created) = f.json(guest_request(slug, &secret, "POST")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["number"], 1);
    let mut fields: Vec<_> = created
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort();
    assert_eq!(
        fields,
        [
            "issued_at",
            "number",
            "people_ahead",
            "service_date",
            "service_name",
            "station_name",
            "status",
            "tips"
        ]
    );
    assert!(!created.to_string().contains(&secret));
    assert_eq!(f.json(guest_request(slug, &secret, "GET")).await.1, created);
    assert_eq!(
        f.json(guest_request(Uuid::new_v4(), &secret, "GET"))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.json(guest_request(slug, &token().0, "GET")).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, profile) = f
        .json(Request::get(&base).body(Body::empty()).unwrap())
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(profile.get("tenant_id").is_none());
    assert!(profile.get("called_by").is_none());
    let page = f
        .app
        .clone()
        .oneshot(
            Request::get(format!("/q/{slug}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()["cache-control"], "no-store");
    assert_eq!(page.headers()["referrer-policy"], "no-referrer");
    assert!(!page.headers().contains_key("access-control-allow-origin"));
    let html = String::from_utf8(
        to_bytes(page.into_body(), 64 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("alert(1)"));
    assert!(!html.contains("<script>alert(1)"));
    assert!(!html.contains(&secret));
    let staff_path = format!("/api/organizations/{}/queues/{}/tickets", f.tenant, f.queue);
    assert_eq!(
        f.json(
            Request::get(staff_path)
                .header("x-queue-ticket", &secret)
                .body(Body::empty())
                .unwrap()
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn protected_settings_http_and_qr_expose_only_the_public_queue_url() {
    let f = Fixture::new().await;
    let auth = f.login(f.owner).await;
    let path = f.settings_url();
    assert_eq!(
        f.json(Request::get(&path).body(Body::empty()).unwrap())
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let request = Request::put(&path)
        .header("cookie", &auth.0)
        .header("origin", ORIGIN)
        .header("x-csrf-token", &auth.1)
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"join_open":true,"expected_version":0}).to_string(),
        ))
        .unwrap();
    let (status, result) = f.json(request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["version"], 1);
    let profile_url = result["public_url"].as_str().unwrap();
    assert!(profile_url.starts_with(&format!("{ORIGIN}/q/")));
    assert!(!profile_url.contains(&f.tenant.to_string()));
    assert!(!profile_url.contains(&f.queue.to_string()));
    let qr_path = result["qr_url"].as_str().unwrap();
    let qr = f
        .app
        .clone()
        .oneshot(
            Request::get(qr_path)
                .header("cookie", &auth.0)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(qr.status(), StatusCode::OK);
    assert_eq!(qr.headers()["content-type"], "image/svg+xml; charset=utf-8");
    assert!(
        qr.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("img-src 'self'")
    );
    let svg =
        String::from_utf8(to_bytes(qr.into_body(), 64 * 1024).await.unwrap().to_vec()).unwrap();
    assert!(svg.contains("<svg"));
    assert!(svg.contains("<path"));
    let operator = f.member(Role::Operator).await;
    let operator_auth = f.login(operator).await;
    let denied = Request::put(&path)
        .header("cookie", &operator_auth.0)
        .header("origin", ORIGIN)
        .header("x-csrf-token", &operator_auth.1)
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"join_open":false,"expected_version":1}).to_string(),
        ))
        .unwrap();
    assert_eq!(f.json(denied).await.0, StatusCode::FORBIDDEN);
    let (_, document) = f
        .json(Request::get("/openapi.json").body(Body::empty()).unwrap())
        .await;
    assert!(
        document["paths"]["/api/public/queues/{slug}/ticket"]["get"]["security"][0]
            .get("queueTicket")
            .is_some()
    );
    assert_eq!(
        document["components"]["securitySchemes"]["queueTicket"]["name"],
        "X-Queue-Ticket"
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn display_publication_is_explicit_independent_and_manager_only() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    assert!(
        !public::settings(&f.runtime, f.owner, f.tenant, f.queue)
            .await
            .unwrap()
            .display_enabled
    );
    assert!(matches!(
        public::display(&f.runtime, slug, 0).await,
        Err(PublicQueueError::NotFound)
    ));
    for role in [Role::Operator, Role::Employee] {
        let user = f.member(role).await;
        assert!(matches!(
            public::configure_display(&f.runtime, user, f.tenant, f.queue, true, 1).await,
            Err(PublicQueueError::Core(_))
        ));
    }
    let (_, hash) = token();
    let ticket = public::issue(&f.runtime, slug, &hash).await.unwrap();
    let manager = f.member(Role::Manager).await;
    let enabled = public::configure_display(&f.runtime, manager, f.tenant, f.queue, true, 1)
        .await
        .unwrap();
    assert!(enabled.display_enabled && enabled.join_open);
    let paused = public::configure(&f.runtime, f.owner, f.tenant, f.queue, false, 2)
        .await
        .unwrap();
    assert!(paused.display_enabled && !paused.join_open);
    assert!(
        !public::display(&f.runtime, slug, 0)
            .await
            .unwrap()
            .join_open
    );
    let disabled = public::configure_display(&f.runtime, f.owner, f.tenant, f.queue, false, 3)
        .await
        .unwrap();
    assert!(!disabled.display_enabled && !disabled.join_open);
    assert_eq!(
        public::ticket(&f.runtime, slug, &hash).await.unwrap(),
        ticket
    );
    assert!(matches!(
        public::display(&f.runtime, slug, 0).await,
        Err(PublicQueueError::NotFound)
    ));
    let other = Fixture::new().await;
    assert!(matches!(
        public::configure_display(&f.runtime, f.owner, other.tenant, other.queue, true, 0).await,
        Err(PublicQueueError::Core(_))
    ));
    let first = public::configure_display(
        &other.runtime,
        other.owner,
        other.tenant,
        other.queue,
        true,
        0,
    )
    .await
    .unwrap();
    assert!(first.display_enabled && !first.join_open);
    let other_slug = first.public_slug.unwrap();
    assert_eq!(
        public::display(&other.runtime, other_slug, 0)
            .await
            .unwrap()
            .waiting_count,
        0
    );
    assert!(matches!(
        public::issue(&other.runtime, other_slug, &token().1).await,
        Err(PublicQueueError::Closed)
    ));
    other.close().await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn display_and_joining_changes_share_one_optimistic_version() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let (joining, display) = tokio::join!(
        public::configure(&f.runtime, f.owner, f.tenant, f.queue, false, 1),
        public::configure_display(&f.runtime, f.owner, f.tenant, f.queue, true, 1)
    );
    assert_ne!(joining.is_ok(), display.is_ok());
    let loser = match (joining, display) {
        (Ok(_), Err(error)) | (Err(error), Ok(_)) => error,
        _ => panic!("exactly one concurrent settings change must succeed"),
    };
    assert!(matches!(
        loser,
        PublicQueueError::Core(turnesta_api::core::CoreError::VersionConflict)
    ));
    let actual = public::settings(&f.runtime, f.owner, f.tenant, f.queue)
        .await
        .unwrap();
    assert_eq!(actual.version, 2);
    assert_eq!(actual.join_open, actual.display_enabled);
    assert_eq!(actual.public_slug, Some(slug));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn display_snapshot_distinguishes_dates_and_excludes_terminal_and_private_data() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    public::configure_display(&f.runtime, f.owner, f.tenant, f.queue, true, 1)
        .await
        .unwrap();
    let old = f
        .staff(Command::Issue { service_id: None })
        .await
        .ticket
        .unwrap();
    sqlx::query("UPDATE queue.tickets SET service_date=service_date-1 WHERE tenant_id=$1 AND queue_id=$2 AND id=$3")
        .bind(f.tenant).bind(f.queue).bind(old.id).execute(&f.migrator).await.unwrap();
    let new = f
        .staff(Command::Issue { service_id: None })
        .await
        .ticket
        .unwrap();
    sqlx::query("UPDATE queue.tickets SET number=1 WHERE tenant_id=$1 AND queue_id=$2 AND id=$3")
        .bind(f.tenant)
        .bind(f.queue)
        .bind(new.id)
        .execute(&f.migrator)
        .await
        .unwrap();
    f.staff(Command::CallNext { station_id: None }).await;
    let operator = f.member(Role::Operator).await;
    queue::execute(
        &f.runtime,
        operator,
        f.tenant,
        f.queue,
        Uuid::new_v4(),
        Command::CallNext { station_id: None },
    )
    .await
    .unwrap();
    f.staff(Command::Issue { service_id: None }).await;
    let snapshot = public::display(&f.runtime, slug, 0).await.unwrap();
    assert_eq!(snapshot.called_count, 2);
    assert_eq!(snapshot.waiting_count, 1);
    assert_eq!(snapshot.called[0].number, 1);
    assert_eq!(snapshot.called[1].number, 1);
    assert_ne!(
        snapshot.called[0].service_date,
        snapshot.called[1].service_date
    );
    let value = serde_json::to_value(&snapshot).unwrap();
    let keys: Vec<_> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        vec![
            "called",
            "called_count",
            "join_open",
            "location_name",
            "next_page",
            "observed_at",
            "organization_name",
            "page",
            "queue_name",
            "time_zone",
            "version",
            "waiting_count"
        ]
    );
    assert_eq!(value["called"][0].as_object().unwrap().len(), 3);
    let serialized = value.to_string();
    for private in [
        f.tenant.to_string(),
        f.queue.to_string(),
        f.owner.to_string(),
        operator.to_string(),
        old.id.to_string(),
        new.id.to_string(),
    ] {
        assert!(!serialized.contains(&private));
    }
    f.staff(Command::Complete { ticket_id: old.id }).await;
    f.staff(Command::Cancel { ticket_id: new.id }).await;
    let actual = public::display(&f.runtime, slug, 0).await.unwrap();
    assert!(actual.called.is_empty());
    assert_eq!(actual.waiting_count, 1);
    assert_ne!(actual.version, snapshot.version);
    let safe: bool = sqlx::query_scalar("SELECT prosecdef AND 'search_path=pg_catalog'=ANY(proconfig) AND NOT EXISTS (SELECT 1 FROM aclexplode(proacl) a WHERE grantee=0 AND privilege_type='EXECUTE') FROM pg_proc WHERE oid='queue.public_display(uuid,integer)'::regprocedure")
        .fetch_one(&f.runtime).await.unwrap();
    assert!(safe);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn display_pagination_does_not_silently_drop_active_calls() {
    let f = Fixture::new().await;
    let settings = public::configure_display(&f.runtime, f.owner, f.tenant, f.queue, true, 0)
        .await
        .unwrap();
    let slug = settings.public_slug.unwrap();
    sqlx::query("WITH users AS (INSERT INTO core.users (id,display_name) SELECT gen_random_uuid(),'Display operator' FROM generate_series(1,101) RETURNING id), members AS (INSERT INTO core.memberships (tenant_id,user_id,role) SELECT $1,id,'operator' FROM users RETURNING user_id), numbered AS (SELECT user_id,row_number() OVER (ORDER BY user_id) AS n FROM members) INSERT INTO queue.tickets (tenant_id,queue_id,id,position,service_date,number,status,called_by,issued_at,called_at) SELECT $1,$2,gen_random_uuid(),n,'2026-10-28',n::integer,'called',user_id,clock_timestamp(),clock_timestamp() FROM numbered")
        .bind(f.tenant).bind(f.queue).execute(&f.migrator).await.unwrap();
    let first = public::display(&f.runtime, slug, 0).await.unwrap();
    let second = public::display(&f.runtime, slug, 1).await.unwrap();
    assert_eq!(first.called_count, 101);
    assert_eq!(first.called.len(), 100);
    assert_eq!(first.next_page, Some(1));
    assert_eq!(second.called.len(), 1);
    assert_eq!(second.called[0].number, 101);
    assert_eq!(second.next_page, None);
    assert_eq!(first.version, second.version);
    assert!(
        public::display(&f.runtime, slug, 2)
            .await
            .unwrap()
            .called
            .is_empty()
    );
    let ticket: Uuid = sqlx::query_scalar(
        "SELECT id FROM queue.tickets WHERE tenant_id=$1 AND queue_id=$2 AND position=1",
    )
    .bind(f.tenant)
    .bind(f.queue)
    .fetch_one(&f.migrator)
    .await
    .unwrap();
    f.staff(Command::Complete { ticket_id: ticket }).await;
    let changed = public::display(&f.runtime, slug, 0).await.unwrap();
    assert_eq!(changed.called_count, 100);
    assert_eq!(changed.next_page, None);
    assert_ne!(changed.version, first.version);
    f.close().await;
}

fn display_configuration(f: &Fixture, auth: &(String, String), body: Value) -> Request<Body> {
    Request::put(format!("{}/display", f.settings_url()))
        .header("cookie", &auth.0)
        .header("origin", ORIGIN)
        .header("x-csrf-token", &auth.1)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn display_http_is_anonymous_strict_and_never_cached_even_when_disabled() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let url = format!("/api/public/queues/{slug}/display");
    let page = format!("/display/{slug}");
    for target in [&url, &page] {
        let response = f
            .app
            .clone()
            .oneshot(Request::get(target).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    public::configure_display(&f.runtime, f.owner, f.tenant, f.queue, true, 1)
        .await
        .unwrap();
    f.staff(Command::Issue { service_id: None }).await;
    f.staff(Command::CallNext { station_id: None }).await;
    let (status, body) = f
        .json(Request::get(&url).body(Body::empty()).unwrap())
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["called"][0]["number"], 1);
    assert_eq!(body["version"].as_str().unwrap(), "3");
    for suffix in [
        "?page=-1",
        "?page=no",
        "?page=21474836",
        "?page=1&page=2",
        "?actor=owner",
    ] {
        assert_eq!(
            f.json(
                Request::get(format!("{url}{suffix}"))
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let response = f
        .app
        .clone()
        .oneshot(Request::get(&page).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let html = String::from_utf8(
        to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("alert(1)"));
    assert!(!html.contains("<script>alert(1)"));
    assert!(!html.contains("№ 001"));
    for path in [
        "/assets/queue-display.css",
        "/assets/queue-display.js",
        "/assets/queue-display-client.js",
    ] {
        let response = f
            .app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    public::configure_display(&f.runtime, f.owner, f.tenant, f.queue, false, 2)
        .await
        .unwrap();
    assert_eq!(
        f.json(Request::get(&url).body(Body::empty()).unwrap())
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (_, document) = f
        .json(Request::get("/openapi.json").body(Body::empty()).unwrap())
        .await;
    assert!(
        document["paths"]["/api/public/queues/{slug}/display"]["get"]
            .get("security")
            .is_none()
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn display_http_configuration_checks_session_csrf_role_and_version() {
    let f = Fixture::new().await;
    let path = format!("{}/display", f.settings_url());
    assert_eq!(
        f.json(Request::put(&path).body(Body::empty()).unwrap())
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let owner = f.login(f.owner).await;
    for field in ["origin", "x-csrf-token"] {
        let mut request =
            display_configuration(&f, &owner, json!({"enabled":true,"expected_version":0}));
        request.headers_mut().remove(field);
        assert_eq!(f.json(request).await.0, StatusCode::FORBIDDEN);
    }
    assert_eq!(
        f.json(display_configuration(
            &f,
            &owner,
            json!({"enabled":true,"expected_version":0,"join_open":true})
        ))
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let operator = f.member(Role::Operator).await;
    let auth = f.login(operator).await;
    assert_eq!(
        f.json(display_configuration(
            &f,
            &auth,
            json!({"enabled":true,"expected_version":0})
        ))
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, settings) = f
        .json(display_configuration(
            &f,
            &owner,
            json!({"enabled":true,"expected_version":0}),
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(settings["display_enabled"], true);
    assert_eq!(settings["join_open"], false);
    assert!(
        settings["display_url"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{ORIGIN}/display/"))
    );
    assert_eq!(
        f.json(display_configuration(
            &f,
            &owner,
            json!({"enabled":false,"expected_version":0})
        ))
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (_, disabled) = f
        .json(display_configuration(
            &f,
            &owner,
            json!({"enabled":false,"expected_version":1}),
        ))
        .await;
    assert!(disabled["display_url"].is_null());
    assert_eq!(disabled["display_enabled"], false);
    f.close().await;
}

fn live_app(f: &Fixture) -> (Router, Arc<LiveHub>, tokio::task::JoinHandle<()>) {
    let hub = LiveHub::new(f.runtime.clone());
    let task = hub.start();
    let app = router(
        AppState::new(f.runtime.clone(), Duration::from_secs(1)).with_queue_live(hub.clone()),
    );
    (app, hub, task)
}
async fn stop_live(hub: Arc<LiveHub>, task: tokio::task::JoinHandle<()>) {
    hub.shutdown();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
}
async fn live_event(stream: &mut axum::body::BodyDataStream) -> String {
    tokio::time::timeout(Duration::from_secs(8), async {
        let mut text = String::new();
        while let Some(frame) = stream.next().await {
            text.push_str(std::str::from_utf8(&frame.unwrap()).unwrap());
            if text.contains("\n\n") {
                return text;
            }
        }
        panic!("SSE ended before an event")
    })
    .await
    .expect("SSE event deadline exceeded")
}
fn event_name(event: &str) -> &str {
    event
        .lines()
        .find_map(|line| line.strip_prefix("event:"))
        .unwrap()
        .trim()
}
fn event_id(event: &str) -> &str {
    event
        .lines()
        .find_map(|line| line.strip_prefix("id:"))
        .unwrap()
        .trim()
}
async fn until_event(stream: &mut axum::body::BodyDataStream, name: &str) -> String {
    tokio::time::timeout(Duration::from_secs(19), async {
        loop {
            let event = live_event(stream).await;
            if event_name(&event) == name {
                return event;
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn sse_http_scopes_strict_queries_and_initial_resync_never_expose_ticket_data() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let (app, hub, task) = live_app(&f);
    let url = format!("/api/public/queues/{slug}/events");
    for suffix in [
        "?audience=unknown",
        "?audience=guest&audience=display",
        "?actor=owner",
    ] {
        assert_eq!(
            app.clone()
                .oneshot(
                    Request::get(format!("{url}{suffix}"))
                        .body(Body::empty())
                        .unwrap()
                )
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        app.clone()
            .oneshot(
                Request::get(format!("{url}?audience=display"))
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.clone()
            .oneshot(
                Request::get(&url)
                    .header("origin", "https://other.test")
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let response = app
        .clone()
        .oneshot(
            Request::get(&url)
                .header("last-event-id", "old-revision")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["x-accel-buffering"], "no");
    assert!(
        !response
            .headers()
            .contains_key("access-control-allow-origin")
    );
    let mut stream = response.into_body().into_data_stream();
    let event = live_event(&mut stream).await;
    assert_eq!(event_name(&event), "refresh");
    assert_eq!(event_id(&event).len(), 64);
    for private in [
        f.tenant.to_string(),
        f.queue.to_string(),
        f.owner.to_string(),
        "old-revision".to_owned(),
    ] {
        assert!(!event.contains(&private));
    }
    assert_eq!(
        event
            .lines()
            .find_map(|line| line.strip_prefix("data:"))
            .unwrap()
            .trim(),
        "{}"
    );
    drop(stream);
    let auth = f.login(f.owner).await;
    let staff_url = format!("/api/organizations/{}/queues/{}/events", f.tenant, f.queue);
    assert_eq!(
        app.clone()
            .oneshot(Request::get(&staff_url).body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let employee = f.member(Role::Employee).await;
    let employee_auth = f.login(employee).await;
    let staff = app
        .clone()
        .oneshot(
            Request::get(&staff_url)
                .header("cookie", &employee_auth.0)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(staff.status(), StatusCode::OK);
    drop(staff);
    let other = Fixture::new().await;
    assert_eq!(
        app.clone()
            .oneshot(
                Request::get(format!(
                    "/api/organizations/{}/queues/{}/events",
                    other.tenant, other.queue
                ))
                .header("cookie", &auth.0)
                .body(Body::empty())
                .unwrap()
            )
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let safe:i64=sqlx::query_scalar("SELECT count(*) FROM pg_proc p WHERE pronamespace='queue'::regnamespace AND proname IN ('live_states','public_stream_target') AND prosecdef AND 'search_path=pg_catalog'=ANY(proconfig) AND NOT EXISTS (SELECT 1 FROM aclexplode(p.proacl) a WHERE a.grantee=0 AND a.privilege_type='EXECUTE')").fetch_one(&f.runtime).await.unwrap();
    assert_eq!(safe, 2);
    other.close().await;
    stop_live(hub, task).await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn sse_postcommit_notifications_cross_pools_and_reconciliation_use_durable_versions() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let (app, hub, task) = live_app(&f);
    let response = app
        .oneshot(
            Request::get(format!("/api/public/queues/{slug}/events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    let first = live_event(&mut stream).await;
    tokio::time::timeout(Duration::from_secs(4), async {
        while !hub.listener_connected() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let mut transaction = f.migrator.begin().await.unwrap();
    sqlx::query("UPDATE queue.queues SET version=version+1 WHERE tenant_id=$1 AND id=$2")
        .bind(f.tenant)
        .bind(f.queue)
        .execute(&mut *transaction)
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(200), async {
            loop {
                assert_ne!(event_name(&live_event(&mut stream).await), "refresh");
            }
        })
        .await
        .is_err()
    );
    transaction.rollback().await.unwrap();
    hub.reconcile().await;
    let unchanged = live_event(&mut stream).await;
    assert_eq!(event_id(&unchanged), event_id(&first));
    let separate = pool("TEST_DATABASE_URL", 1).await;
    public::issue(&separate, slug, &token().1).await.unwrap();
    let changed = tokio::time::timeout(Duration::from_secs(3), until_event(&mut stream, "refresh"))
        .await
        .unwrap();
    assert_ne!(event_id(&changed), event_id(&first));
    sqlx::query(
        "UPDATE core.locations SET name='Renamed point',version=version+1 WHERE tenant_id=$1",
    )
    .bind(f.tenant)
    .execute(&f.migrator)
    .await
    .unwrap();
    let metadata = until_event(&mut stream, "refresh").await;
    assert_ne!(event_id(&metadata), event_id(&changed));
    assert_eq!(
        public::profile(&f.runtime, slug)
            .await
            .unwrap()
            .location_name,
        "Renamed point"
    );
    drop(stream);
    hub.reconcile().await;
    assert_eq!(hub.queue_count(), 0);
    separate.close().await;
    stop_live(hub, task).await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn sse_display_revocation_is_independent_of_paused_guest_issuance() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    public::configure_display(&f.runtime, f.owner, f.tenant, f.queue, true, 1)
        .await
        .unwrap();
    let (app, hub, task) = live_app(&f);
    let url = format!("/api/public/queues/{slug}/events");
    let guest = app
        .clone()
        .oneshot(Request::get(&url).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let display = app
        .clone()
        .oneshot(
            Request::get(format!("{url}?audience=display"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut guest = guest.into_body().into_data_stream();
    let mut display = display.into_body().into_data_stream();
    live_event(&mut guest).await;
    live_event(&mut display).await;
    public::configure(&f.runtime, f.owner, f.tenant, f.queue, false, 2)
        .await
        .unwrap();
    assert_eq!(
        event_name(&until_event(&mut guest, "refresh").await),
        "refresh"
    );
    assert_eq!(
        event_name(&until_event(&mut display, "refresh").await),
        "refresh"
    );
    public::configure_display(&f.runtime, f.owner, f.tenant, f.queue, false, 3)
        .await
        .unwrap();
    assert_eq!(
        event_name(&until_event(&mut display, "denied").await),
        "denied"
    );
    assert!(display.next().await.is_none());
    assert_eq!(
        event_name(&until_event(&mut guest, "refresh").await),
        "refresh"
    );
    drop(guest);
    stop_live(hub, task).await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn sse_staff_revalidates_revoked_expired_sessions_and_removed_membership() {
    let f = Fixture::new().await;
    let (app, hub, task) = live_app(&f);
    let operator = f.member(Role::Operator).await;
    let employee = f.member(Role::Employee).await;
    let owner_auth = f.login(f.owner).await;
    let operator_auth = f.login(operator).await;
    let employee_auth = f.login(employee).await;
    let url = format!("/api/organizations/{}/queues/{}/events", f.tenant, f.queue);
    let mut streams = Vec::new();
    for auth in [&owner_auth, &operator_auth, &employee_auth] {
        let response = app
            .clone()
            .oneshot(
                Request::get(&url)
                    .header("cookie", &auth.0)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body().into_data_stream();
        live_event(&mut stream).await;
        streams.push(stream);
    }
    // Test-only fixtures in the dedicated database. No existing users are edited.
    sqlx::query("UPDATE auth.sessions SET revoked_at=clock_timestamp() WHERE user_id=$1")
        .bind(f.owner)
        .execute(&f.migrator)
        .await
        .unwrap();
    sqlx::query("DELETE FROM core.memberships WHERE tenant_id=$1 AND user_id=$2")
        .bind(f.tenant)
        .bind(operator)
        .execute(&f.migrator)
        .await
        .unwrap();
    sqlx::query("UPDATE auth.sessions SET expires_at=clock_timestamp() WHERE user_id=$1")
        .bind(employee)
        .execute(&f.migrator)
        .await
        .unwrap();
    let mut a = streams.remove(0);
    let mut b = streams.remove(0);
    let mut c = streams.remove(0);
    let (a, b, c) = tokio::join!(
        until_event(&mut a, "denied"),
        until_event(&mut b, "denied"),
        until_event(&mut c, "denied")
    );
    assert_eq!(
        (event_name(&a), event_name(&b), event_name(&c)),
        ("denied", "denied", "denied")
    );
    stop_live(hub, task).await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn sse_connection_budget_is_released_and_public_clients_cannot_take_staff_slots() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let (app, hub, task) = live_app(&f);
    let url = format!("/api/public/queues/{slug}/events");
    let mut responses = Vec::new();
    for _ in 0..96 {
        let response = app
            .clone()
            .oneshot(Request::get(&url).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        responses.push(response);
    }
    assert_eq!(hub.queue_count(), 1);
    assert_eq!(
        app.clone()
            .oneshot(Request::get(&url).body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    let auth = f.login(f.owner).await;
    let staff = app
        .clone()
        .oneshot(
            Request::get(format!(
                "/api/organizations/{}/queues/{}/events",
                f.tenant, f.queue
            ))
            .header("cookie", &auth.0)
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(staff.status(), StatusCode::OK);
    drop(responses.pop());
    let replacement = app
        .oneshot(Request::get(&url).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(replacement.status(), StatusCode::OK);
    drop(replacement);
    drop(staff);
    drop(responses);
    hub.reconcile().await;
    assert_eq!(hub.queue_count(), 0);
    stop_live(hub, task).await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database"]
async fn sse_database_failure_and_shutdown_end_streams_without_holding_pool_connections() {
    let f = Fixture::new().await;
    let slug = f.publish().await;
    let (app, hub, task) = live_app(&f);
    let response = app
        .oneshot(
            Request::get(format!("/api/public/queues/{slug}/events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    live_event(&mut stream).await;
    tokio::time::timeout(Duration::from_secs(2), f.runtime.close())
        .await
        .expect("SSE held a primary pool connection");
    hub.reconcile().await;
    assert_eq!(event_name(&live_event(&mut stream).await), "unavailable");
    assert!(stream.next().await.is_none());
    stop_live(hub, task).await;
    f.close().await;
}
