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

This mode is for trying Turnesta on your computer: it listens on 127.0.0.1 only and uses development database passwords.

If a port is busy, change it in `.env` and run the script again: `TURNESTA_API_PORT` together with the port in `TURNESTA_PUBLIC_ORIGIN`, or `TURNESTA_POSTGRES_PORT`.

## Running on a server

Requirements: a Linux server with Docker, a domain pointing to the server, and open ports 80 and 443.

```sh
git clone https://github.com/TimeOfAnarchy/turnesta.git
cd turnesta
./scripts/install.sh --server
```

The script generates strong passwords and uses a ready-made image, so the server does not need to compile anything. HTTPS certificates are issued automatically. To send email, fill in the SMTP settings in `.env`, uncomment `COMPOSE_PROFILES=mail` and run the script again.

Everyday commands, run from the project folder:

```sh
./scripts/backup.sh
git pull && ./scripts/update.sh
./scripts/restore.sh backups/turnesta-<date>.dump
```

The database is backed up every day at 03:30 UTC and the last 14 copies stay in `backups/`. An update makes a backup first; `update.sh <image>` installs a specific version. To keep copies off the server, configure an encrypted [rclone](https://rclone.org/crypt/) remote and set `TURNESTA_BACKUP_REMOTE` in `.env`.

Keep a copy of `.env` somewhere safe: it holds the keys needed to restore on a new server. Point an uptime monitor at `https://your-domain/health/ready`.

To move to a new server, copy `.env` and a backup into a fresh clone, then run `restore.sh`.

## Tech stack

- Rust, Axum, Tokio, SQLx
- PostgreSQL
- React, TypeScript, Vite
- Docker Compose

## License

[AGPL-3.0](LICENSE)
