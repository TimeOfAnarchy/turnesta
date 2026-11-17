# Operations runbook

Use this runbook after [deployment](DEPLOYMENT.md). Agree an operator, incident contact, acceptable data loss and recovery time with the business. Successful readiness does not prove email delivery, data correctness or restore readiness. The project has local synthetic verification; a real VPS and external integrations remain unverified.

## Select the installation

Run from the selected server checkout; verify the Docker context before maintenance. Keep resolved secrets out of terminal logs.

```sh
dc() { docker compose --env-file .env --file deploy/compose.prod.yaml "$@"; }
docker context show
dc config --quiet
dc ps --all
dc run --rm --no-deps inspector
```

Root compose.yaml is development, not this server stack. db-bootstrap/migrator exiting successfully is normal. `/health` indicates a living process; `/health/ready` performs a bounded database check and fails during dependency/shutdown problems. Worker has internal metrics, not a separate readiness endpoint. Inspect the actual delivery queue/result.

## Password recovery

Email password reset is not implemented. A trusted administrator can set a temporary password for an existing account through `auth password-set`. This revokes that user's sessions/codes/setup grants across all organizations and requires a different personal password at next sign-in. It does not grant membership; use the verified existing user/email binding for recovery.

Use the existing **user UUID**, not an organization/membership UUID. Obtain it from an authorized team API response or provisioning record. If unavailable, a trusted database administrator can look up the existing binding in an interactive local psql session:

```sh
dc exec db psql --no-psqlrc -U turnesta -d turnesta
```

At the psql prompt, enter:

```sql
\prompt 'Account email: ' account_email
SELECT user_id FROM auth.identities
WHERE provider = 'email' AND subject = lower(:'account_email');
\q
```

Confirm the intended user/email before changing a credential. In a private **Bash** terminal, the following reads the temporary password without echoing it or putting it into command arguments/history:

```bash
set +x
IFS= read -r -p 'Existing user UUID: ' user_id
IFS= read -r -p 'Account email: ' account_email
IFS= read -r -s -p 'Temporary password (15-128 characters): ' temporary_password
printf '\n'
if printf '%s\n' "$temporary_password" | dc run --rm --no-deps -T admin auth password-set \
    --user "$user_id" --email "$account_email" --password-stdin; then
    reset_status=0
else
    reset_status=$?
fi
unset temporary_password
```

Proceed only when `reset_status` is 0 and the CLI reports `status: password_set`. A rejected binding or unavailable database does not confirm recovery. Privately deliver the temporary password and verify first sign-in/password setup. Restarting services is unnecessary. For an already signed-in user, use the account password-change form with the current password.

## Secrets and backups

Protect server `.env` and `.env.before-update*` with mode 0600 and restricted directory access. Retain dated encrypted off-server copies with the app digest/configuration corresponding to each backup. The delivery key is needed for active invitation payloads even when the dump itself is encrypted. Also protect SMTP/registry/rclone credentials and the private monitoring URL.

Generate an age identity on a trusted device with `age-keygen -o turnesta-backup.key`; obtain its public recipient with `age-keygen -y turnesta-backup.key`. Put only the recipient in server TURNESTA_BACKUP_AGE_RECIPIENT. Store the private identity and its backup separately from VPS/dumps. Losing it makes encrypted dumps unrecoverable. Old identities remain necessary for old archives after rotation.

`./scripts/backup.sh` writes a custom-format pg_dump, checks archive readability and optionally encrypts it without publishing a plaintext dump. Without a recipient it warns and keeps plaintext; confidentiality then depends on the chosen storage. It optionally copies through rclone TURNESTA_BACKUP_REMOTE and pings TURNESTA_BACKUP_PING_URL on success/failure. A monitor error can fail the command after a valid backup was saved.

Local retention defaults to the newest fourteen archives; offsite retention is separate. A failed archive is not a successful copy. Verify access to an offsite copy after losing the server. Keep the matching `.env`/delivery key independently. Database backups do not include a private age identity.

## Updates and restore

Use `update.sh IMAGE` after fetching the intended repository changes. It holds the maintenance lock, resolves/pins a digest, preserves the previous image/environment, backs up, stops its API/worker, then runs the new stack/migrations and inspector. Coordinate independently managed replicas/admin writers and expect downtime. Migration 32 changes appointment journal format; 40 changes public consent submission. Old incompatible processes must not run against those new schemas.

The no-argument update keeps the pinned version. Failure can restore `.env` without rolling back a changed schema/container. The printed old-image command is not a schema rollback. Use the appropriate restore plan/backup before an incompatible older image. Do not delete the saved previous image/environment while it is needed for recovery.

