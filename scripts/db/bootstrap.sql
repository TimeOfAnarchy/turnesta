\set ECHO none
\set QUIET on
\set VERBOSITY terse
\set SHOW_CONTEXT never
\getenv api_password TURNESTA_API_DB_PASSWORD
\getenv admin_password TURNESTA_ADMIN_DB_PASSWORD
\getenv migrator_password TURNESTA_MIGRATOR_DB_PASSWORD
\getenv worker_password TURNESTA_WORKER_DB_PASSWORD

-- A local development bootstrap, run as the existing POSTGRES_USER. Role names
-- are fixed; passwords enter through escaped psql literals, never shell SQL.
-- Keep credentials out of PostgreSQL statement/error logs in this session.
SET log_statement = 'none';
SET log_min_error_statement = 'panic';
SET client_min_messages = 'warning';

BEGIN;
DO $bootstrap$
BEGIN
    PERFORM pg_advisory_xact_lock(hashtextextended('turnesta.database-bootstrap', 0));

    IF current_user IN (
        'turnesta_runtime', 'turnesta_provisioner',
        'turnesta_api', 'turnesta_admin', 'turnesta_migrator', 'turnesta_worker', 'turnesta_delivery_worker'
    ) THEN
        RAISE EXCEPTION 'bootstrap must use a separate database administrator';
    END IF;

    IF EXISTS (
        SELECT FROM pg_database AS database
        JOIN pg_roles AS owner ON owner.oid = database.datdba
        WHERE database.datname = current_database()
        AND owner.rolname IN ('turnesta_runtime', 'turnesta_provisioner', 'turnesta_api', 'turnesta_admin', 'turnesta_worker', 'turnesta_delivery_worker')
    ) THEN
        RAISE EXCEPTION 'an application role must not own the database';
    END IF;
END
$bootstrap$;

SELECT format('CREATE ROLE %I', role_name)
FROM (VALUES
    ('turnesta_runtime'), ('turnesta_provisioner'),
    ('turnesta_api'), ('turnesta_admin'), ('turnesta_migrator'), ('turnesta_worker'), ('turnesta_delivery_worker')
) AS managed(role_name)
WHERE NOT EXISTS (SELECT FROM pg_roles WHERE rolname = role_name)
\gexec

ALTER ROLE turnesta_runtime NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS INHERIT;
ALTER ROLE turnesta_provisioner NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS INHERIT;
ALTER ROLE turnesta_api LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS INHERIT;
ALTER ROLE turnesta_admin LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS INHERIT;
ALTER ROLE turnesta_migrator LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS INHERIT;
ALTER ROLE turnesta_delivery_worker NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS INHERIT;
ALTER ROLE turnesta_worker LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS INHERIT;
ALTER ROLE turnesta_worker SET statement_timeout = '5s';
ALTER ROLE turnesta_worker SET idle_in_transaction_session_timeout = '5s';

SELECT format('ALTER ROLE turnesta_api PASSWORD %L', :'api_password')
\gexec
SELECT format('ALTER ROLE turnesta_admin PASSWORD %L', :'admin_password')
\gexec
SELECT format('ALTER ROLE turnesta_migrator PASSWORD %L', :'migrator_password')
\gexec
SELECT format('ALTER ROLE turnesta_worker PASSWORD %L', :'worker_password')
\gexec
\unset api_password
\unset admin_password
\unset migrator_password
\unset worker_password

-- Remove unexpected inherited privileges only from the managed application roles.
SELECT format('REVOKE %I FROM %I', parent.rolname, member.rolname)
FROM pg_auth_members AS membership
JOIN pg_roles AS parent ON parent.oid = membership.roleid
JOIN pg_roles AS member ON member.oid = membership.member
WHERE member.rolname IN (
    'turnesta_runtime', 'turnesta_provisioner',
    'turnesta_api', 'turnesta_admin', 'turnesta_migrator', 'turnesta_worker', 'turnesta_delivery_worker'
)
AND NOT (
    (parent.rolname = 'turnesta_runtime' AND member.rolname IN ('turnesta_api', 'turnesta_migrator'))
    OR (parent.rolname = 'turnesta_provisioner' AND member.rolname IN ('turnesta_admin', 'turnesta_migrator'))
    OR (parent.rolname = 'turnesta_delivery_worker' AND member.rolname = 'turnesta_worker')
)
\gexec

GRANT turnesta_runtime TO turnesta_api, turnesta_migrator WITH INHERIT TRUE;
GRANT turnesta_runtime TO turnesta_api, turnesta_migrator WITH SET FALSE;
GRANT turnesta_runtime TO turnesta_api, turnesta_migrator WITH ADMIN FALSE;
GRANT turnesta_provisioner TO turnesta_admin, turnesta_migrator WITH INHERIT TRUE;
GRANT turnesta_provisioner TO turnesta_admin, turnesta_migrator WITH SET FALSE;
GRANT turnesta_provisioner TO turnesta_admin, turnesta_migrator WITH ADMIN FALSE;
GRANT turnesta_delivery_worker TO turnesta_worker WITH INHERIT TRUE;
GRANT turnesta_delivery_worker TO turnesta_worker WITH SET FALSE;
GRANT turnesta_delivery_worker TO turnesta_worker WITH ADMIN FALSE;

