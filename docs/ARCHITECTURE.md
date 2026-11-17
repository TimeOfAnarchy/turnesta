# Architecture

Turnesta is a modular monolith: one Rust executable runs the HTTP API, background worker, migrations or trusted administrative commands. PostgreSQL holds business state, sessions, idempotency journals and delivery queues. React provides the staff workspace; Askama and small JavaScript modules provide public pages. Docker builds both clients and the server. Production uses Caddy for HTTPS.

## Process and trust boundaries

The API uses a restricted runtime database login. A separate migrator owns the schema; a trusted administrative login provisions organizations and accounts. The worker can invoke narrow delivery functions, without reading arbitrary business or identity tables. Production publishes only the reverse proxy. Its metrics listener and the database stay on the Docker network.

Tenant tables use composite tenant/resource keys and FORCE RLS. `TenantSession` opens a SQLx transaction, sets transaction-local user/tenant context and checks active membership. Context values are supplied by authenticated server code, not trusted directly from HTTP input. These settings do not authenticate a caller with database credentials. Staff sessions and tenant membership are checked server-side. Roles cover the entire organization; specialist profiles are independent of login accounts. New authorized requests observe revoked membership, while previously authorized transactions may finish. Queue SSE rechecks staff access every fifteen seconds. See the [permissions guide](GETTING-STARTED.md#team-and-permissions).

## Commands and concurrency

Queue commands acquire a common queue lock for FIFO allocation and ticket transitions. Staff and visitor issuance use the same allocator. Idempotency records and business changes commit together. Uncertain browser requests are recovered explicitly with their original key, rather than automatically replayed offline.

Bookings uses explicit appointment offers and specialist calendars. Staff booking, public confirmation and schedule changes share calendar locking and occupancy checks. Known intervals are half-open; unknown duration is not filled with an assumed duration. Batch schedule creation is an atomic addition, not a recurring scheduler. Both paths confirm atomically on submission, without a browsing hold. Start-only offers cannot establish occupancy after an unknown end; by-arrangement offers use explicit capacity. See the [booking workflow](GETTING-STARTED.md#appointments).

Applied SQLx migrations are immutable. Migrations are embedded during compilation and run by a separate process before the new API starts. Old processes must stop before incompatible migrations. The update script stops its Compose API/worker, but independently managed replicas and administrative writers require operator coordination.

## Updates and jobs

Queue SSE carries refresh signals, not a durable business event log. One PostgreSQL listener per API process drives a bounded registry; clients reload authoritative snapshots, reconnect and fall back to reads. Bookings currently polls HTTP. Each connection starts with refresh; the client coalesces reads, rejects old generations and rereads on reconnect. A heartbeat only confirms a successfully applied snapshot. Stale display calls are hidden, rather than shown as current.

Invitation delivery uses an encrypted PostgreSQL outbox, short transactions, expiring leases, retry limits and fenced acknowledgements. Network SMTP work happens outside a database transaction. Delivery attempts are at least once; duplicate delivery remains possible after a lost acknowledgement, and SMTP acceptance does not prove arrival in the recipient's inbox. API/admin/worker share the delivery key, while only the worker receives SMTP credentials. See [key management](OPERATIONS.md#secrets-and-backups).

The public booking widget uses a separate embed route, exact allowed origins, route-specific CSP and a checked postMessage handshake. No client answers, contact details or booking capabilities are sent to the parent website. See [widget](BOOKINGS-WIDGET.md).

## Public access and client data

Visitor capabilities are separate from staff sessions. Public receipt reads expose no client contacts/form answers. A confirmation number is a stable business reference, not a bearer credential or public search key. Booking read access lasts 400 days from creation; Queue access lasts through an active ticket and for 24 hours after closure. These periods do not define business-data retention. Losing browser storage does not cancel a record. Expired keys remain reserved so an old request cannot become a new appointment/ticket.

Settings, roles, scope and capacity are checked independently of client-side presentation. Menu visibility does not disable APIs or public publication. Widget allowed origins constrain framing; they do not make a standalone public booking page private. See [public flows](GETTING-STARTED.md) and [widget isolation](BOOKINGS-WIDGET.md).

Public appointments preserve the shown form/answers and consent URL/time plus consent version/text hash. Changed form/consent versions require reconfirmation. The hash covers the consent string, not the linked website; old text is not retained as a searchable version history. Earlier records may lack a text hash/version. Staff contact edits do not rewrite the original form snapshot. [Data controls](OPERATIONS.md#client-data-and-retention) describe export, anonymization and access auditing.

## Operational limits

Tips is an explicit mock mode. No real payment provider, payout service, booking reminders or messenger integration is implemented. Menu visibility is a presentation preference, not an authorization boundary. There is no implemented subscription entitlement system or arbitrary page builder.

Local integration and browser verification must be distinguished from a real VPS rollout, real SMTP, a business website and physical-device testing. See [operations](OPERATIONS.md), [deployment acceptance](DEPLOYMENT.md#first-acceptance) and [development](DEVELOPMENT.md).
