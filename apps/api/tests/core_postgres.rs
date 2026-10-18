use std::time::Duration;

use sqlx::{PgPool, postgres::PgPoolOptions};
use turnesta_api::{
    core::{
        CoreError, Name, ProvisionedOrganization, Provisioner, Role, TenantId, TenantSession,
        UserId,
    },
    database,
};

struct Fixture {
    runtime: PgPool,
    admin: PgPool,
}

impl Fixture {
    async fn new() -> Self {
        let runtime = connect("TEST_DATABASE_URL", 1).await;
        let admin = connect("TEST_ADMIN_DATABASE_URL", 2).await;
        Self { runtime, admin }
    }

    async fn organization(&self) -> ProvisionedOrganization {
        Provisioner::new(self.admin.clone())
            .create_organization(&name("Core integration test"), &name("Test owner"))
            .await
            .unwrap()
    }

    async fn close(self) {
        self.runtime.close().await;
        self.admin.close().await;
    }
}

async fn connect(variable: &str, max_connections: u32) -> PgPool {
    let url = std::env::var(variable).unwrap_or_else(|_| panic!("{variable} is required"));
    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(3))
        .connect(&url)
        .await
        .expect("test PostgreSQL connection");
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        database.ends_with("_test"),
        "Core integration tests require a dedicated *_test database"
    );
    pool
}

fn name(value: &str) -> Name {
    Name::parse(value).unwrap()
}

async fn context(
    pool: &PgPool,
    tenant: TenantId,
    user: UserId,
) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(
        "SELECT set_config('app.tenant_id', $1, true), set_config('app.user_id', $2, true)",
    )
    .bind(tenant.to_string())
    .bind(user.to_string())
    .execute(&mut *tx)
    .await
    .unwrap();
    tx
}

