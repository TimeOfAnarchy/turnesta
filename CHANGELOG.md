# Changelog

## Unreleased

- Published a bilingual product tour and README with screenshots from v0.1.0.

## [0.1.0](https://github.com/TimeOfAnarchy/turnesta/releases/tag/v0.1.0) — 2026-11-18

Initial pre-release for evaluation. The `v0.1.0` tag points to `24d166e`. The release includes source archives, their checksums and the digest of the Linux AMD64 image tested by CI.

### Added

- Organizations, locations, owner/manager/operator/employee roles, PostgreSQL tenant isolation and a staff web cabinet.
- Email-code and password sign-in, server-side sessions, temporary-password setup, password changes and an administrator reset command.
- Team invitations and role/access management, with an encrypted delivery outbox and a separate email worker.
- Electronic queues with idempotent ticket commands, an operator workspace, public tickets, QR links, an anonymized display, service catalogs and service windows. Queue screens use SSE with snapshot recovery.
- Booking schedules, specialists, optional presets, flexible appointment times, day copying and batch schedule creation. Staff can record, edit, move and cancel appointments.
- Public booking pages with configurable forms and consent, plus an embeddable widget with allowed origins and adaptive sizing.
- **Mock-only Tips** with a simulated checkout, callback delivery and reconciliation, including an optional link after queue service. Real payments and payouts are not implemented.
- Client-data export and anonymization, versioned consent records, staff access auditing and retention cleanup.
- Local and server deployment scripts, HTTPS proxy configuration, digest-pinned updates, backups, optional `age` encryption and isolated restore verification.
- Internal Prometheus metrics and request logs with tenant/user identifiers and a hashed client IP.
- CI checks for Rust, PostgreSQL, frontend code, dependencies and container images; Playwright scenarios, generated OpenAPI types and ESLint checks.
- A security reporting policy and this changelog.

### Fixed

- Updates and restores share a maintenance lock, including failed-update cleanup; backup locking remains independent.
- Booking confirmations show a stable organization-scoped number in the public page and staff cabinet, with copy and manual-selection fallback.
- Backup scheduling documentation reflects the server cron time zone and manual installation fallback.
- Public booking limits count active appointments, and password failure budgets are scoped to avoid a global lockout.
- Unexpected database failures return internal errors; API database operations have bounded statement, lock and idle-transaction times.
- Client admission limits, production OpenAPI visibility, widget origin validation and deployment resource limits were tightened.
- Appointment command journals exclude client personal data. Audit, retention and consent regressions were corrected.
- API and worker startup reject an invalid delivery key; API startup rejects an unsuitable database role.
- Booking form hint contrast and spacing between client privacy actions were corrected.

### Changed

- Large booking and appointment components were separated into hooks and views, and a top-level error boundary was added.
- Backend timeout, location-check, SQLSTATE and rate-limit helpers are shared while preserving existing API error codes and response formats.
- README links to the security policy and changelog and identifies Tips as mock-only.

### Current limits

Local integration, browser and backup/restore checks have been performed. Deployment to a real VPS, external SMTP delivery and integration with a real business website have not been verified. Booking holds, booking reminders, messenger integrations and real payment providers remain future work.
