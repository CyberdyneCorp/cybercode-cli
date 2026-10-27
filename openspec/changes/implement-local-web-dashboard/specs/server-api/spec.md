## MODIFIED Requirements

### Requirement: Local password authentication
(P0) The server SHALL keep a password in `~/.local/state/cyber/password` (mode 0600), generated on first use as 32 random bytes encoded base64url. It SHALL require HTTP Basic credentials (username `cyber`) on TCP and Unix-socket owner API routes except the unauthenticated health route defined by "Health and version". (P1) The embedded dashboard shell/assets at `/` and `/app/*` MAY be public, but SHALL contain no server/Session data or credentials; static fallback SHALL NOT intercept API routes or read arbitrary filesystem paths. No other authentication exemption is introduced by the web client. An `auth_token` query parameter carrying base64 `cyber:<password>` SHALL be accepted only on `GET /api/v1/event`, `GET /api/v1/sessions/:sessionID/events` and PTY connect upgrades, where browsers cannot set headers; on any other route it SHALL be ignored, because query strings reach logs and histories. Unix-socket requests SHALL additionally require peer credentials identifying the server OS user; peer credentials alone SHALL NOT authorize owner API access. A Unix listener without a configured owner password SHALL fail closed. Explicitly trusted in-process Embedded calls MAY bypass local password authentication. `cyber service password [value]` SHALL print or replace the password, restarting the server on replacement.

#### Scenario: Missing credentials
- **WHEN** a TCP or same-user Unix request arrives without valid Basic credentials or an allowed stream-route `auth_token`
- **THEN** the server responds `401` with `WWW-Authenticate: Basic realm="cyber"` and `{ "_tag": "UnauthorizedError" }`

#### Scenario: Same-user socket
- **WHEN** a process owned by the server's OS user connects over the Unix socket
- **THEN** the request without the owner credential is rejected with `401`; the same request with valid owner Basic credentials is authorized

#### Scenario: Query token rejected on a mutating route
- **WHEN** `POST /api/v1/sessions?auth_token=...` arrives with no Basic header
- **THEN** the server responds `401`

#### Scenario: Owner consent cannot be forged by model processes
- **WHEN** a same-user process connects through the default or an overridden Unix socket without the owner credential and attempts to create interactive presence, review or allow native control
- **THEN** each request is rejected before any presence or control authority changes

#### Scenario: Private owner authentication storage
- **WHEN** a model tool or sandboxed child attempts to read the owner's state directory, including a password replaced after bootstrap
- **THEN** the read is denied even with ordinary read permission or an explicit readable-path grant; owner credentials and their exact environment-value aliases are not inherited through environment allow/set overrides

## ADDED Requirements

### Requirement: Interactive browser attachment leases
(P1) Owner-authenticated `POST /api/v1/sessions/:sessionID/interactive-clients` SHALL create an interactive attachment and return `{ client_id, session_id, expires_at }`. `PUT /api/v1/sessions/:sessionID/interactive-clients/:clientID` SHALL renew only that live attachment and `DELETE` SHALL idempotently release it. Ordinary authentication, Location/Session routing and idempotency SHALL apply. A lease SHALL expire 45 seconds after its last renewal, be renewed by clients every 15 seconds and be invalid after server restart. An expired create receipt SHALL NOT recreate presence on replay; renewing an unknown/expired lease SHALL return 404. A lease SHALL record authority/expiry without credentials, never grant tool permissions, and never clear another client's attachment. When the last interactive attachment disappears, the server SHALL settle pending/future asks under the existing Non-interactive behavior of `permissions-modes`, never approve automatically or cancel unrelated executing tools. Existing descendant-request routing SHALL preserve the child owner when a parent dashboard answers.

#### Scenario: Wrong Session renewal
- **WHEN** a browser tries to renew its lease through another Session or Location
- **THEN** the renewal is refused and neither Session's interactive authority changes

#### Scenario: Idempotent create after expiry
- **WHEN** a POST receipt is retried with its original key after the lease expired
- **THEN** the original expired receipt is returned without registering a new interactive client

#### Scenario: Restart invalidates presence
- **WHEN** the server restarts while a browser had a live lease
- **THEN** the old lease cannot authorize interactive presence and the browser must authenticate and attach again

#### Scenario: Child approval routing
- **WHEN** a child worktree Session routes a pending permission to a parent with a live browser lease
- **THEN** the card identifies the child and its reply targets that child's exact request under the existing parent-routing rules