-- PUBLIC privileges also apply to every application login. Only this selected
-- development database is changed; no other database or schema is reassigned.
SELECT format('REVOKE CREATE, TEMPORARY ON DATABASE %I FROM PUBLIC', current_database())
\gexec
SELECT format(
    'REVOKE ALL ON DATABASE %I FROM turnesta_runtime, turnesta_provisioner, turnesta_api, turnesta_admin, turnesta_migrator, turnesta_worker, turnesta_delivery_worker',
    current_database()
)
\gexec
SELECT format('GRANT CONNECT ON DATABASE %I TO turnesta_runtime, turnesta_provisioner, turnesta_delivery_worker', current_database())
\gexec
SELECT format('GRANT CONNECT, CREATE ON DATABASE %I TO turnesta_migrator', current_database())
\gexec

REVOKE CREATE ON SCHEMA public FROM PUBLIC, turnesta_runtime, turnesta_provisioner, turnesta_api, turnesta_admin, turnesta_worker, turnesta_delivery_worker;
GRANT USAGE, CREATE ON SCHEMA public TO turnesta_migrator;

-- The first server version used POSTGRES_USER for migrations. Transfer only
-- Turnesta's schema/relations and SQLx history, with an explicit owner guard.
DO $bootstrap$
DECLARE
    object record;
    migrator_oid oid := 'turnesta_migrator'::regrole;
    bootstrap_oid oid := current_user::regrole;
BEGIN
    IF EXISTS (
        SELECT FROM pg_namespace
        WHERE nspname = 'core' AND nspowner NOT IN (bootstrap_oid, migrator_oid)
    ) THEN
        RAISE EXCEPTION 'core schema has an unexpected owner; bootstrap left it unchanged';
    END IF;

    IF EXISTS (
        SELECT FROM pg_class AS relation
        JOIN pg_namespace AS namespace ON namespace.oid = relation.relnamespace
        WHERE namespace.nspname = 'core'
        AND relation.relkind IN ('r', 'p', 'S', 'v', 'm', 'f')
        AND relation.relowner NOT IN (bootstrap_oid, migrator_oid)
    ) THEN
        RAISE EXCEPTION 'core relation has an unexpected owner; bootstrap left it unchanged';
    END IF;

    IF EXISTS (
        SELECT FROM pg_class AS relation
        JOIN pg_namespace AS namespace ON namespace.oid = relation.relnamespace
        WHERE namespace.nspname = 'public' AND relation.relname = '_sqlx_migrations'
        AND (relation.relkind <> 'r' OR relation.relowner NOT IN (bootstrap_oid, migrator_oid))
    ) THEN
        RAISE EXCEPTION 'SQLx migration history has an unexpected owner/type; bootstrap left it unchanged';
    END IF;

    CREATE SCHEMA IF NOT EXISTS core AUTHORIZATION turnesta_migrator;
    ALTER SCHEMA core OWNER TO turnesta_migrator;

    FOR object IN
        SELECT namespace.nspname, relation.relname,
            CASE relation.relkind
                WHEN 'S' THEN 'SEQUENCE'
                WHEN 'v' THEN 'VIEW'
                WHEN 'm' THEN 'MATERIALIZED VIEW'
                ELSE 'TABLE'
            END AS object_kind
        FROM pg_class AS relation
        JOIN pg_namespace AS namespace ON namespace.oid = relation.relnamespace
        WHERE namespace.nspname = 'core'
        AND relation.relkind IN ('r', 'p', 'S', 'v', 'm', 'f')
        AND relation.relowner <> migrator_oid
        ORDER BY CASE WHEN relation.relkind = 'S' THEN 1 ELSE 0 END
    LOOP
        EXECUTE format('ALTER %s %I.%I OWNER TO turnesta_migrator', object.object_kind, object.nspname, object.relname);
    END LOOP;

    IF to_regclass('public._sqlx_migrations') IS NOT NULL THEN
        ALTER TABLE public._sqlx_migrations OWNER TO turnesta_migrator;
    END IF;
END
$bootstrap$;

REVOKE ALL ON SCHEMA core FROM PUBLIC;
REVOKE CREATE ON SCHEMA core FROM turnesta_runtime, turnesta_provisioner, turnesta_api, turnesta_admin;
REVOKE ALL ON SCHEMA core FROM turnesta_worker, turnesta_delivery_worker;

COMMIT;
\echo Turnesta database roles and ownership are ready.
