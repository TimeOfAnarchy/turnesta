# Turnesta

Appointments, electronic queues and tips for service businesses.

## Features

- **Online booking** — specialist schedules, a public booking page and an embeddable widget for your website.
- **Appointments** — book, reschedule and cancel clients from the staff cabinet.
- **Electronic queue** — tickets via link or QR code, an operator workspace and a live display board.
- **Tips** — voluntary tips via link or QR code after a visit.
- **Team** — organizations, locations and roles for owners, managers, operators and employees.

## Getting started

Requirements: Docker with Docker Compose. This setup is for local development: the app listens on 127.0.0.1 only and uses development database passwords.

Create `.env` with random local keys and start the app:

```sh
./scripts/init-env.sh
docker compose --env-file .env up --build --wait
```

Create an organization with its owner and note `owner.id` in the output:

```sh
docker compose --env-file .env run --rm admin core organization-create --name "My business" --owner-name "Owner"
```

Set a temporary password for the owner (typed input is hidden, 15–128 characters):

```sh
read -rs PW && printf '%s' "$PW" | docker compose --env-file .env run --rm -T admin auth password-set --user <owner-id> --email owner@example.com --password-stdin; unset PW
```

Sign in at http://127.0.0.1:8080/login. On first sign-in you will be asked to choose your own password.

On Windows, use PowerShell 7: `.\scripts\Initialize-Development.ps1` instead of `init-env.sh`, and `.\scripts\Set-LoginPassword.ps1 -User <owner-id> -Email owner@example.com` to set the password.

## Tech stack

- Rust, Axum, Tokio, SQLx
- PostgreSQL
- React, TypeScript, Vite
- Docker Compose

## License

[AGPL-3.0](LICENSE)
