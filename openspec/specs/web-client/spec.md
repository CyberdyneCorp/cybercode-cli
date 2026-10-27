# web-client Specification

## Purpose
The web client is a browser UI for Cyber Code with the same surface as the TUI, served from the user's own server without an account (like OpenCode's `opencode web`), reused by the Relay for remote Devices, and wrapped by a desktop application. One bundle, three transports: local password, Relay channel, desktop shell. It exists so local-first users get a graphical client without any hosted service.

## Requirements

### Requirement: Local web client
(P1) The system SHALL bundle a browser client into the `cyber` binary and serve it from the local server at `GET /` (and `/app/*`). `cyber web [--hostname <host>] [--port <n>] [--no-open]` SHALL ensure the background server is running and open the client in the default browser, authenticated with the local password through the login page (never via a URL token). The web client SHALL offer the same surface as the TUI through the public API: session list and picker, composer with steer and queue, transcript with diffs, permission and question prompts, `/tasks`, workflow monitor, goal panel, model and mode pickers, and settings backed by `PATCH /api/v1/config`. No Cyber Account SHALL be required. Binding a non-loopback `--hostname` SHALL print a warning that the password grants owner access and SHALL require TLS (`server.tls.cert`/`key`); a plaintext `--insecure-lan` override SHALL NOT be offered. A client may instead reach a loopback HTTP listener through a user-managed SSH forward. Cyber SHALL NOT automatically create tunnels, expose router ports or require a Cyber Account for this transport. The first delivery slice SHALL include Session navigation/transcripts/diffs, steer/queue and interrupt, pending permissions/questions, and existing Workflow Run, Job and Goal status/control; the remaining TUI-equivalent surfaces remain subsequent P1 scope. An already running shared server SHALL NOT be rebound or weakened silently by `cyber web`.

#### Scenario: Open the web client
- **WHEN** the user runs `cyber web`
- **THEN** the browser opens `http://127.0.0.1:4747/`, asks for the local password once, and shows the Sessions of the current directory

#### Scenario: LAN access requires TLS
- **WHEN** the user runs `cyber web --hostname 0.0.0.0` without TLS configured
- **THEN** the command fails with `off-loopback web access requires global server.tls.cert and server.tls.key; use HTTPS or an SSH forward to loopback` and exit code 2

#### Scenario: Approve through a private forward
- **WHEN** the user opens a manually configured SSH-forwarded loopback URL and authenticates with the local password
- **THEN** they can inspect and answer existing pending requests without an account or Relay, and execution stays on the server machine

#### Scenario: Existing listener mismatch
- **WHEN** `cyber web --hostname` requests another bind than the registered server uses
- **THEN** the command refuses with an actionable restart instruction and changes no existing listener

### Requirement: Shared bundle for Relay and desktop
(P3) The Relay web client (`remote-control`) SHALL be the same bundle, loaded with the Relay transport (OIDC login, Device pairing, end-to-end encrypted channel) instead of the local password transport. Feature probes SHALL come from `GET /api/v1/health` in both cases, so the client hides screens its server does not implement.

#### Scenario: One bundle, two transports
- **WHEN** the same client build is served by a local server and by the Relay
- **THEN** both show identical screens for the features the connected server reports in `features`

### Requirement: Desktop application
(P4) The project SHALL ship a desktop application for macOS, Windows and Linux that wraps the web client in a Tauri shell (origin `tauri://localhost`, already allowed by `server-api`). It SHALL start or attach to the user's background server, register the `cyber://` URL scheme, show native notifications, support several windows on different Locations, and offer the same remote-attach flow as the web client. Native computer automation is owned by `computer-use` and does not require this desktop wrapper.

#### Scenario: Desktop attaches to the service
- **WHEN** the user opens the desktop application
- **THEN** it ensures the background server runs and lists Sessions without opening a browser

