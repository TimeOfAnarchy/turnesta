# Turnesta

Open-source platform for service businesses: appointments, electronic queues and tips in one place.

> **Early stage.** Turnesta is being piloted locally with a small clinic. It is not production-ready yet, and real payments are not implemented.

## What it does

- **Bookings** — specialist schedules, manual and bulk slot creation, client appointments with reschedule and cancel, a public booking page and an embeddable widget for your website.
- **Queue** — electronic tickets via link or QR code, an operator workspace with service windows, and an anonymized display board with live updates over SSE.
- **Tips** — voluntary tips via link or QR after a visit. Only an explicit mock payment mode exists for now.
- **Team** — organizations, locations, roles (owner, manager, operator, employee), email and password sign-in.

## Quick start

Requires Docker Desktop with Linux containers. Run from the project root in PowerShell:

```powershell
.\scripts\Initialize-Development.ps1
docker compose --env-file .env up --build --wait
```

Then open `http://127.0.0.1:8080/app/`. The script creates an ignored `.env` with random local keys; the example environment contains development-only credentials. Outgoing mail is captured by a local MailHog. To create the first owner and password, see [PASSWORDS.md](docs/PASSWORDS.md).

Details on configuration, tests and code structure: [DEVELOPMENT.md](docs/DEVELOPMENT.md).

## Stack

- **Backend:** Rust, Axum, Tokio, SQLx, PostgreSQL; REST API with OpenAPI at `/openapi.json`.
- **Cabinet:** React, TypeScript, Vite.
- **Public pages** (booking, ticket, board, tips): server-rendered HTML with a little JavaScript.
- **Background jobs:** PostgreSQL outbox and a worker built from the same codebase.
- **Deployment:** Docker Compose.

## Documentation

Documentation is currently in Russian.

- Product: [PRODUCT](docs/PRODUCT.md), [ROADMAP](docs/ROADMAP.md), [MONETIZATION](docs/MONETIZATION.md)
- Architecture: [ARCHITECTURE](docs/ARCHITECTURE.md), [STACK](docs/STACK.md), [CORE](docs/CORE.md)
- Bookings: [overview](docs/BOOKINGS.md), [editor](docs/BOOKINGS-EDITOR.md), [appointments](docs/BOOKINGS-APPOINTMENTS.md), [public page](docs/BOOKINGS-PUBLIC.md), [widget](docs/BOOKINGS-WIDGET.md), [bulk fill](docs/BOOKINGS-BATCH.md)
- Queue: [overview](docs/QUEUE.md), [live updates](docs/QUEUE-LIVE.md), [services](docs/SERVICES.md), [service windows](docs/SERVICE-POINT.md), [tips after a visit](docs/QUEUE-TIPS.md)
- Tips: [mock flow](docs/TIPS-MOCK.md), [payments in Russia](docs/TIPS-RU.md)
- Accounts: [team](docs/TEAM.md), [passwords](docs/PASSWORDS.md), [sign-in and sessions](docs/AUTH.md), [cabinet](docs/CABINET.md), [navigation](docs/NAVIGATION.md), [mail delivery](docs/DELIVERY.md)
- Operations: [local pilot, backup and restore](docs/OPERATIONS.md), [development](docs/DEVELOPMENT.md), [sources](docs/SOURCES.md)

## License

[GNU Affero General Public License v3.0](LICENSE) (`AGPL-3.0-only`). You may use, modify and run Turnesta, including commercially. If you offer a modified version to users over a network, you must make its source code available to them under the same license.
