# Turnesta

Appointments, electronic queues and tips for service businesses.

## Features

- **Online booking** — specialist schedules, a public booking page and an embeddable widget for your website.
- **Appointments** — book, reschedule and cancel clients from the staff cabinet.
- **Electronic queue** — tickets via link or QR code, an operator workspace and a live display board.
- **Tips** — voluntary tips via link or QR code after a visit.
- **Team** — organizations, locations and roles for owners, managers, operators and employees.

## Getting started

Requirements: Docker with Docker Compose. On Windows, run the command in Git Bash.

```sh
./scripts/install.sh
```

The script asks for a port, an organization name and an owner, builds and starts the app, and prints the sign-in address with a temporary password. You choose your own password on first sign-in. Running it again is safe: it restarts the app and keeps existing data.

This setup is for local use: the app listens on 127.0.0.1 only and uses development database passwords.

If a port is busy, change it in `.env` and run the script again: `TURNESTA_API_PORT` together with the port in `TURNESTA_PUBLIC_ORIGIN`, or `TURNESTA_POSTGRES_PORT`.

## Tech stack

- Rust, Axum, Tokio, SQLx
- PostgreSQL
- React, TypeScript, Vite
- Docker Compose

## License

[AGPL-3.0](LICENSE)
