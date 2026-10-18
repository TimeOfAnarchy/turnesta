\set ON_ERROR_STOP on
-- Dedicated integration database, on this local development PostgreSQL only.
SELECT 'CREATE DATABASE turnesta_test OWNER turnesta_migrator'
WHERE NOT EXISTS (SELECT 1 FROM pg_database WHERE datname = 'turnesta_test')
\gexec
REVOKE CREATE, TEMPORARY ON DATABASE turnesta_test FROM PUBLIC;
\connect turnesta_test
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
GRANT USAGE, CREATE ON SCHEMA public TO turnesta_migrator;
