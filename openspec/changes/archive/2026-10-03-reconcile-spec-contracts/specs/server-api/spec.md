## ADDED Requirements

### Requirement: Stdio transport
(P0) `cyber serve --stdio` SHALL speak JSON-RPC 2.0 over stdin and stdout, carrying the same operation IDs, request bodies and event envelopes as the WebSocket transport, for SDK embedded mode and editor hosts. It SHALL run as an embedded private server with no listener, on the database named by `CYBER_DB` or an in-memory database, and SHALL exit when stdin closes. Authentication SHALL be disabled on this transport because the parent process owns both pipes.

#### Scenario: SDK embedded mode
- **WHEN** the TypeScript SDK starts `cyber serve --stdio` and sends `{"jsonrpc":"2.0","id":1,"method":"v1.session.create","params":{...}}`
- **THEN** a Session is created in the private database and the reply carries the same body as `POST /api/v1/sessions`

## MODIFIED Requirements

### Requirement: Local password authentication
(P0) The server SHALL keep a password in `~/.local/state/cyber/password` (mode 0600), generated on first use as 32 random bytes encoded base64url. It SHALL require HTTP Basic credentials (username `cyber`) on every TCP route. An `auth_token` query parameter carrying base64 `cyber:<password>` SHALL be accepted only on `GET /api/v1/event`, `GET /api/v1/sessions/:sessionID/events` and PTY connect upgrades, where browsers cannot set headers; on any other route it SHALL be ignored, because query strings reach logs and histories. Unix-socket requests from the same OS user SHALL be authenticated by peer credentials without a password. `cyber service password [value]` SHALL print or replace the password, restarting the server on replacement.

#### Scenario: Missing credentials
- **WHEN** a TCP request arrives without valid Basic credentials, `auth_token` on a stream route, or Bearer token
- **THEN** the server responds `401` with `WWW-Authenticate: Basic realm="cyber"` and `{ "_tag": "UnauthorizedError" }`

#### Scenario: Same-user socket
- **WHEN** a process owned by the server's OS user connects over the Unix socket
- **THEN** the request is authorized without credentials

#### Scenario: Query token rejected on a mutating route
- **WHEN** `POST /api/v1/sessions?auth_token=...` arrives with no Basic or Bearer header
- **THEN** the server responds `401`

### Requirement: Route groups
(P0) The server SHALL expose exactly these route groups under `/api/v1`: `health`, `location`, `session`, `message`, `permission`, `question`, `agent`, `model`, `provider`, `credential`, `tool`, `fs`, `pty`, `workflow`, `goal`, `loop`, `job`, `messaging` (cross-session messages), `remote`, `runner`, `routine`, `channel`, `worktree`, `memory`, `lsp`, `formatter`, `usage`, `vcs`, `ide`, `policy`, `mcp`, `skill`, `command`, `hook`, `plugin`, `config`, `account`, `oauth` and `event`. Group names are OpenAPI tags and operation-ID prefixes (e.g. `v1.session.prompt`). This list is the single registry of first path segments; a capability SHALL NOT introduce a route outside these groups without adding the group here. URL paths SHALL use the plural collection form (`/api/v1/sessions/{id}`, `/api/v1/sessions/{id}/messages`, `/api/v1/permissions/requests`, `/api/v1/workflows/runs`, `/api/v1/channels`, `/api/v1/routines`, `/api/v1/runners`, `/api/v1/agents`, `/api/v1/worktrees`, `/api/v1/credentials`, `/api/v1/oauth-attempts`), except singletons and streams (`/api/v1/health`, `/api/v1/config`, `/api/v1/account`, `/api/v1/policy`, `/api/v1/vcs`, `/api/v1/usage`, `/api/v1/memory`, `/api/v1/lsp`, `/api/v1/formatters`, `/api/v1/ide`, `/api/v1/event`, `/api/v1/fs`, `/api/v1/messaging`, `/api/v1/ws`). A group whose capability belongs to a later phase SHALL respond `503` `ServiceUnavailableError` with `service` naming the group until it is implemented.

#### Scenario: Unimplemented group
- **WHEN** a client calls `GET /api/v1/runners` on a build without runner support
- **THEN** the server responds `503` with `{ "_tag": "ServiceUnavailableError", "service": "runner" }`

### Requirement: Session and prompt routes
(P0) The server SHALL provide `GET|POST /api/v1/sessions`, `GET|PATCH|DELETE /api/v1/sessions/:sessionID`, `POST .../fork`, `POST .../prompt`, `POST .../interrupt`, `POST .../agent`, `POST .../model`, `POST .../mode`, `POST .../compact`, `POST .../rewind`, `POST .../revert/stage|clear|commit`, `GET .../messages`, `GET .../messages/:messageID`, `GET .../inbox`, `PATCH|DELETE .../inbox/:messageID`, `POST .../inbox/:messageID/release`, `POST .../inbox/:messageID/drop`, `GET .../context`, `GET .../diff`, `GET .../history` and `GET .../events`. `POST .../prompt` SHALL accept `{ id?, parts, delivery?: "steer"|"queue"|"hold", resume?: boolean }`, durably admit the prompt, and respond `202` with `{ data: Admitted }`. It SHALL wake the Session's Drain unless `resume` is `false` or `delivery` is `hold`. The inbox release and drop routes SHALL be the only way to release or drop any held item, whatever its origin (user, cross-session message, channel event).

#### Scenario: Queue delivery
- **WHEN** a client posts a prompt with `delivery: "queue"` while the Session is running
- **THEN** the prompt is admitted and promoted only when the Session would otherwise become idle

#### Scenario: Hold delivery
- **WHEN** a prompt is admitted with `delivery: "hold"`
- **THEN** it is not promoted until a user approves it via `POST /api/v1/sessions/:sessionID/inbox/:messageID/release`

### Requirement: Durable session replay
(P0) `GET /api/v1/sessions/:sessionID/events?after=<seq>` SHALL replay the Session's durable events with sequence greater than `after`, then tail newly committed events without gaps. It SHALL honor `Last-Event-ID` as an alternative to `after`. `GET .../history?after&limit` (limit 1–500, default 100) SHALL return `{ data, hasMore }`. Live-only deltas (text, reasoning and tool-input fragments) SHALL be excluded from replay.

#### Scenario: Reconnect without loss
- **WHEN** a client reconnects with `Last-Event-ID: 41` after events 42–45 were committed
- **THEN** events 42–45 are delivered first in order, followed by live events

### Requirement: Embedded transport
(P0) The server SHALL be constructible in-process with the identical router and no listener, so `--embedded` clients and the Rust SDK can send requests through an in-memory transport using base URL `http://cyber.internal`. An embedded server SHALL use a private database and SHALL NOT open the shared database for writing. Authentication SHALL be disabled for the in-process transport only.

#### Scenario: TUI without a port
- **WHEN** the TUI starts with `--embedded` and an in-memory database
- **THEN** requests are served without a TCP or socket listener and the user's registered server is unaffected