fn sqlstate(error: &sqlx::Error) -> String {
    error
        .as_database_error()
        .unwrap()
        .code()
        .unwrap()
        .into_owned()
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin URLs"]
async fn provisioning_creates_owner_and_rejects_wrong_database_capability() {
    let fixture = Fixture::new().await;
    let org = fixture.organization().await;
    let mut session = TenantSession::begin(&fixture.runtime, org.owner.id, org.organization.id)
        .await
        .unwrap();
    assert_eq!(
        session.organization().await.unwrap().id,
        org.organization.id
    );
    session.commit().await.unwrap();

    let denied = Provisioner::new(fixture.runtime.clone())
        .create_user(&name("Denied"))
        .await;
    assert!(matches!(denied, Err(CoreError::ProvisioningRoleRequired)));
    let denied = TenantSession::begin(&fixture.admin, org.owner.id, org.organization.id).await;
    assert!(matches!(denied, Err(CoreError::RuntimeRoleRequired)));

    let duplicate = Provisioner::new(fixture.admin.clone())
        .add_membership(org.organization.id, org.owner.id, Role::Manager)
        .await;
    assert!(matches!(duplicate, Err(CoreError::MembershipExists)));
    let missing = Provisioner::new(fixture.admin.clone())
        .add_membership(TenantId::new(), org.owner.id, Role::Employee)
        .await;
    assert!(matches!(missing, Err(CoreError::ReferenceNotFound)));
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin URLs"]
async fn rls_isolates_organizations_even_without_query_tenant_filter() {
    let fixture = Fixture::new().await;
    let first = fixture.organization().await;
    let second = fixture.organization().await;
    let mut session = TenantSession::begin(&fixture.runtime, first.owner.id, first.organization.id)
        .await
        .unwrap();
    let location = session
        .create_location(&name("Первый филиал"), "Europe/Moscow")
        .await
        .unwrap();
    session.commit().await.unwrap();

    let denied =
        TenantSession::begin(&fixture.runtime, first.owner.id, second.organization.id).await;
    assert!(matches!(denied, Err(CoreError::AccessDenied)));
    let mut tx = context(&fixture.runtime, first.organization.id, first.owner.id).await;
    let ids: Vec<TenantId> = sqlx::query_scalar("SELECT id FROM core.organizations")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(ids, vec![first.organization.id]);
    let locations: Vec<turnesta_api::core::LocationId> =
        sqlx::query_scalar("SELECT id FROM core.locations")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert_eq!(locations, vec![location.id]);
    let error = sqlx::query("INSERT INTO core.locations (tenant_id, id, name, time_zone) VALUES ($1, gen_random_uuid(), 'Wrong tenant', 'UTC')")
        .bind(second.organization.id).execute(&mut *tx).await.unwrap_err();
    assert_eq!(sqlstate(&error), "42501");
    tx.rollback().await.unwrap();

    // A valid member of two organizations still sees only the selected one.
    Provisioner::new(fixture.admin.clone())
        .add_membership(second.organization.id, first.owner.id, Role::Employee)
        .await
        .unwrap();
    let mut session =
        TenantSession::begin(&fixture.runtime, first.owner.id, second.organization.id)
            .await
            .unwrap();
    assert!(session.locations().await.unwrap().is_empty());
    assert_eq!(
        session.organization().await.unwrap().id,
        second.organization.id
    );
    session.commit().await.unwrap();
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin URLs"]
async fn locations_enforce_business_roles_in_service_and_database() {
    let fixture = Fixture::new().await;
    let org = fixture.organization().await;
    let provisioner = Provisioner::new(fixture.admin.clone());
    for role in [Role::Owner, Role::Manager, Role::Operator, Role::Employee] {
        let user = provisioner
            .create_user(&name("Role test user"))
            .await
            .unwrap();
        provisioner
            .add_membership(org.organization.id, user.id, role)
            .await
            .unwrap();
        let mut session = TenantSession::begin(&fixture.runtime, user.id, org.organization.id)
            .await
            .unwrap();
        let result = session
            .create_location(&name("Role test location"), "America/New_York")
            .await;
        if role.can_manage_locations() {
            assert_eq!(result.unwrap().time_zone, "America/New_York");
        } else {
            assert!(matches!(result, Err(CoreError::Forbidden)));
        }
        session.commit().await.unwrap();
        if !role.can_manage_locations() {
            let mut tx = context(&fixture.runtime, org.organization.id, user.id).await;
            let error = sqlx::query("INSERT INTO core.locations (tenant_id, id, name, time_zone) VALUES ($1, gen_random_uuid(), 'Denied', 'UTC')")
                .bind(org.organization.id).execute(&mut *tx).await.unwrap_err();
            assert_eq!(sqlstate(&error), "42501");
            tx.rollback().await.unwrap();
        }
    }
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin URLs"]
async fn transaction_context_and_writes_do_not_leak_after_commit_drop_or_cancellation() {
    let fixture = Fixture::new().await;
    let org = fixture.organization().await;
    let mut session = TenantSession::begin(&fixture.runtime, org.owner.id, org.organization.id)
        .await
        .unwrap();
    session
        .create_location(&name("Committed"), "UTC")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_no_context(&fixture.runtime).await;

    let mut session = TenantSession::begin(&fixture.runtime, org.owner.id, org.organization.id)
        .await
        .unwrap();
    session
        .create_location(&name("Rolled back"), "UTC")
        .await
        .unwrap();
    drop(session);
    assert_no_context(&fixture.runtime).await;

    let pool = fixture.runtime.clone();
    let (started, ready) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut session = TenantSession::begin(&pool, org.owner.id, org.organization.id)
            .await
            .unwrap();
        session
            .create_location(&name("Cancelled"), "UTC")
            .await
            .unwrap();
        started.send(()).unwrap();
        std::future::pending::<()>().await;
        drop(session);
    });
    ready.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_no_context(&fixture.runtime).await;
    let mut session = TenantSession::begin(&fixture.runtime, org.owner.id, org.organization.id)
        .await
        .unwrap();
    let locations = session.locations().await.unwrap();
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].name, "Committed");
    session.commit().await.unwrap();
    fixture.close().await;
}