### Requirement: Browser login and credential handling
(P1) The local dashboard SHALL authenticate API requests with explicit Basic credentials for the existing local owner password, held only in page memory, using fetch with `credentials: "omit"` and authenticated fetch-based event streams. Login SHALL state that the password grants owner authority rather than a limited Device grant. Credentials SHALL NOT enter URLs, browser persistent/session storage, IndexedDB, service-worker caches, logs or rendered errors; logout, reload and authentication failure SHALL clear usable authentication state. Only the embedded static shell/assets may be public; API data SHALL stay authenticated and use `Cache-Control: no-store`. The client SHALL render transcript/diff/log content as inert data, disable raw HTML in Markdown and apply same-origin CSP with no third-party execution, framing or external connections.

#### Scenario: Reload after login
- **WHEN** an authenticated page is reloaded
- **THEN** it asks for the password again, with no credential recovered from browser storage or URL

#### Scenario: Hostile transcript
- **WHEN** a transcript contains scripts, HTML event handlers or an external image beacon
- **THEN** the dashboard executes none of them and sends no external request

### Requirement: Browser approval inbox and authority
(P1) On attach and reconnect the dashboard SHALL reconcile pending permissions and questions for the selected Location with live events by owner Session/request ID. Cards SHALL show the origin, tool/resource, command or diff, reason and saved-approval scope; replies SHALL use existing permission/question APIs and ceilings. "Always" SHALL require explicit scope review. A submission SHALL retain its idempotency key across uncertain delivery and remain pending until acknowledged. Offline/stale cards SHALL NOT submit. A request settled by another client SHALL be removed or marked answered and SHALL NOT transfer its decision to another request. A rejected headless ask SHALL remain rejected; browser attachment SHALL NOT resurrect it.

#### Scenario: Terminal answers first
- **WHEN** a terminal approves a request while a browser displays its card
- **THEN** the browser reconciles that request as answered and does not apply its pending click to a different request

#### Scenario: Lost reply acknowledgement
- **WHEN** the network drops after the server receives an approval but before the browser sees its receipt
- **THEN** the browser retains the same request/key, reconciles the result and never resubmits it with a fresh identity

### Requirement: Browser attachment and recovery state
(P1) The dashboard SHALL hold an interactive attachment lease defined by `server-api` while acting as an approver, renew every 15 seconds and visibly handle its 45-second expiry. It SHALL release only its own lease on logout or Session switch. After server restart it SHALL reauthenticate, reconcile state and obtain a new lease. Connection states SHALL distinguish connected, reconnecting, offline and authentication required; reconnect SHALL use exponential 500 ms–15 second backoff, resume durable event cursors and reload ephemeral pending/progress state. Losing the page SHALL NOT stop the server; expiry SHALL apply ordinary non-interactive permission behavior without widening authority.

#### Scenario: Browser disappears
- **WHEN** the last browser attachment stops renewing and no other interactive client remains
- **THEN** within 45 seconds its lease expires and pending/future asks follow the Session's non-interactive Mode policy rather than waiting indefinitely or being auto-approved

#### Scenario: Another client remains
- **WHEN** one browser's lease expires but the TUI remains interactively attached
- **THEN** the TUI remains an approver and its presence is not removed by that expiry

### Requirement: Responsive accessible dashboard
(P1) The first dashboard slice SHALL work at 360 CSS pixels with single-column navigation and a pending-approval count, and at desktop widths without hiding request authority or connection state. Commands and diffs SHALL scroll inside their cards. Controls SHALL have accessible names, visible keyboard focus and keyboard operation; status SHALL NOT rely on color alone. If `solve` evidence is available, the Workflow view SHALL distinguish its checks, candidate branches, recommendation and incomplete status, without offering automatic merge/publication. Unsupported routes/features SHALL show an unavailable state, not a fake successful control. P1 SHALL NOT claim Relay, native mobile apps or push notifications.

#### Scenario: Phone-sized approval
- **WHEN** a user opens the dashboard at 360 CSS pixels with a long command pending
- **THEN** they can read its Session/origin and command, inspect its scope, and submit a labelled decision without desktop-only controls

#### Scenario: Candidate report has no winner
- **WHEN** the server returns `solve` evidence with no recommended candidate
- **THEN** the dashboard shows the checks and reasons without labelling any branch the winner
