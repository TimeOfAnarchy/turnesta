# Deployment

Choose local evaluation for development and the separate Linux server stack for hosting. For the product overview and quick start, see the [README](../README.md).

## Local evaluation

Install Git, Docker with Compose and a POSIX shell. On Windows use Docker Desktop's Linux containers and Git Bash. Clone the repository, enter it and run `./scripts/install.sh`. The first build compiles Rust/React inside containers. The script asks for a port/organization/owner/email, creates ignored development configuration and bootstraps a temporary password on an empty installation. First sign-in sets a personal password.

Default local addresses and their `.env` settings:

| Service | Default | Setting |
| --- | --- | --- |
| API/workspace | `http://127.0.0.1:8080` | `TURNESTA_API_PORT` and `TURNESTA_PUBLIC_ORIGIN` |
| PostgreSQL | `127.0.0.1:54329` | `TURNESTA_POSTGRES_PORT` |
| MailHog inbox | `http://127.0.0.1:8025` | `TURNESTA_MAILHOG_PORT` |

MailHog captures email without delivering to real mailboxes. Change conflicting ports in `.env`, keeping API port and public origin consistent, then rerun the installer. Use local development credentials only for evaluation. Existing data/credentials are preserved, but bootstrap questions repeat; routine startup is `docker compose up --wait`. Follow [first configuration](GETTING-STARTED.md) after sign-in.

## Linux VPS

Prerequisites: Linux, Git, Docker Engine, Compose and Buildx plugins, flock/util-linux, curl, a domain with correct A/AAAA records and reachable TCP 80/443. Configure the server firewall/administrative access for the selected host. The current image CI runs on AMD64 Linux; other architectures need a compatible supplied image and separate verification. No host PowerShell, Node or Rust is required.

```sh
git clone https://github.com/TimeOfAnarchy/turnesta.git
cd turnesta
./scripts/install.sh --server
```

For restricted registry access, sign in to GHCR with an account allowed to read the image first. Installation resolves/pins the app image digest, generates independent database/delivery/log secrets and starts Caddy HTTPS. Keep `.env` private and save its protected off-server copy. Development configuration is not converted in place into server configuration; use a separate server checkout.

Use the production Compose file for all manual administration:

```sh
dc() { docker compose --env-file .env --file deploy/compose.prod.yaml "$@"; }
docker context show
dc config --quiet
dc ps --all
curl --fail https://your-domain/health/ready
```

The root Compose file is local development. Production does not publish PostgreSQL, API or metrics ports; Caddy is the external entry point. Memory/CPU limits are configuration, not measured capacity guarantees. Confirm available resources on the actual workload before committing to a server size.

## Email

The first owner can sign in with a password without SMTP. To send invitations/email codes, set `TURNESTA_SMTP_HOST`, `TURNESTA_SMTP_PORT`, `TURNESTA_SMTP_FROM`, and any required username/password in the server `.env`. Use `TURNESTA_SMTP_TLS_MODE=smtps` (normally 465) or `starttls` (normally 587), according to your provider, then enable `COMPOSE_PROFILES=mail` and run `dc up --wait`.

The worker sends queued invitations. A running worker or SMTP acceptance is not proof of inbox delivery; test receipt and sign-in using a controlled real mailbox. Local MailHog is not part of the server stack.

## First acceptance

1. Sign in as the initial owner and choose a personal password. Follow [business setup](GETTING-STARTED.md).
2. Verify publicly trusted TLS and redirect from a separate network/device.
3. If email is needed, set SMTP host/port/from/credentials and TLS mode, enable `COMPOSE_PROFILES=mail`, restart with `dc up --wait` and verify an invitation in a controlled real mailbox. Password login alone does not prove email works.
4. Set up backup encryption/offsite copies, protect the matching `.env` and age identity, and perform an isolated restore drill.
5. Verify the daily backup/purge schedule and the first actual scheduled result. It is 03:30 in the host cron time zone, not implicitly UTC. Install the printed crontab if the installer could not write `/etc/cron.d`.
6. Configure uptime/readiness and backup-success/failure monitoring; test the actual business website/widget and target phones.

## Maintenance

```sh
./scripts/backup.sh
git pull
./scripts/update.sh ghcr.io/timeofanarchy/turnesta:latest
./scripts/restore.sh backups/turnesta-DATE.dump
./scripts/restore.sh backups/turnesta-DATE.dump.age /path/to/age-identity.txt
```

An update pins the chosen digest, saves an environment snapshot, backs up and stops API/worker before migrations. Expect downtime. Stop independently managed replicas/admin writers separately. No-argument update retains the pinned version. A failed update can restore `.env` while containers/schema have already changed; review the printed recovery commands and use a matching database backup when reverting an incompatible image.

Restore is destructive and asks for explicit `restore`. It restores into a staging database before swapping and retains the replaced database until the next restore. It does not automatically back up the current database first. See [operations](OPERATIONS.md) for backup keys, rotation, incident response and rehearsals.

The project has been checked locally with synthetic data. A real VPS deployment, external SMTP service and real business website integration remain unverified; complete the acceptance checks above on the target environment.
