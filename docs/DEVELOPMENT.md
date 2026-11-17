# Development and verification

Use this guide for reproducible local builds and checks. The [architecture overview](ARCHITECTURE.md) describes process, tenant and concurrency boundaries. Validate deployment separately using the [server checklist](DEPLOYMENT.md#first-acceptance).

## Repository layout

| Path | Purpose |
| --- | --- |
| `apps/api/src` | Rust domain modules, HTTP, worker, configuration and CLI |
| `apps/api/templates`, `apps/api/web` | Embedded public HTML/CSS/JavaScript |
| `apps/web` | React staff workspace and frontend tests |
| `apps/api/tests` | HTTP and PostgreSQL integration tests |
| `migrations` | Immutable applied SQLx migrations |
| `scripts/*.sh` | Local/server installation and Linux maintenance |
| `scripts/test-postgres.mjs` | Cross-platform Node/Docker database test runner |
| `scripts/*.ps1`, `scripts/ops` | Optional Windows workflows |
| `deploy/compose.prod.yaml` | Linux VPS stack, separate from local Compose |
| `deploy/compose.local*.yaml` | Isolated stand configurations used by Windows helpers |
| `tests/*.test.sh`, `e2e` | Deployment regressions and browser tests |

## Start locally

Use the root README's installer for a complete first owner. For development without owner bootstrap:

```sh
./scripts/init-env.sh
docker compose --env-file .env up --build --wait
curl --fail http://127.0.0.1:8080/health/ready
docker compose logs --tail 30 api worker
```

The stack includes its local MailHog inbox (default loopback 8025). API and PostgreSQL host ports are loopback 8080/54329 by default. Change API port and public origin together. Local credentials and insecure local SMTP are development settings; deploy using the separate production Compose file.

The binary reads process environment and does not load `.env` by itself. Compose supplies the appropriate database role. Reference host URLs such as ADMIN_DATABASE_URL do not switch the binary's DATABASE_URL automatically. `init-env.sh` creates independent delivery/mock/log secrets, preserves existing values and rejects invalid existing keys. Applied database passwords are not all changed merely by editing POSTGRES_PASSWORD; see [operations](OPERATIONS.md).

## Rust checks without host Rust

```sh
docker build --target devtool --tag turnesta-devtool:local .
docker run --rm turnesta-devtool:local cargo fmt --check
docker run --rm turnesta-devtool:local cargo clippy --workspace --all-targets --locked -- -D warnings
docker run --rm turnesta-devtool:local cargo test --workspace --locked
```

For repeated work, add named mounts for `/usr/local/cargo/registry`, `/usr/local/cargo/git` and `/workspace/target`. Rebuild the devtool after changing its copied sources, or explicitly bind the current source tree. The database runner binds the selected repository. Docker --mount cannot represent a comma in its source path; the runner rejects that case clearly. Paths with spaces are supported through argument arrays.

## PostgreSQL tests

Require Node.js 22.13+, a running local synthetic Compose stack and the devtool image. The runner reads resolved Compose configuration without sourcing `.env`, validates local targets, creates sibling `turnesta_test`, applies infrastructure/migration checks first and runs selected suites serially. Test variables are passed by environment name rather than credential-bearing CLI arguments. Configuration errors do not print raw secrets.

```sh
node scripts/test-postgres.mjs
node scripts/test-postgres.mjs core_postgres delivery_postgres
node scripts/test-postgres.mjs --env-file .env --file compose.yaml --project turnesta-dev bookings_postgres
node scripts/test-postgres.mjs --help
```

Suites: core_postgres, auth_store_postgres, auth_http_postgres, delivery_postgres, tips_postgres, tips_http_postgres, queue_postgres, queue_public_postgres, bookings_postgres, team_postgres, operations_postgres and password_postgres. Infrastructure suite `postgres` always runs first. Direct Cargo test configuration must point all TEST_* URLs at a dedicated `_test` database and preserve that ordering. Tests retain synthetic rows; no automatic volume/database removal is performed.

The older Test-Postgres.ps1 remains optional compatibility tooling. Windows private pilot/backup/restore helpers use PowerShell 7.4 and Windows ACLs; they are not portable Linux deployment requirements.

## Web and browser checks

From `apps/web` with Node.js 22.13+:

```sh
npm ci
npm --prefix tools/lint ci
npm run lint
npm test
npm run build
npm run api:types -- --check
```

After API changes, refresh the committed OpenAPI snapshot from an updated local API with `npm run api:snapshot`, then regenerate types using `npm run api:types`. CI checks both with --check against the running API. Do not hand-edit generated types. Generated schemas coexist with handwritten transport/DTOs.

Use [e2e/README](../e2e/README.md) for isolated Playwright setup. Browser tests create synthetic organizations and exercise public/staff/widget flows, error recovery, drafts and accessibility. Vite dev proxy is a development convenience; it must respect the API's exact Origin and CSRF checks.

## Migrations and checks for deployment scripts

SQLx embeds migrations and stores checksums; applied files must not be edited, including line endings. Add a new migration for schema/function changes. LF is fixed for SQL/shell files. Migration and API lifecycle are separate; `update.sh` stops its API/worker before new migrator startup. Coordinate independent writers too.

```sh
sh tests/deploy-update.test.sh
sh tests/maintenance-lock.test.sh
sh tests/backup-restore.test.sh
sh tests/purge.test.sh
```

These use controlled Docker substitutes and real Linux utilities. Git Bash lacks flock on some installations, so use a Linux environment for maintenance regressions. They do not establish successful real VPS operations. Run syntax/ShellCheck on affected scripts as in CI. `caddy-metrics.test.sh` additionally needs a live target.

## Contract changes and verification scope

The committed [OpenAPI snapshot](../apps/web/openapi.json) and Rust HTTP handlers define API shapes. Use the snapshot for inspection; `/openapi.json` is available on a local loopback setup and must be explicitly exposed for a server. Generated schema types coexist with handwritten request transport and some DTOs. The snapshot does not establish runtime authorization behavior.

Preserve organization isolation, current-role checks, optimistic versions and exact-command recovery when changing an endpoint. Reusing a staff command key with changed content conflicts; a successful replay is distinct from a fresh intention. Public Queue/Bookings recovery has its own capability contract. Never infer rollback from a timeout. See [architecture](ARCHITECTURE.md) and the [user recovery guidance](GETTING-STARTED.md#passwords-and-uncertain-actions).

Run focused checks appropriate to the change and report the command, result and missing coverage. Rust's ordinary test command skips ignored database tests; a successful build is not a database or browser test. Deployment shell regressions use Docker substitutes; they are not a live VPS update/restore check. A passing local browser test does not verify real SMTP, a business website or physical phones.
