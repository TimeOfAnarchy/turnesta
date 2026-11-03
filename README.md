# Turnesta

Appointments, electronic queues and tips for service businesses.

## Features

- **Online booking** — specialist schedules, a public booking page and an embeddable widget for your website.
- **Appointments** — book, reschedule and cancel clients from the staff cabinet.
- **Electronic queue** — tickets via link or QR code, an operator workspace and a live display board.
- **Tips** — voluntary tips via link or QR code after a visit.
- **Team** — organizations, locations and roles for owners, managers, operators and employees.

## Getting started

Requirements: Docker with Docker Compose and PowerShell 7.4+.

```powershell
.\scripts\Initialize-Development.ps1
docker compose --env-file .env up --build --wait
```

Create an organization with its owner, then set the owner's password using the `owner.id` from the output:

```powershell
docker compose --env-file .env run --rm admin core organization-create --name "My business" --owner-name "Owner"
.\scripts\Set-LoginPassword.ps1 -User <owner-id> -Email owner@example.com
```

Sign in at http://127.0.0.1:8080/login.

## Tech stack

- Rust, Axum, Tokio, SQLx
- PostgreSQL
- React, TypeScript, Vite
- Docker Compose

## License

[AGPL-3.0](LICENSE)