async fn assert_no_context(pool: &PgPool) {
    let values: (Option<String>, Option<String>, i64) = sqlx::query_as(
        "SELECT nullif(current_setting('app.tenant_id', true), ''), \
         nullif(current_setting('app.user_id', true), ''), (SELECT count(*) FROM core.locations)",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(values, (None, None, 0));
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin URLs"]
async fn runtime_has_no_ddl_provisioning_or_privilege_escalation_access() {
    let fixture = Fixture::new().await;
    assert!(database::is_runtime_role(&fixture.runtime).await.unwrap());
    for statement in [
        "CREATE TABLE core.forbidden (id int)",
        "CREATE TABLE public.forbidden (id int)",
        "CREATE SCHEMA forbidden",
        "CREATE TEMP TABLE forbidden (id int)",
        "SET ROLE turnesta_provisioner",
        "INSERT INTO core.organizations (id, name) VALUES (gen_random_uuid(), 'Denied')",
        "INSERT INTO core.memberships (tenant_id, user_id, role) VALUES (gen_random_uuid(), gen_random_uuid(), 'owner')",
    ] {
        let mut tx = fixture.runtime.begin().await.unwrap();
        let error = sqlx::query(statement).execute(&mut *tx).await.unwrap_err();
        assert_eq!(
            sqlstate(&error),
            "42501",
            "unexpected error for {statement}"
        );
        tx.rollback().await.unwrap();
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM core.organizations")
        .fetch_one(&fixture.runtime)
        .await
        .unwrap();
    assert_eq!(count, 0);
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin URLs"]
async fn timezone_validation_and_reference_constraint_reject_unknown_zones() {
    let fixture = Fixture::new().await;
    let org = fixture.organization().await;
    let mut session = TenantSession::begin(&fixture.runtime, org.owner.id, org.organization.id)
        .await
        .unwrap();
    let invalid = session
        .create_location(&name("Bad zone"), "Mars/Olympus")
        .await;
    assert!(matches!(invalid, Err(CoreError::InvalidTimeZone)));
    assert!(session.locations().await.unwrap().is_empty());
    session.commit().await.unwrap();
    let mut tx = context(&fixture.runtime, org.organization.id, org.owner.id).await;
    let error = sqlx::query("INSERT INTO core.locations (tenant_id, id, name, time_zone) VALUES ($1, gen_random_uuid(), 'Bad zone', 'Mars/Olympus')")
        .bind(org.organization.id).execute(&mut *tx).await.unwrap_err();
    assert_eq!(sqlstate(&error), "23503");
    tx.rollback().await.unwrap();
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires migrated dedicated PostgreSQL *_test database and runtime/admin URLs"]
async fn update_policies_protect_roles_and_immutable_tenant_keys() {
    let fixture = Fixture::new().await;
    let org = fixture.organization().await;
    let provisioner = Provisioner::new(fixture.admin.clone());
    let operator = provisioner
        .create_user(&name("Readonly update test"))
        .await
        .unwrap();
    provisioner
        .add_membership(org.organization.id, operator.id, Role::Operator)
        .await
        .unwrap();
    let mut session = TenantSession::begin(&fixture.runtime, org.owner.id, org.organization.id)
        .await
        .unwrap();
    let location = session
        .create_location(&name("Protected"), "UTC")
        .await
        .unwrap();
    session.commit().await.unwrap();
    let mut tx = context(&fixture.runtime, org.organization.id, operator.id).await;
    let affected =
        sqlx::query("UPDATE core.locations SET name = 'Forbidden' WHERE tenant_id=$1 AND id=$2")
            .bind(org.organization.id)
            .bind(location.id)
            .execute(&mut *tx)
            .await
            .unwrap();
    assert_eq!(affected.rows_affected(), 0);
    tx.rollback().await.unwrap();
    let mut tx = context(&fixture.runtime, org.organization.id, org.owner.id).await;
    let error = sqlx::query("UPDATE core.locations SET tenant_id=$1 WHERE id=$2")
        .bind(TenantId::new())
        .bind(location.id)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert_eq!(sqlstate(&error), "42501");
    tx.rollback().await.unwrap();
    fixture.close().await;
}