A failed stop can leave some services stopped. Configuration rollback does not promise automatic service restart; inspect `dc ps --all` and follow the printed recovery instructions before declaring the installation healthy.

`restore.sh ARCHIVE [IDENTITY_FILE]` asks for `restore` and destructively replaces the current database. It decrypts/authenticates encrypted input before replacing data, restores into turnesta_restore, swaps databases, starts the stack and runs inspector. Replaced data remains in turnesta_previous only until the next restore. There is no automatic pre-restore backup; take one explicitly if needed. A fresh-server recovery needs a fresh checkout, protected matching `.env`, the app image and backup, plus its age identity when encrypted.

Update/restore share a maintenance flock; backup has a separate flock. Locks wait at most ten minutes. Do not defeat them or infer completion from the existence of an archive filename alone.

## Scheduling and retention cleanup

Installer schedules backup followed by purge daily at 03:30 in the host's cron time zone. If it cannot write `/etc/cron.d`, install its printed user crontab. Confirm time zone and the first actual scheduled execution, including offsite copy/ping/purge. Ensure that the cron user can access Docker/rclone and its configuration. Root cron output goes to `/var/log/turnesta-backup.log`; verify log rotation. For user cron, arrange a retained log destination explicitly.

`./scripts/purge.sh --dry-run` counts eligible cleanup; without the flag it deletes in bounded batches. Expired authentication/delivery rows, command results and public claims have separate retention rules; compact reserved-key markers prevent replay as new commands. Appointment auto-anonymization is off by default, configurable by organization; untimed records without known end are not guessed. Access-log retention is configured separately as described below. Existing backups are not changed by purge/anonymization.

## Client data and retention

Owners can export matching appointments from an appointment's details in CSV or JSON. Select an existing phone or email: the export spans the whole organization, including cancelled appointments and other locations. A shared contact can match several people; it is not proof of one person's identity. Protect the downloaded file and verify the contact before sharing it. The audit event records server-side generation, not successful saving on a device.

Owner/manager can anonymize active or cancelled appointments. This clears the name/contacts/note/form answers and the related public request fingerprint while retaining the appointment's status, assignment and occupancy. It does not cancel or release a confirmed place. Contacts cannot be restored to that row; move/cancel remain possible for an active appointment. A staff command replay must not resurrect erased contacts.

Anonymization does not erase older backups, copies of exports, other devices' pending browser requests or the staff journal's command hash. Browser capability access continues to its normal expiry, with no client contacts/answers in the receipt. Plan backup/retention policies together rather than treating one erasure action as deletion of every copy.

| Data/control | Current behavior |
| --- | --- |
| Automatic appointment anonymization | Off by default; owner/manager can select 1..1200 months in organization settings |
| Eligibility | Cancelled appointments age from cancellation; other eligible records use their known end. Missing date/end is not filled with an assumed value |
| Client access log | Owner chooses 30..3650 days, default 365 |
| Completed staff command results | Retained for at least 180 days; compact key markers prevent an expired command executing again |
| Public booking receipt access | 400 days from creation, independently of appointment retention |
| Public Queue ticket access | While active, then 24 hours after closure |
| Backup archives | Newest fourteen local copies by default; offsite retention is separate |

Changing a setting does not immediately scrub data; scheduled `purge.sh` applies eligible cleanup in bounded batches. Increasing retention cannot restore deleted records. A receipt-access expiry does not cancel or delete the appointment/ticket.

Public booking stores the shown form/answer snapshot and consent version/text hash, policy URL and time. The hash covers the consent string, not the linked website. Old consent text is not retained as a version history; keep the wording separately if needed to check a historical hash. Earlier records may lack a hash/version. Form edits do not silently relabel old answers.

### Client access log

Only the active owner can view the access log. Filters/timestamps use UTC. Successful staff reads of appointment lists/details/command results, including replayed results, and owner CSV/JSON export are recorded. Events identify the actor/location/action and appointment or list parameters, without copying client names, contacts, notes or answers. Staff/location names are shown as they currently exist.

Data reads and audit insertion share a transaction: audit failure prevents returning client data and rolls back the corresponding appointment command. Denied/missing-resource requests, historical reads from before the feature and trusted direct database administration are outside this log. Retention-setting changes are not separately recorded as access events. A workspace polling every ten seconds can create up to 360 events/hour per open tab, so include that volume in storage/backup planning.

## Service limits

These bounds apply in addition to database capacity checks and are not a measured server capacity or SLA:

| Flow | Bound |
| --- | --- |
| New public bookings | 30/minute and 1000 active public appointments per location |
| Public bookings for one normalized phone | Three active online appointments per location; digit comparison does not equate +7 and 8 prefixes |
| Public guest Queue issuance | 30 new tickets per rolling minute and 500 total waiting tickets per queue |

