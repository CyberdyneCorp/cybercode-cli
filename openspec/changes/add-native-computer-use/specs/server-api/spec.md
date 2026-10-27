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

### Requirement: Private owner socket paths
(P0) The owner listener SHALL accept custom Unix socket paths only as absolute, non-traversing paths under its protected state directory with an existing non-symlink parent and no symlink leaf. Existing non-socket files SHALL NOT be replaced. First-party clients SHALL reject unsafe legacy socket registrations before transmitting owner credentials. The protected state root SHALL refuse reads, writes and ancestor replacement from ordinary model tools and sandboxed children even when the root is inside an otherwise writable project.

#### Scenario: Model-writable socket path
- **WHEN** an owner configures a socket in a workspace or through a symlink outside protected state
- **THEN** startup fails before readiness or socket replacement, with a migration diagnostic
