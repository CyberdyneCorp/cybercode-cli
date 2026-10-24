## Context and delivery boundary

`web-client` already targets a bundled browser surface in P1, and `remote-control` targets account-authenticated paired Devices over an encrypted Relay in P3. This proposal turns P1 into an executable delivery plan. "Away from the terminal" means the user can reach their running server over a private TLS endpoint or an SSH forward they set up; it does not mean a sleeping computer continues executing or a public Relay exists.

The first slice has Session navigation/transcripts/diffs, steer/queue prompt submission, interrupt, a pending-request inbox and existing workflow/Job/Goal views and controls. Full settings, PTY, attachments, branch editing and all other TUI surfaces remain the eventual existing target and follow this slice. Do not offer unsupported controls: probe server features and handle unavailable routes explicitly.

## Decisions

### 1. Browser bundle and boundaries

Build a TypeScript browser client as static artifacts embedded in the Rust distribution. Reuse browser-safe generated API types and operation definitions; keep rendering, state/orchestration and HTTP/stream adapters separate. No Node runtime is required on the user's machine. Pin frontend build dependencies and check reproducible bundle generation in CI.

Only the embedded HTML/assets at `/` and `/app/*` are public. A static-route fallback never intercepts `/api/*`, reads arbitrary disk paths or includes server state. Session text, diffs and logs are inert escaped data; Markdown raw HTML is disabled. Use a same-origin CSP with no third-party scripts, no framing, no external connections, `base-uri 'none'`, `form-action 'self'` and no evaluation of transcript content. Browser assets contain no credentials or account information. Data responses use `Cache-Control: no-store`.

### 2. Login and transport

The login form asks for the existing local password and states that it grants owner access to this server, not a limited Device grant. Every API fetch supplies explicit HTTP Basic `cyber:<password>` and `credentials: "omit"` and `redirect: "error"`; do not use cookies, ambient browser Basic caching, local/session storage, IndexedDB, service-worker caches or URL/query tokens. Hold the password only in page memory, erase it on logout, and require login after page reload. Do not log headers or echo the password in errors. Failed login has a bounded retry/backoff in the UI; server authentication remains authoritative.

Use fetch/ReadableStream SSE with explicit headers rather than EventSource query credentials. Bind the client to its configured origin and Location, retain durable replay cursors and refetch live state after reconnect. PTY/WebSocket support is not part of the first slice. A later transport must preserve the credential rules instead of placing a password in a WebSocket URL.

### 3. Private remote access and CLI

`cyber web` opens the registered loopback server by default. `--no-open` prints its URL without credentials. `--hostname`/`--port` select an explicit listener; if an existing registered server has another bind, fail with an actionable restart instruction instead of rebinding or weakening it. A non-loopback listener requires configured TLS and a valid certificate for the user-facing hostname. No `--insecure-lan` override is offered for the new web client. An SSH forward may present a loopback HTTP URL because SSH supplies network transport security; no tunnel is created by Cyber.

Preserve existing Host/Origin checks; authenticate every data route regardless of the browser shell. Configure any needed CORS origin explicitly; do not expand to wildcard origins. The peer listener remains `/api/v1/peer/*` only and never serves the dashboard or accepts the owner password. Do not treat a private-network browser as a P3 paired Device: account step-up, Device revocation and Relay scope constraints remain separate.

Owner TLS reads only the merged defaults/global `server` section, then validates that owned section. Unrelated invalid optional peer settings retain their existing listener-isolation behavior; malformed owner TLS configuration fails closed. Project/profile/request settings never select the transport identity.

### 4. Interactive presence

Add Session routes `POST /api/v1/sessions/:sessionID/interactive-clients`, `PUT` and `DELETE /api/v1/sessions/:sessionID/interactive-clients/:clientID` for owner-authenticated attachment leases. POST returns `{ client_id, session_id, expires_at }`; PUT renews only that live lease; DELETE is idempotent. Every write uses ordinary idempotency keys. POST retries return their original receipt; an expired receipt does not recreate presence. Unknown/expired lease renewal returns 404.

