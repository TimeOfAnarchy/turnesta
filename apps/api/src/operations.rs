//! Trusted, read-only operational inspection. Never mounted in the HTTP router.
use crate::{
    core::{TenantId, TenantSession, UserId},
    database,
};
use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use futures_util::TryStreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::time::Duration;

#[derive(Subcommand)]
pub enum OperationsCommand {
    /// Inspect migrations, ownership and delivery queues without changing data.
    Inspect {
        /// Hash all table contents. Intended for a stopped, synthetic rehearsal.
        #[arg(long)]
        fingerprints: bool,
        /// Verify the actual runtime login using TURNESTA_VERIFY_RUNTIME_DATABASE_URL.
        #[arg(long)]
        verify_runtime: bool,
        /// Authenticate pending encrypted invitations without sending or leasing them.
        #[arg(long)]
        check_delivery_key: bool,
        #[arg(long,default_value_t=300,value_parser=clap::value_parser!(u32).range(1..=86400))]
        max_due_age_seconds: u32,
    },
}
#[derive(Serialize)]
struct Migration {
    version: i64,
    checksum: String,
}
#[derive(Serialize)]
struct Table {
    schema: String,
    name: String,
    rows: i64,
    fingerprint: Option<String>,
}
#[derive(Serialize)]
struct QueueState {
    queue: &'static str,
    state: String,
    count: i64,
}
#[derive(Serialize)]
struct DeliveryHealth {
    states: Vec<QueueState>,
    overdue: i64,
    expired_pending: i64,
    max_due_age_seconds: u32,
}
#[derive(Serialize)]
pub struct Inspection {
    format_version: u32,
    server_version_num: i32,
    database: String,
    migrations: Vec<Migration>,
    tables: Vec<Table>,
    ownership_valid: bool,
    runtime_verified: bool,
    tenant_scope_exercised: bool,
    encrypted_invitations_verified: Option<usize>,
    delivery: DeliveryHealth,
}

pub async fn run(command: OperationsCommand, pool: &PgPool) -> Result<()> {
    let OperationsCommand::Inspect {
        fingerprints,
        verify_runtime,
        check_delivery_key,
        max_due_age_seconds,
    } = command;
    let inspection = inspect(
        pool,
        fingerprints,
        verify_runtime,
        check_delivery_key,
        max_due_age_seconds,
    )
    .await?;
    println!("{}", serde_json::to_string(&inspection)?);
    Ok(())
}

