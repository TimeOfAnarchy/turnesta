# Turnesta

Appointments, electronic queues and mock tips for service businesses.

- **Bookings:** specialist schedules, staff appointments, a public booking page and a website widget.
- **Queues:** visitor tickets via link or QR code, an operator workspace and a live display board.
- **Team:** organizations, locations and owner/manager/operator/employee roles.
- **Mock Tips:** simulated checkout and callbacks; real charges and payouts are unavailable.

The interface is currently Russian. Booking holds, visitor cancellation/rescheduling and reminders are not implemented. Local integration checks have passed; a real VPS, external SMTP and a business website have not yet been verified.

## Try locally

Install Git and Docker with the Compose plugin. Run these commands in a POSIX shell; on Windows use Git Bash and Docker Desktop with Linux containers.

```sh
git clone https://github.com/TimeOfAnarchy/turnesta.git
cd turnesta
./scripts/install.sh
```

The installer builds the application, creates the first organization/owner and prints the sign-in address and temporary password. Set a personal password on first sign-in, then follow [first configuration](docs/GETTING-STARTED.md).

Local mode binds to loopback and uses development database credentials. Its included [MailHog inbox](http://127.0.0.1:8025) captures email without sending it externally. Ports and restart behavior are covered in [deployment](docs/DEPLOYMENT.md#local-evaluation).

## Run on a Linux server

In a fresh server checkout with Docker, Compose, Buildx, flock, curl and a configured domain, run:

```sh
./scripts/install.sh --server
```

The server uses a prebuilt application image and Caddy HTTPS. PowerShell, host Rust and host Node.js are not required. Follow the [deployment guide](docs/DEPLOYMENT.md#linux-vps) for prerequisites, email and first acceptance, then the [operations runbook](docs/OPERATIONS.md) for updates, backups and recovery.

## Documentation

| Task | Guide |
| --- | --- |
| Find the right instructions | [Documentation index](docs/INDEX.md) |
| Configure a business and staff | [First configuration](docs/GETTING-STARTED.md) |
| Install locally or on a VPS | [Deployment](docs/DEPLOYMENT.md) |
| Update, back up or restore | [Operations](docs/OPERATIONS.md) |
| Embed booking on a website | [Booking widget](docs/BOOKINGS-WIDGET.md) |
| Build, test and contribute | [Development](docs/DEVELOPMENT.md) |
| Understand the implementation | [Architecture](docs/ARCHITECTURE.md) |

Built with Rust, Axum, Tokio, SQLx, PostgreSQL, React, TypeScript and Vite.

[Security policy](SECURITY.md) · [Changelog](CHANGELOG.md) · [AGPL-3.0 license](LICENSE)