Successful retries of an existing public intention do not consume another new-record quota. Manual appointments do not consume public-booking quotas. Undated public appointments remain active until cancellation. A submission without a phone is not subject to the phone-based quota. These checks do not verify a contact or establish one unique person per browser. Run representative traffic and dependency tests on the target deployment.

## Monthly restore rehearsal

1. Download a completed offsite copy. Record UTC time/name/size and trusted checksum when available. Select matching app/schema/configuration keys and start a recovery timer.
2. Use a disposable Linux VM with its own Docker Engine and empty volumes. Close outside access and isolate outgoing mail/production integrations. Set a test domain/origin and fresh database passwords; retain the backup's delivery key. Disable the mail profile, monitoring pings and offsite writes. Verify the selected Docker host and services.
3. Use a test Caddyfile with internal TLS for localhost if required, provide the age identity privately and run restore. The script starts API/Caddy; this must be the isolated VM, not the production server.
4. Check inspector migrations/role ownership/runtime verification, readiness and a known synthetic record or agreed business count. A positive encrypted_invitations_verified checks pending payload decryption; zero has no payload against which to verify the key. Readiness alone is insufficient.
5. Record source/digest, restored data age, actual read checks and measured recovery time in a restricted log. Stop/dispose of the temporary environment according to its data-handling procedure. If it fails, retain evidence, identify the last verified archive and report to the operator/business before retrying.

## Rotation

For database passwords, schedule maintenance, back up/protect the previous config and stop public writers. Change the administrator password through psql's hidden `\password turnesta` prompt; simply editing POSTGRES_PASSWORD does not change an initialized cluster. Keep the old session while testing a new login. Set independent application passwords and run db-bootstrap, which updates the four logins transactionally. Restart, inspect and check staff/mail access. Restoring an old `.env` does not reverse passwords already changed in PostgreSQL.

For delivery-key rotation, stop new enqueues/admin jobs, drain or expire old-key jobs with the old worker, verify the queue, stop the worker and deploy one new shared 32-byte key to API/admin/worker. Preserve old keys with their backups. A valid key format or an empty queue does not establish key equality. Age rotation requires a new identity/recipient and a verified offsite restore using it before retiring an old key.

## Monitoring and request logs

API/worker expose `/metrics` on internal port 9090 in Compose; scrape api:9090 and worker:9090 only when the worker is enabled. Caddy denies public `/metrics` paths. No host metrics port is published. With native processes, explicitly choose separate loopback listener addresses if needed. worker --once does not start a metrics listener.

Metrics include turnesta_http_responses_total, turnesta_http_request_duration_seconds, turnesta_http_rate_limited_total, pool usage/maximum, turnesta_sse_connections and turnesta_outbox_oldest_unsent_email_age_seconds. HTTP duration ends at response headers, so it is not total SSE lifetime. Labeled series appear after matching traffic; counters reset at restart. Unsent-email age is zero for an empty queue; database read failure makes worker scrape 503 instead of claiming a fresh queue.

API request-completed JSON logs contain request ID, matched route, method, status, duration and authenticated user/tenant context when verified. Client IP is hashed with independent TURNESTA_LOG_IP_SALT; blank salt rotates at process restart. Production trusts Caddy's overwritten X-Real-IP while API ports remain private. Invalid trusted headers must not become a client identity. Hashes support correlation, not authentication. Bodies, cookies, query strings, credentials and raw IP are not logged in that API event; Caddy's separate access log can contain raw network addresses and must be protected.

## Incident response and first launch

On suspected compromise, record UTC time/evidence and contact the business without disclosing client data/secrets. Restrict external access, preserve administrative access, stop public API/worker and pause cron/purge/updates to retain evidence. Preserve protected container/SSH/hosting/backup logs before rotating compromised credentials. Rebuild a compromised host from trusted sources and a verified archive; authorize reopening only after runtime permissions, data, delivery and a fresh offsite backup are checked.

The first VPS launch checklist is [deployment acceptance](DEPLOYMENT.md). Real SMTP, public TLS, client IP chain, offsite restore, actual cron and the business site/phones need live checks. Local synthetic verification is separate.

## Optional Windows tools

PowerShell 7.4 helpers Inspect-Local, Backup-Database, Restore-Drill, Start-LocalPilot, Update-LocalPilot and Test-Operations preserve private metadata using Windows ACLs. Backup bundles contain database.dump plus manifest; restore drills use a new private cluster without worker/host ports by default. They are useful Windows rehearsal tools, not dependencies of the Linux VPS workflow. Existing private stand data/configuration should be selected explicitly rather than inferred from a hard-coded developer path.