A browser renews every 15 seconds and a lease expires after 45 seconds without renewal. Store only the authority binding and expiry, never credentials. A lease is bound to its Session/Location and can answer requests routed there, including eligible descendant requests. On logout/Session switch release only this browser's lease. On restart all old leases expire; the browser must authenticate, reconcile state and create a fresh lease before presenting itself as connected. A lease registers human presence, never permission grants. Browser data streams identify themselves with `X-Cyber-Client: web` and do not count as independent interactive attachments. A no-secret durable Session marker preserves the lease-managed policy across restart without restoring live presence; never-browser-attached Sessions keep the existing TUI/SDK contract. Each live runtime accepts at most 128 leases and each Session at most 16. Expiry settles pending permission and question asks as `Unattended`, without the rejection/dismissal halt cascade.

The server applies existing Non-interactive behavior when the last interactive attachment disappears: outstanding/future asks are handled under the Session's Mode and existing rejection/feedback rules, without automatic approval or cancellation of an unrelated executing tool. Existing TUI/SDK presence is not replaced by this browser's expiry. A browser cannot turn a previously rejected headless Workflow ask into a pending request; the UI explains the terminal outcome instead.

The Session event log retains a no-secret marker that a Session uses explicit browser-presence accounting. This marker survives restart so a previously attached Session cannot regain permanent legacy interactive authority merely because its leases disappeared. Lease IDs, deadlines and approvals remain in memory and are never restored. Existing Sessions never attached by a browser keep the legacy TUI/SDK behavior; a matching live native stream continues to count alongside browser leases.

### 5. Pending requests and races

Load current pending permissions and questions for the selected Location on attach/reconnect, then subscribe to live events; reconcile any race by request ID. Show owner Session, child origin, tool/resource, command/diff and reason. Permission actions use the current server contract: approve once, approve always where allowed, reject-and-stop without feedback, or reject with feedback. Display the saved-approval scope before "always"; respect remote/peer refusals and never invent a session-wide permission setting.

Capture the exact Session/request ID and immutable displayed payload for a response. Disable duplicate clicks while the request is in flight and retain its idempotency key across transport uncertainty. Only a server acknowledgement marks it applied. If another client settles it, refetch/remove the card and show "already answered"; never attach the old decision to another request or replay uncertain approvals with fresh keys. Questions support the server's choice/multiple/free-text rules and dismissal.

### 6. Reconnect and small screens

Connection state is visible in the page header: connected, reconnecting, offline, authentication required. Offline/stale cards cannot submit. Retry with 500 ms to 15 s exponential backoff, resuming durable events by sequence and reloading ephemeral pending/usage/progress state; a server restart also invalidates leases. Auth failure erases usable credentials and returns to login. Never imply that closing the page stops the server or its running work.

At 360 CSS pixels, show a single-column Session view and an Approval inbox tab with a pending count; diffs/long commands scroll within their cards. Every control has an accessible name, visible focus, keyboard operation and readable connection/request status. Do not depend on color alone. Destructive or broad saved approvals require explicit review of the command/scope, rather than a notification quick action. No push notification delivery is promised in P1.

### 7. Delivery order

1. Bundle/public shell, explicit auth, secure listener/SSH-forward instructions and read-only Session/status views.
2. Leases, composer and pending permissions/questions, including races, disconnect and expiry.
3. Existing workflow/Job/Goal controls and optional `solve` report evidence when available.
4. Real browser acceptance at desktop/mobile sizes, native build assets and user docs.

## Validation and risks

Test production server routes with fake time for lease expiry, two real browser contexts plus a terminal client, a scripted provider and real pending tool requests. Cover lost acknowledgements, duplicate/stale replies, malicious transcript Markdown, denied origins, unauthenticated data access, source/destination Location switches, TLS refusal, a local SSH-style forward and server restart. Use browser automation for keyboard and 360/1280-width flows, plus an actual second device for the private-TLS acceptance scenario.

The owner password grants broad authority: network access must remain deliberate and HTTPS-protected off loopback. Keeping credentials in page memory means re-login on reload. Background browser suspension can expire a lease and invoke non-interactive handling; that is shown on reconnect. P3 paired Devices will provide different identity and revocation semantics through the same UI bundle.