pub async fn inspect(
    pool: &PgPool,
    fingerprints: bool,
    verify_runtime: bool,
    check_key: bool,
    max_due_age_seconds: u32,
) -> Result<Inspection> {
    let allowed:bool=sqlx::query_scalar("SELECT current_user='turnesta_migrator' AND NOT rolsuper AND NOT rolbypassrls FROM pg_roles WHERE rolname=current_user").fetch_one(pool).await?;
    ensure!(
        allowed,
        "operational inspection requires the migration login"
    );
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;
    sqlx::raw_sql(
        "SET LOCAL statement_timeout='30s'; SET LOCAL lock_timeout='2s'; SET LOCAL timezone='UTC'",
    )
    .execute(&mut *tx)
    .await?;
    let (server_version_num, database): (i32, String) =
        sqlx::query_as("SELECT current_setting('server_version_num')::integer,current_database()")
            .fetch_one(&mut *tx)
            .await?;
    let actual: Vec<(i64, Vec<u8>, bool)> = sqlx::query_as(
        "SELECT version,checksum,success FROM public._sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut *tx)
    .await?;
    let expected: Vec<_> = database::MIGRATOR.iter().collect();
    ensure!(
        actual.len() == expected.len()
            && actual
                .iter()
                .zip(&expected)
                .all(|((version, checksum, success), m)| *success
                    && *version == m.version
                    && checksum.as_slice() == m.checksum.as_ref()),
        "database migrations differ from this binary; no migrations were applied"
    );
    let migrations = actual
        .into_iter()
        .map(|(version, checksum, _)| Migration {
            version,
            checksum: hex::encode(checksum),
        })
        .collect();
    let ownership_valid:bool=sqlx::query_scalar(r#"
        SELECT NOT EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
            WHERE (n.nspname IN('core','auth','catalog','queue','bookings','tips') OR (n.nspname='public' AND c.relname='_sqlx_migrations'))
            AND c.relkind IN('r','p','v','m','S') AND pg_get_userbyid(c.relowner)<>'turnesta_migrator')
        AND NOT EXISTS(SELECT 1 FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace
            WHERE n.nspname IN('core','auth','catalog','queue','bookings','tips') AND pg_get_userbyid(p.proowner)<>'turnesta_migrator')
        AND NOT EXISTS(SELECT 1 FROM pg_constraint k JOIN pg_class c ON c.oid=k.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace
            WHERE n.nspname IN('core','auth','catalog','queue','bookings','tips') AND NOT k.convalidated)
        AND NOT EXISTS(SELECT 1 FROM pg_index i JOIN pg_class c ON c.oid=i.indrelid JOIN pg_namespace n ON n.oid=c.relnamespace
            WHERE n.nspname IN('core','auth','catalog','queue','bookings','tips') AND (NOT i.indisvalid OR NOT i.indisready))
        AND NOT EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE c.relkind='r'
            AND n.nspname IN('core','auth','catalog','queue','bookings','tips')
            AND (EXISTS(SELECT 1 FROM pg_attribute a WHERE a.attrelid=c.oid AND a.attname='tenant_id' AND NOT a.attisdropped)
                OR (n.nspname='core' AND c.relname IN('users','organizations')))
            AND (NOT c.relrowsecurity OR NOT c.relforcerowsecurity))
    "#).fetch_one(&mut *tx).await?;
    ensure!(
        ownership_valid,
        "object ownership, constraints or indexes are invalid"
    );
    let relations:Vec<(String,String)>=sqlx::query_as("SELECT n.nspname,c.relname FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE c.relkind='r' AND (n.nspname IN('core','auth','catalog','queue','bookings','tips') OR (n.nspname='public' AND c.relname='_sqlx_migrations')) ORDER BY n.nspname,c.relname").fetch_all(&mut *tx).await?;
    let mut tables = Vec::new();
    for (schema, name) in relations {
        // Catalog identifiers are quoted, never accepted as SQL from a request.
        let relation = format!(
            "\"{}\".\"{}\"",
            schema.replace('"', "\"\""),
            name.replace('"', "\"\"")
        );
        let (rows, fingerprint) = if fingerprints {
            // Hash canonical JSON rows in sorted order; this is a local drill,
            // not a cheap online metric. Rust streams rows instead of collecting them.
            let query = format!(
                "SELECT to_jsonb(t)::text FROM {relation} t ORDER BY to_jsonb(t)::text COLLATE \"C\""
            );
            let mut stream =
                sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(query)).fetch(&mut *tx);
            let mut digest = Sha256::new();
            let mut count = 0;
            while let Some(row) = stream.try_next().await? {
                digest.update((row.len() as u64).to_be_bytes());
                digest.update(row.as_bytes());
                count += 1;
            }
            (count, Some(hex::encode(digest.finalize())))
        } else {
            // Exact counts are bounded by statement_timeout; no sample rows leave this tool.
            (
                sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
                    "SELECT count(*) FROM {relation}"
                )))
                .fetch_one(&mut *tx)
                .await?,
                None,
            )
        };
        tables.push(Table {
            schema,
            name,
            rows,
            fingerprint,
        });
    }
    let mut states = Vec::new();
    for (queue, relation) in [
        ("invitations", "auth.invitation_deliveries"),
        ("mock_callbacks", "tips.mock_delivery_outbox"),
    ] {
        let rows: Vec<(String, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT state,count(*) FROM {relation} GROUP BY state ORDER BY state"
        )))
        .fetch_all(&mut *tx)
        .await?;
        states.extend(rows.into_iter().map(|(state, count)| QueueState {
            queue,
            state,
            count,
        }));
    }
    let overdue:i64=sqlx::query_scalar(r#"
        SELECT (SELECT count(*) FROM auth.invitation_deliveries WHERE expires_at>now() AND
            ((state='pending' AND available_at<now()-make_interval(secs=>$1)) OR (state='leased' AND lease_until<now()-make_interval(secs=>$1))))
            +(SELECT count(*) FROM tips.mock_delivery_outbox WHERE
            (state='pending' AND available_at<now()-make_interval(secs=>$1)) OR (state='leased' AND lease_until<now()-make_interval(secs=>$1)))
    "#).bind(f64::from(max_due_age_seconds)).fetch_one(&mut *tx).await?;
    let expired_pending:i64=sqlx::query_scalar("SELECT count(*) FROM auth.invitation_deliveries WHERE state IN('pending','leased') AND expires_at<=now()").fetch_one(&mut *tx).await?;
    let encrypted_invitations_verified = if check_key {
        Some(crate::auth::delivery::verify_pending_invitations(&mut tx).await?)
    } else {
        None
    };
    tx.commit().await?;
    let tenant_scope_exercised = if verify_runtime {
        verify_runtime_access(pool).await?
    } else {
        false
    };
    Ok(Inspection {
        format_version: 1,
        server_version_num,
        database,
        migrations,
        tables,
        ownership_valid,
        runtime_verified: verify_runtime,
        tenant_scope_exercised,
        encrypted_invitations_verified,
        delivery: DeliveryHealth {
            states,
            overdue,
            expired_pending,
            max_due_age_seconds,
        },
    })
}

