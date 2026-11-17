# Booking website widget

The staff workspace generates a loader snippet for another website. Save that website's exact origin in the location's widget settings, publish public booking separately, then copy the snippet. Use the generated real URL/slug, not a copied placeholder. The ordinary public link remains a usable fallback.

## Connect a website

1. As owner/manager, choose the location in Bookings, create available offers and enable public booking. Test its standalone link first.
2. In that location's widget settings, save the website's exact origin, such as `https://clinic.example`. Save bare and `www` origins separately if both serve the page.
3. Copy the generated code into the website HTML. It includes the public link, a `data-turnesta-booking` container and the loader script. Keep the real URLs/slug from the workspace.
4. If the website has CSP, permit the Turnesta origin in `script-src` and `frame-src`. Open the published top-level page, not just a CMS preview.
5. In a separate visitor session, load the calendar, submit a synthetic booking, compare its confirmation number with staff records, then cancel it through staff. Check a narrow viewport and the browser's storage restrictions before sharing the widget with clients.

For local testing use different origins: for example, Turnesta at `http://127.0.0.1:8080` and the website at `http://localhost:5500`. Both hosts and ports must match the saved settings. The loader refuses same-origin embedding.

## Visitor behavior

The widget confirms a booking when the form is successfully submitted, without a hold while browsing or a separate staff approval. No visitor cancellation/rescheduling or reminders are implemented. Staff handles changes. Pausing public booking prevents new submissions while preserving earlier receipts and successful-request recovery.

The organization-wide confirmation number has at least six digits, survives move/cancel/anonymization and is not an access credential. Receipt read access uses a secret browser-held capability and expires 400 days after creation. Public receipts contain no client contacts or form answers. These rules also apply to standalone booking; see [first configuration](GETTING-STARTED.md#appointments).

## Integration boundary

The loader is an external classic script which creates a sandboxed iframe. No cross-origin public API CORS is enabled. The parent site must permit Turnesta in `script-src` and `frame-src` when it has CSP. A direct iframe without the loader is not supported. Test the actual top-level published page; an iframe-based CMS preview can have a different origin/ancestor chain.

Allowed origins include exact scheme/host/port, with no credentials, wildcard, arbitrary path, query or fragment. A final `/` is accepted. Up to ten origins are normalized, including IDNA host conversion and default ports. Bare and `www` hosts are different. HTTP is allowed only for localhost/127.0.0.1/[::1] when Turnesta itself has a loopback public origin. Same-origin embedding is refused: this integration requires a different origin, including during local tests.

The wrapper uses Shadow DOM and a constructable stylesheet to isolate its presentation. The host can set external width/margins. Older browsers can receive functional unstyled fallback markup. Custom themes/modal launchers are not implemented.

## Storage and visitor workflow

The iframe uses the same calendar/form/atomic confirmation/recovery as standalone booking; there is no alternate reservation path. Requests use `credentials: omit`; staff/third-party session cookies are unnecessary. Receipt/pending state belongs to iframe storage and can be partitioned by parent site or blocked by the browser.

Startup probes real read/write storage under a Web Lock with a three-second budget. Submission still requires preserving the request before POST. Without storage/Web Locks, submission is blocked and the standalone link is offered before sending. Switching pages does not transfer a pending request/capability. Recover an uncertain result in the same widget; after storage loss contact the business before making a duplicate. No Storage Access API or capability transfer between windows is implemented.

## CSP, revocation and messages

Only `/embed/book/{slug}` can receive a route-specific `frame-ancestors` exception through an internal typed response extension. The server checks the configured parent origin; supplying it in query is not authorization. Errors deny framing. HTML/API stay no-store/no-referrer; do not substitute shared CDN caching for tenant-specific CSP.

The iframe sandbox allows scripts, same-origin storage/API, forms and policy popups, without top navigation/camera/microphone/geolocation. Same-origin means the iframe's real Turnesta origin, not the business origin. Removing an allowed origin blocks new iframe loads; an already loaded document is not retroactively revoked by CSP. Pause public booking to stop new submissions; earlier receipt reads/recovery remain available.

The protocol uses version 1, a random per-instance UUID channel and the exact source window/origin. Iframe sends `turnesta:ready`; parent replies `turnesta:init`; iframe sends `turnesta:resize` with an integer height. Both reject extra fields/wrong channel/source/origin and use exact targetOrigin rather than `*`. The parent accepts heights 320..2400 px; larger content scrolls inside. Handshake repeats across load, listeners precede insertion, and cleanup removes listeners/observers. Ready/init do not wait for offscreen animation frames.

Contacts, form answers, capabilities, confirmation numbers and appointment assignments never appear in the parent message protocol. Resize is not a booking/CRM/analytics callback. Multiple instances have independent channels.

## Settings and verification

`GET/PUT /api/organizations/{tenant}/locations/{point}/bookings/widget` reads/updates allowed origins and returns publication/script URLs. PUT requires owner/manager, session, Origin, CSRF and expected version; version 0 creates settings. Its version is independent of public form settings. Unknown outcomes require a read before another edit. FORCE RLS/composite foreign keys and a narrow origin projection protect storage.

Playwright covers a cross-origin clinic page and booking in Chromium/Firefox/WebKit. Local unit tests cover protocol validation, bounds, storage probe and cleanup. These are not proof of Safari on a physical phone, a real CMS, every third-party storage policy or HTTPS/CDN behavior. See [browser tests](../e2e/README.md), [architecture](ARCHITECTURE.md#public-access-and-client-data) and [operations](OPERATIONS.md).

## Troubleshooting

| Symptom | Check |
| --- | --- |
| A link appears but the calendar does not load | Use the generated loader, a different saved origin and a top-level published page; check the site's script/frame CSP |
| Calendar loads but submission is unavailable | Confirm public booking is enabled, an offer is available, and iframe storage/Web Locks are permitted |
| Saved receipt history differs between website and standalone page | Browser storage can be partitioned by parent site; history/capabilities are not automatically transferred |
| Submission timed out or its outcome is unknown | Recover the original request in the same widget/browser before making a new booking; storage loss does not cancel the original |

The standalone link is a fallback before submission, but opening it after an uncertain widget submission does not recover that widget's pending request. Ask the business to check if the original browser state is no longer available.
