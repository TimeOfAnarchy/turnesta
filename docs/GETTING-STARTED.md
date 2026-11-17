# First business configuration

Complete [local or server installation](DEPLOYMENT.md) and open the printed `/login` address. Sign in with the initial email/temporary password and choose a personal password of 15..128 characters. The current interface is Russian; section names below describe the corresponding controls in English.

The installer creates the first organization. Creating further organizations is a trusted administrative operation. Rerunning the installer preserves an existing organization's credentials; it does not reset a forgotten password.

## Appointments

1. Open Locations and add the location name and IANA time zone. Confirm any browser suggestion before creating a schedule.
2. Open Bookings and add a specialist. This is a scheduling/public profile, independent of a staff login. A single specialist is selected automatically.
3. On Schedule, add an offer manually or use an optional preset. An interval has a known end, start-only leaves the end unknown, and by-arrangement can leave time/date unspecified. Confirmation without a time requires further coordination with the business. Existing offers retain their saved time zone.
4. Test a manual appointment with synthetic client data. Open its details, edit contacts, move it to another offer of the same specialist and cancel it. Cancellation frees the place and keeps history. Cross-specialist moves, restoring a cancelled appointment, completion and no-show are unavailable.
5. Optional presets and batch scheduling speed up entry. Review the preview and conflicts before saving a copied day or preset series. Copying creates new offers without clients. Accepting overlapping offers as alternatives does not permit conflicting client bookings.
6. On Online booking, configure the form, privacy-policy URL and consent text, then explicitly enable publication. It is off by default. Name is always required; phone is required by default and other fields are configurable. Only available scheduled offers are public.
7. In a separate visitor session, choose an offer and submit the form. Booking confirms immediately on submission; browsing/filling a form does not hold a place. Compare the confirmation number with the staff record. The stable organization-wide number has at least six digits and is not a public access key.

Pausing public booking stops new submissions while preserving existing appointments and their receipt access. Staff handles cancellation/rescheduling; visitor self-service, holds and reminders are not implemented. The [operations guide](OPERATIONS.md#service-limits) lists current public limits.

To place booking on another website, save its exact origin and use the generated loader code. Follow the [widget guide](BOOKINGS-WIDGET.md); the standalone public link also works without embedding.

## Team and permissions

The owner manages staff accounts, roles and email invitations. Temporary-password accounts require a personal password on first sign-in. Invitations require working SMTP/worker; local installation captures them in MailHog. An active emailed code lasts ten minutes; a delayed/expired invitation may need a new request.

| Action | Owner | Manager | Operator | Employee |
| --- | --- | --- | --- | --- |
| Read locations, schedules and queues | Yes | Yes | Yes | Yes |
| Edit location/organization settings, schedules and public publication | Yes | Yes | No | No |
| Record/edit/move/cancel client appointments | Yes | Yes | Yes | No |
| Issue/call Queue tickets | Yes | Yes | Yes | No |
| Anonymize client data | Yes | Yes | No | No |
| Export matching client appointments and view the access log | Yes | No | No | No |
| Manage the team | Yes | No | No | No |

Roles apply to the whole organization, across all its locations. A specialist profile does not restrict staff access. Owners cannot be changed through team management; ownership transfer and per-location permissions are unavailable. Revocation blocks new authorized requests; already-authorized work may finish.

Settings can hide Queue, Bookings or Tips from the shared menu. This does not disable their APIs or public pages. Pause public booking, visitor issuance or display publication in that service's own controls.

## Passwords and uncertain actions

Staff can change their own password after verifying the current one. Email password reset is unavailable; a trusted administrator can issue a temporary password using the [recovery procedure](OPERATIONS.md#password-recovery).

After an uncertain save or network timeout, use the offered explicit recovery action in the same browser/profile. Keep the original saved request; do not create a second intention merely because the response was lost. Timeout does not prove rollback. If browser storage was lost, ask the business to check the record before making a replacement. Losing a browser receipt does not cancel an appointment.

## Client data

Owner contact-based export and owner/manager anonymization are available from appointment details. Shared phone/email can match more than one person. Anonymization clears contacts/answers but preserves the appointment and its occupancy; it does not cancel it. Automatic anonymization is off by default. See [data controls and retention](OPERATIONS.md#client-data-and-retention) before choosing a policy or downloading client files.

## Electronic queues

Create a queue at a location; optionally add services and service windows. Enable visitor ticket issuance and the anonymized display independently. Test staff/visitor issuance in one FIFO, calling at a selected window and completion/cancellation. An operator can finish/cancel their own called ticket; owner/manager can do so for any called ticket in that queue.

Service selection is optional and does not create a separate FIFO. Window names are public and should describe a place without personal data. Same-named windows in different queues are independent resources. Check the live/disconnected state before relying on a display; stale calls are hidden.

Tips is mock-only and disabled by the server configuration. It does not charge or pay out money. Complete [server and website acceptance](DEPLOYMENT.md#first-acceptance) before real clients use public flows.