async fn verify_runtime_access(migrator: &PgPool) -> Result<bool> {
    let url = std::env::var("TURNESTA_VERIFY_RUNTIME_DATABASE_URL")
        .context("runtime verification URL is required")?;
    let options = crate::config::Config::from_lookup(|key| {
        if key == "DATABASE_URL" {
            Some(url.clone())
        } else {
            None
        }
    })?
    .database
    .options;
    let runtime = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(3))
        .connect_with(options)
        .await
        .context("runtime verification connection failed")?;
    let result=async {
        ensure!(database::is_runtime_role(&runtime).await?,"verification login is not a restricted runtime");
        let source:String=sqlx::query_scalar("SELECT current_database()||':'||coalesce(inet_server_addr()::text,'local')||':'||coalesce(inet_server_port()::text,'local')||':'||(SELECT oid::text FROM pg_database WHERE datname=current_database())").fetch_one(migrator).await?;
        let target:String=sqlx::query_scalar("SELECT current_database()||':'||coalesce(inet_server_addr()::text,'local')||':'||coalesce(inet_server_port()::text,'local')||':'||(SELECT oid::text FROM pg_database WHERE datname=current_database())").fetch_one(&runtime).await?;
        ensure!(source==target,"verification must target the inspected database");
        let unsafe_grants:bool=sqlx::query_scalar("SELECT has_any_column_privilege(current_user,'auth.identities','SELECT') OR has_any_column_privilege(current_user,'auth.invitation_deliveries','SELECT') OR has_any_column_privilege(current_user,'core.memberships','UPDATE') OR has_table_privilege(current_user,'core.memberships','INSERT') OR has_any_column_privilege(current_user,'core.team_commands','SELECT')").fetch_one(&runtime).await?;
        ensure!(!unsafe_grants,"private runtime grants are too broad");
        let visible:i64=sqlx::query_scalar("SELECT count(*) FROM core.locations").fetch_one(&runtime).await?;
        ensure!(visible==0,"tenant rows are visible without tenant context");
        let owners:Vec<(TenantId,UserId)>=sqlx::query_as("SELECT tenant_id,user_id FROM core.memberships WHERE role='owner' AND active ORDER BY tenant_id,user_id LIMIT 20").fetch_all(migrator).await?;
        for (tenant,user) in &owners {
            let mut session=TenantSession::begin(&runtime,*user,*tenant).await?;
            session.organization().await?;
            let rows:Vec<TenantId>=sqlx::query_scalar("SELECT DISTINCT tenant_id FROM core.locations").fetch_all(session.connection()).await?;
            ensure!(rows.iter().all(|id|id==tenant),"cross-tenant rows are visible");
            session.commit().await?;
        }
        Ok(!owners.is_empty())
    }.await;
    runtime.close().await;
    result
}
