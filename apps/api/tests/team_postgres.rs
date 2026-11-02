use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::time::Duration;
use tower::ServiceExt;
use turnesta_api::{
    auth::login::{self, Email},
    core::{Name, Provisioner, Role, TenantId, TenantSession, UserId},
    http::{AppState, router},
    team::{self, TeamCommand, TeamError},
};
use uuid::Uuid;
const ORIGIN: &str = "http://127.0.0.1:8080";
struct Fixture {
    runtime: PgPool,
    admin: PgPool,
    migrator: PgPool,
    tenant: TenantId,
    owner: UserId,
    app: Router,
}
async fn pool(key: &str) -> PgPool {
    let p = PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(3))
        .connect(&std::env::var(key).unwrap())
        .await
        .unwrap();
    let db: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&p)
        .await
        .unwrap();
    assert!(db.ends_with("_test"));
    p
}
impl Fixture {
    async fn new() -> Self {
        let runtime = pool("TEST_DATABASE_URL").await;
        let admin = pool("TEST_ADMIN_DATABASE_URL").await;
        let migrator = pool("TEST_MIGRATION_DATABASE_URL").await;
        let org = Provisioner::new(admin.clone())
            .create_organization(
                &Name::parse("Team test").unwrap(),
                &Name::parse("Owner").unwrap(),
            )
            .await
            .unwrap();
        let app = router(AppState::new(runtime.clone(), Duration::from_secs(1)));
        Self {
            runtime,
            admin,
            migrator,
            tenant: org.organization.id,
            owner: org.owner.id,
            app,
        }
    }
    async fn execute(&self, key: Uuid, c: TeamCommand) -> Result<team::TeamReceipt, TeamError> {
        team::execute(&self.runtime, self.owner, self.tenant, key, c, ORIGIN).await
    }
    async fn invite(&self) -> team::TeamReceipt {
        self.execute(Uuid::new_v4(), invite()).await.unwrap()
    }
    async fn cookie(&self, user: UserId) -> (String, String) {
        let mut token = [0; 32];
        let mut csrf = [0; 32];
        getrandom::fill(&mut token).unwrap();
        getrandom::fill(&mut csrf).unwrap();
        sqlx::query("SELECT auth.create_session($1,$2,$3,3600)")
            .bind(user)
            .bind(Sha256::digest(token).as_slice())
            .bind(Sha256::digest(csrf).as_slice())
            .execute(&self.runtime)
            .await
            .unwrap();
        (
            format!(
                "turnesta_session={}; turnesta_csrf={}",
                hex::encode(token),
                hex::encode(csrf)
            ),
            hex::encode(csrf),
        )
    }
    async fn http(
        &self,
        method: &str,
        path: &str,
        cookie: &str,
        csrf: Option<&str>,
        value: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut r = Request::builder()
            .method(method)
            .uri(path)
            .header("cookie", cookie)
            .header("origin", ORIGIN)
            .header("idempotency-key", Uuid::new_v4().to_string());
        if let Some(csrf) = csrf {
            r = r.header("x-csrf-token", csrf);
        }
        let body = if let Some(v) = value {
            r = r.header("content-type", "application/json");
            Body::from(v.to_string())
        } else {
            Body::empty()
        };
        let response = self
            .app
            .clone()
            .oneshot(r.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        (
            status,
            serde_json::from_slice(&to_bytes(response.into_body(), 256 * 1024).await.unwrap())
                .unwrap(),
        )
    }
    async fn close(self) {
        self.runtime.close().await;
        self.admin.close().await;
        self.migrator.close().await;
    }
}
fn invite() -> TeamCommand {
    TeamCommand::Invite {
        display_name: "Colleague".into(),
        email: format!("team-{}@example.test", Uuid::new_v4()),
        role: Role::Operator,
    }
}
fn change(user: UserId, role: Role, active: bool, version: i32) -> TeamCommand {
    TeamCommand::SetMember {
        user_id: user,
        role,
        active,
        expected_version: version,
    }
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn team_invitation_replay_encryption_and_scoped_membership_are_atomic() {
    let f = Fixture::new().await;
    let key = Uuid::new_v4();
    let command = invite();
    let first = f.execute(key, command.clone()).await.unwrap();
    assert_eq!(first.delivery.as_ref().unwrap().state, "queued");
    let member = team::members(&f.runtime, f.owner, f.tenant, None, 50)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.user_id == first.user_id)
        .unwrap();
    assert_eq!(member.role, Role::Operator);
    assert_eq!(member.delivery_state.as_deref(), Some("pending"));
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM core.team_commands WHERE tenant_id=$1")
            .bind(f.tenant)
            .fetch_one(&f.migrator)
            .await
            .unwrap();
    assert_eq!(rows, 1);
    let (nonce,cipher,hash):(Vec<u8>,Vec<u8>,Vec<u8>)=sqlx::query_as("SELECT j.nonce,j.ciphertext,j.code_hash FROM core.team_invitations t JOIN auth.invitation_deliveries j ON j.id=t.job_id WHERE t.tenant_id=$1 AND t.user_id=$2")
        .bind(f.tenant).bind(first.user_id).fetch_one(&f.migrator).await.unwrap();
    assert_eq!(nonce.len(), 24);
    assert_eq!(hash.len(), 32);
    assert!(!cipher.windows(9).any(|w| w == b"Colleague"));
    assert!(!serde_json::to_string(&first).unwrap().contains("code"));
    f.execute(
        Uuid::new_v4(),
        change(first.user_id, Role::Employee, false, 1),
    )
    .await
    .unwrap();
    let replay = f.execute(key, command.clone()).await.unwrap();
    assert_eq!(
        serde_json::to_value(first).unwrap(),
        serde_json::to_value(replay).unwrap()
    );
    assert!(matches!(
        f.execute(key, invite()).await,
        Err(TeamError::Idempotency)
    ));
    assert!(matches!(
        f.execute(Uuid::new_v4(), command).await,
        Err(TeamError::Exists)
    ));
    let disabled = team::members(&f.runtime, f.owner, f.tenant, None, 50)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.user_id == member.user_id)
        .unwrap();
    assert!(!disabled.active);
    assert_eq!(disabled.version, 2);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn team_reuses_global_identity_without_renaming_or_canceling_another_login() {
    let a = Fixture::new().await;
    let b = Fixture::new().await;
    let command = invite();
    let first = a.execute(Uuid::new_v4(), command.clone()).await.unwrap();
    let mut other = command.clone();
    if let TeamCommand::Invite {
        display_name, role, ..
    } = &mut other
    {
        *display_name = "Do not rename".into();
        *role = Role::Manager;
    }
    let second = b.execute(Uuid::new_v4(), other).await.unwrap();
    assert_eq!(second.user_id, first.user_id);
    assert_eq!(second.delivery.unwrap().state, "deferred");
    let member = team::members(&b.runtime, b.owner, b.tenant, None, 50)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.user_id == first.user_id)
        .unwrap();
    assert_eq!(member.display_name, "Colleague");
    assert_eq!(member.delivery_state.as_deref(), Some("deferred"));
    assert!(member.resend_after.is_some());
    let active_codes:i64=sqlx::query_scalar("SELECT count(*) FROM auth.login_codes c JOIN auth.identities i ON i.id=c.identity_id WHERE i.user_id=$1 AND c.used_at IS NULL").bind(first.user_id).fetch_one(&a.migrator).await.unwrap();
    assert_eq!(active_codes, 1);
    a.execute(
        Uuid::new_v4(),
        change(first.user_id, Role::Operator, false, 1),
    )
    .await
    .unwrap();
    assert!(
        TenantSession::begin(&a.runtime, first.user_id, a.tenant)
            .await
            .is_err()
    );
    let mut s = TenantSession::begin(&b.runtime, first.user_id, b.tenant)
        .await
        .unwrap();
    assert_eq!(s.role(), Role::Manager);
    assert_eq!(s.organization().await.unwrap().id, b.tenant);
    s.commit().await.unwrap();
    let visible: Vec<(TenantId,)> =
        sqlx::query_as("SELECT id FROM core.list_user_organizations($1,NULL,100)")
            .bind(first.user_id)
            .fetch_all(&a.runtime)
            .await
            .unwrap();
    assert_eq!(visible, vec![(b.tenant,)]);
    a.close().await;
    b.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn team_resend_cooldown_versions_and_stale_updates_do_not_enqueue_twice() {
    let f = Fixture::new().await;
    let member = f.invite().await;
    let send = TeamCommand::SendLogin {
        user_id: member.user_id,
        expected_version: 1,
    };
    assert!(matches!(
        f.execute(Uuid::new_v4(), send.clone()).await,
        Err(TeamError::RateLimited)
    ));
    sqlx::query("UPDATE core.team_invitations SET sent_at=clock_timestamp()-interval '61 seconds' WHERE tenant_id=$1").bind(f.tenant).execute(&f.migrator).await.unwrap();
    let key = Uuid::new_v4();
    let result = f.execute(key, send.clone()).await.unwrap();
    assert_eq!(result.version, 2);
    f.execute(key, send).await.unwrap();
    let jobs:i64=sqlx::query_scalar("SELECT count(*) FROM auth.invitation_deliveries j JOIN auth.identities i ON i.id=j.identity_id WHERE i.user_id=$1").bind(member.user_id).fetch_one(&f.migrator).await.unwrap();
    assert_eq!(jobs, 2);
    assert!(matches!(
        f.execute(
            Uuid::new_v4(),
            change(member.user_id, Role::Manager, true, 1)
        )
        .await,
        Err(TeamError::Version)
    ));
    let latest = team::members(&f.runtime, f.owner, f.tenant, None, 50)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.user_id == member.user_id)
        .unwrap();
    assert_eq!(latest.role, Role::Operator);
    assert_eq!(latest.version, 2);
    f.execute(
        Uuid::new_v4(),
        change(member.user_id, Role::Operator, false, 2),
    )
    .await
    .unwrap();
    assert!(matches!(
        f.execute(
            Uuid::new_v4(),
            TeamCommand::SendLogin {
                user_id: member.user_id,
                expected_version: 3
            }
        )
        .await,
        Err(TeamError::NotFound)
    ));
    f.execute(
        Uuid::new_v4(),
        change(member.user_id, Role::Employee, true, 3),
    )
    .await
    .unwrap();
    assert_eq!(
        TenantSession::begin(&f.runtime, member.user_id, f.tenant)
            .await
            .unwrap()
            .role(),
        Role::Employee
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn team_concurrent_invite_and_membership_versions_have_one_winner() {
    let f = Fixture::new().await;
    let key = Uuid::new_v4();
    let c = invite();
    let (a, b) = tokio::join!(f.execute(key, c.clone()), f.execute(key, c.clone()));
    let a = a.unwrap();
    assert_eq!(a.user_id, b.unwrap().user_id);
    let (a, b) = tokio::join!(
        f.execute(Uuid::new_v4(), change(a.user_id, Role::Manager, true, 1)),
        f.execute(Uuid::new_v4(), change(a.user_id, Role::Employee, true, 1))
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(a, Err(TeamError::Version)) || matches!(b, Err(TeamError::Version)));
    let c = invite();
    let (a, b) = tokio::join!(
        f.execute(Uuid::new_v4(), c.clone()),
        f.execute(Uuid::new_v4(), c)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn team_http_checks_roles_csrf_strict_shapes_and_revokes_existing_session() {
    let f = Fixture::new().await;
    let member = f.invite().await;
    let (owner, csrf) = f.cookie(f.owner).await;
    let (staff, _) = f.cookie(member.user_id).await;
    let path = format!("/api/organizations/{}/team", f.tenant);
    let endpoint = format!("{path}/commands");
    assert_eq!(
        f.http("GET", &path, &staff, None, None).await.0,
        StatusCode::FORBIDDEN
    );
    let wrong = format!("/api/organizations/{}/team", Uuid::new_v4());
    assert_eq!(
        f.http("GET", &wrong, &owner, None, None).await.0,
        StatusCode::NOT_FOUND
    );
    let command = json!({"type":"set_member","user_id":member.user_id,"role":"employee","active":false,"expected_version":1});
    assert_eq!(
        f.http("POST", &endpoint, &owner, None, Some(command.clone()))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let mut extra = command.clone();
    extra["actor_id"] = json!(f.owner);
    assert_eq!(
        f.http("POST", &endpoint, &owner, Some(&csrf), Some(extra))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.http("POST", &endpoint, &owner, Some(&csrf), Some(command))
            .await
            .0,
        StatusCode::OK
    );
    let org = format!("/api/organizations/{}", f.tenant);
    assert_eq!(
        f.http("GET", &org, &staff, None, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.http("GET", "/auth/session", &staff, None, None).await.0,
        StatusCode::OK
    );
    let (_, orgs) = f
        .http("GET", "/api/organizations", &staff, None, None)
        .await;
    assert_eq!(orgs["items"].as_array().unwrap().len(), 0);
    assert!(matches!(
        f.execute(Uuid::new_v4(), change(f.owner, Role::Operator, false, 1))
            .await,
        Err(TeamError::OwnerProtected)
    ));
    let mut s = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    let allowed:bool=sqlx::query_scalar("SELECT has_table_privilege(current_user,'core.memberships','UPDATE') OR has_table_privilege(current_user,'auth.identities','SELECT') OR has_table_privilege(current_user,'core.team_commands','SELECT')")
        .fetch_one(s.connection()).await.unwrap();
    assert!(!allowed);
    s.commit().await.unwrap();
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn team_does_not_invalidate_cli_code_and_pagination_has_no_duplicates() {
    let f = Fixture::new().await;
    let p = Provisioner::new(f.admin.clone());
    let u = p
        .create_user(&Name::parse("Existing person").unwrap())
        .await
        .unwrap();
    let email = Email::parse(&format!("cli-{}@example.test", Uuid::new_v4())).unwrap();
    login::bind_email(&f.admin, u.id, &email).await.unwrap();
    let mut code_hash = [0u8; 32];
    getrandom::fill(&mut code_hash).unwrap();
    login::issue_login_code(&f.admin, &email, &code_hash, 600)
        .await
        .unwrap();
    let r = f
        .execute(
            Uuid::new_v4(),
            TeamCommand::Invite {
                display_name: "Unchanged".into(),
                email: email.as_str().into(),
                role: Role::Employee,
            },
        )
        .await
        .unwrap();
    assert_eq!(r.user_id, u.id);
    assert_eq!(r.delivery.unwrap().state, "deferred");
    let used: bool =
        sqlx::query_scalar("SELECT used_at IS NOT NULL FROM auth.login_codes WHERE token_hash=$1")
            .bind(code_hash.as_slice())
            .fetch_one(&f.migrator)
            .await
            .unwrap();
    assert!(!used);
    let first = team::members(&f.runtime, f.owner, f.tenant, None, 1)
        .await
        .unwrap();
    assert_eq!(first.len(), 2);
    let next = team::members(&f.runtime, f.owner, f.tenant, Some(first[0].user_id), 1)
        .await
        .unwrap();
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].user_id, first[1].user_id);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn team_revoke_hides_rls_and_preserves_queue_history_and_definer_checks() {
    use turnesta_api::queue::{self, Command};
    let f = Fixture::new().await;
    let member = f.invite().await;
    f.execute(
        Uuid::new_v4(),
        change(member.user_id, Role::Manager, true, 1),
    )
    .await
    .unwrap();
    let mut s = TenantSession::begin(&f.runtime, f.owner, f.tenant)
        .await
        .unwrap();
    let point = s
        .create_location(&Name::parse("Main").unwrap(), "Europe/Moscow")
        .await
        .unwrap();
    s.commit().await.unwrap();
    let q = queue::create(
        &f.runtime,
        f.owner,
        f.tenant,
        point.id,
        &Name::parse("Service").unwrap(),
    )
    .await
    .unwrap();
    queue::execute(
        &f.runtime,
        member.user_id,
        f.tenant,
        q.id,
        Uuid::new_v4(),
        Command::Issue { service_id: None },
    )
    .await
    .unwrap();
    let called = queue::execute(
        &f.runtime,
        member.user_id,
        f.tenant,
        q.id,
        Uuid::new_v4(),
        Command::CallNext { station_id: None },
    )
    .await
    .unwrap();
    f.execute(
        Uuid::new_v4(),
        change(member.user_id, Role::Manager, false, 2),
    )
    .await
    .unwrap();
    assert!(
        queue::get(&f.runtime, member.user_id, f.tenant, q.id)
            .await
            .is_err()
    );
    let ticket = queue::get(&f.runtime, f.owner, f.tenant, q.id)
        .await
        .unwrap();
    assert_eq!(ticket.id, q.id);
    let old_actor: UserId =
        sqlx::query_scalar("SELECT called_by FROM queue.tickets WHERE tenant_id=$1 AND id=$2")
            .bind(f.tenant)
            .bind(called.ticket.unwrap().id)
            .fetch_one(&f.migrator)
            .await
            .unwrap();
    assert_eq!(old_actor, member.user_id);
    let mut tx = f.runtime.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.user_id',$1,true),set_config('app.tenant_id',$2,true)")
        .bind(member.user_id.to_string())
        .bind(f.tenant.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM core.locations")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(rows, 0);
    let error = sqlx::query("SELECT tips.reconcile_attempt($1,$2)")
        .bind(f.tenant)
        .bind(Uuid::new_v4())
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("T1003")
    );
    tx.rollback().await.unwrap();
    f.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated *_test database"]
async fn team_hourly_limit_rolls_back_user_identity_membership_and_command() {
    let f = Fixture::new().await;
    sqlx::query("INSERT INTO core.team_commands(tenant_id,actor_id,key,command,result) SELECT $1,$2,gen_random_uuid(),'{\"type\":\"send_login\"}'::jsonb,'{}'::jsonb FROM generate_series(1,20)")
        .bind(f.tenant).bind(f.owner).execute(&f.migrator).await.unwrap();
    let command = invite();
    let email = if let TeamCommand::Invite { email, .. } = &command {
        email.clone()
    } else {
        unreachable!()
    };
    assert!(matches!(
        f.execute(Uuid::new_v4(), command).await,
        Err(TeamError::RateLimited)
    ));
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM auth.identities WHERE subject=$1)")
            .bind(email)
            .fetch_one(&f.migrator)
            .await
            .unwrap();
    assert!(!exists);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM core.memberships WHERE tenant_id=$1")
        .bind(f.tenant)
        .fetch_one(&f.migrator)
        .await
        .unwrap();
    assert_eq!(count, 1);
    f.close().await;
}
