# server-api Specification

## Purpose
The server API is the single public contract every Cyber Code client uses: the TUI, `cyber exec`, IDE integrations, web, mobile, SDKs and remote Devices. It follows OpenCode's server-first design (one HTTP + SSE server for many projects, Location routing, OpenAPI, durable event replay from v2). It adds Codex/Claude Code-style background service management, idempotent writes, and Bearer authentication with a Cyber Account so the same surface can be reached through the Relay.

## Requirements

### Requirement: One server per user
(P0) The system SHALL run at most one long-lived `cyber` server per OS user and serve every Location from it. `cyber serve` SHALL run a server in the foreground until terminated, printing `cyber server listening on <url>`.

#### Scenario: Foreground serve
- **WHEN** the user runs `cyber serve`
- **THEN** the server binds its listener, prints `cyber server listening on http://127.0.0.1:<port>`, and runs until SIGINT or SIGTERM

#### Scenario: Many projects on one server
- **WHEN** two clients send requests for `/repo/a` and `/repo/b` to the same server
- **THEN** each request is handled by the Location services of its own directory without starting a second server

### Requirement: Background service management
(P0) The system SHALL provide `cyber service start|stop|restart|status`. `start` SHALL reuse a registered server whose `GET /api/v1/health` answers within 2 seconds with a matching version, and otherwise SHALL spawn a detached `cyber serve --register` and poll every 50 ms, up to 100 attempts, for a healthy registration. A registered server SHALL write `~/.local/state/cyber/server.json` (mode 0600) with `id`, `version`, `url`, `socket` and `pid`, and SHALL remove it on shutdown when the file still holds its own `id`.

#### Scenario: Reuse a healthy server
- **WHEN** `cyber service start` runs and `server.json` points to a server reporting the same version
- **THEN** no new process is spawned and the existing URL is printed

#### Scenario: Replace a stale server
- **WHEN** the registered server reports a different version or does not answer within 2 seconds
- **THEN** the stale server is stopped (SIGTERM, then SIGKILL after 5 seconds if the same registration is still healthy), a new server is spawned, and `server.json` is rewritten

#### Scenario: Status
- **WHEN** the user runs `cyber service status`
- **THEN** it prints `running <url> (v<version>)` for a healthy matching server, otherwise `stopped`, and removes a stale `server.json`

### Requirement: Listener defaults
(P0) The server SHALL bind `127.0.0.1` by default. It SHALL try port 4747 first and fall back to an OS-assigned free port. On macOS and Linux it SHALL also listen on the Unix domain socket `~/.local/state/cyber/cyber.sock` (mode 0600). `--hostname`, `--port` and `--socket` SHALL override these defaults, and `--no-tcp` SHALL disable the TCP listener.

#### Scenario: Port 4747 busy
- **WHEN** port 4747 is already in use and no `--port` is given
- **THEN** the server binds a random free port and records it in `server.json`

#### Scenario: Socket-only mode
- **WHEN** the server starts with `--no-tcp`
- **THEN** it accepts requests only on the Unix socket and `server.json` has no `url`

### Requirement: Local password authentication
(P0) The server SHALL keep a password in `~/.local/state/cyber/password` (mode 0600), generated on first use as 32 random bytes encoded base64url. It SHALL require HTTP Basic credentials (username `cyber`) or an `auth_token` query parameter carrying base64 `cyber:<password>` on every TCP route. Unix-socket requests from the same OS user SHALL be authenticated by peer credentials without a password. `cyber service password [value]` SHALL print or replace the password, restarting the server on replacement.

#### Scenario: Missing credentials
- **WHEN** a TCP request arrives without valid Basic credentials, `auth_token`, or Bearer token
- **THEN** the server responds `401` with `WWW-Authenticate: Basic realm="cyber"` and `{ "_tag": "UnauthorizedError" }`

#### Scenario: Same-user socket
- **WHEN** a process owned by the server's OS user connects over the Unix socket
- **THEN** the request is authorized without credentials

### Requirement: Cyber Account bearer authentication
(P3) The server SHALL accept `Authorization: Bearer <access token>` issued by CyberdyneAuth. It SHALL validate the signature against the JWKS published at the issuer's discovery document (never a hard-coded key or issuer), and SHALL check `exp`, `iss` (equal to the discovery issuer), `aud` (when the `cyber-cli` client opted into audiences), and that `sub` equals the account the server is logged into, or is an org member allowed by `policy`. Discovery and JWKS SHALL be cached for 10 minutes and refetched on an unknown `kid`.

#### Scenario: Token for another user
- **WHEN** a Bearer token is valid but its `sub` differs from the server's logged-in account and no org policy grants access
- **THEN** the server responds `403` `ForbiddenError`

#### Scenario: Key rotation
- **WHEN** a token carries a `kid` absent from the cached JWKS
- **THEN** the server refetches JWKS once and validates against the refreshed keys

### Requirement: Origin and CORS policy
(P0) The server SHALL allow requests whose `Origin` is absent, equals the request Host, starts with `http://localhost:` or `http://127.0.0.1:`, equals `tauri://localhost`, or is listed in `remote.cors_origins`. It SHALL answer CORS preflight only for those origins, with `Access-Control-Max-Age: 86400`. Requests from any other origin SHALL be rejected with `403`.

#### Scenario: Foreign web page
- **WHEN** a browser page at `https://evil.example` sends a request to the local server
- **THEN** the server responds `403` and grants no CORS headers

### Requirement: Location routing
(P0) Location-scoped routes SHALL resolve the Location from the `location[directory]` and `location[workspace]` query parameters, then from the `x-cyber-directory` (URI-decoded) and `x-cyber-workspace` headers, and otherwise from the server's working directory. Routes under `/api/v1/sessions/:sessionID/...` SHALL use the Location stored on the Session and ignore request input. Location-scoped responses SHALL be wrapped as `{ location: { directory, workspace?, project: { id, directory } }, data }`.

#### Scenario: Header routing
- **WHEN** a request to `GET /api/v1/agents` carries `x-cyber-directory: %2Frepo%2Fpkg`
- **THEN** agents for Location `/repo/pkg` are returned with that Location in the envelope

#### Scenario: Session-pinned
- **WHEN** a request to `/api/v1/sessions/ses_1/prompt` carries `x-cyber-directory: /other`
- **THEN** the request runs against the Location recorded on `ses_1`

### Requirement: Route groups
(P0) The server SHALL expose these route groups under `/api/v1`: `health`, `location`, `session`, `message`, `permission`, `question`, `agent`, `model`, `provider`, `tool`, `fs`, `pty`, `workflow`, `goal`, `loop`, `job`, `message-x` (cross-session messages, mounted at `/api/v1/messaging`), `remote`, `runner`, `mcp`, `skill`, `command`, `config`, `account` and `event`. Group names are OpenAPI tags and operation-ID prefixes (e.g. `session.prompt`). URL paths SHALL use the plural collection form (`/api/v1/sessions/{id}`, `/api/v1/permissions/requests`, `/api/v1/workflows/runs`, `/api/v1/channels`, `/api/v1/routines`, `/api/v1/runners`, `/api/v1/agents`), except for singletons and streams (`/api/v1/health`, `/api/v1/config`, `/api/v1/account`, `/api/v1/event`, `/api/v1/fs`, `/api/v1/messaging`, `/api/v1/ws`). A group whose capability belongs to a later phase SHALL respond `503` `ServiceUnavailableError` with `service` naming the group until it is implemented.

#### Scenario: Unimplemented group
- **WHEN** a client calls `GET /api/v1/runners` on a build without runner support
- **THEN** the server responds `503` with `{ "_tag": "ServiceUnavailableError", "service": "runner" }`

### Requirement: Session and prompt routes
(P0) The server SHALL provide `GET|POST /api/v1/sessions`, `GET|PATCH|DELETE /api/v1/sessions/:sessionID`, `POST .../fork`, `POST .../prompt`, `POST .../interrupt`, `POST .../agent`, `POST .../model`, `POST .../mode`, `POST .../compact`, `POST .../revert/stage|clear|commit`, `GET .../message`, `GET .../message/:messageID`, `GET .../context`, `GET .../history` and `GET .../event`. `POST .../prompt` SHALL accept `{ id?, parts, delivery?: "steer"|"queue"|"hold", resume?: boolean }`, durably admit the prompt, and respond `202` with `{ data: Admitted }`. It SHALL wake the Session's Drain unless `resume` is `false` or `delivery` is `hold`.

#### Scenario: Queue delivery
- **WHEN** a client posts a prompt with `delivery: "queue"` while the Session is running
- **THEN** the prompt is admitted and promoted only when the Session would otherwise become idle

#### Scenario: Hold delivery
- **WHEN** a prompt is admitted with `delivery: "hold"`
- **THEN** it is not promoted until a user approves it via `POST /api/v1/sessions/:sessionID/inbox/:messageID/release`

### Requirement: Idempotency keys
(P0) Every non-GET route SHALL accept an `Idempotency-Key` header of 1–128 printable characters. The server SHALL retain the key, request hash and response for 24 hours per authenticated principal. A retry with the same key and body SHALL return the original response with header `Idempotent-Replayed: true`, and a retry with the same key but a different body SHALL fail with `409` `ConflictError`.

#### Scenario: Network retry
- **WHEN** a client resends `POST /api/v1/sessions` with the same `Idempotency-Key` and body after a timeout
- **THEN** the same Session is returned and no second Session is created

### Requirement: Instance event stream
(P0) `GET /api/v1/event` SHALL return `text/event-stream`. Its first event SHALL be `server.connected`, followed by live events for the requested Location (or all Locations with `scope=all`). A `: heartbeat` comment SHALL be sent every 15 seconds. Each event SHALL carry `id`, `type`, `data`, optional `location`, and for durable events `durable: { aggregateID, seq, version }`. Responses SHALL set `Cache-Control: no-cache, no-transform` and `X-Accel-Buffering: no`.

#### Scenario: Heartbeat
- **WHEN** no events occur for 15 seconds
- **THEN** the stream emits a `: heartbeat` comment

### Requirement: Durable session replay
(P0) `GET /api/v1/sessions/:sessionID/event?after=<seq>` SHALL replay the Session's durable events with sequence greater than `after`, then tail newly committed events without gaps. It SHALL honor `Last-Event-ID` as an alternative to `after`. `GET .../history?after&limit` (limit 1–500, default 100) SHALL return `{ data, hasMore }`. Live-only deltas (text, reasoning and tool-input fragments) SHALL be excluded from replay.

#### Scenario: Reconnect without loss
- **WHEN** a client reconnects with `Last-Event-ID: 41` after events 42–45 were committed
- **THEN** events 42–45 are delivered first in order, followed by live events

### Requirement: WebSocket transport
(P0) The server SHALL offer `GET /api/v1/ws` as a WebSocket alternative carrying the same event envelopes and accepting JSON-RPC 2.0 requests that map 1:1 to OpenAPI operation IDs. Remote Devices connected through the Relay SHALL use this transport.

#### Scenario: RPC over WebSocket
- **WHEN** a client sends `{"jsonrpc":"2.0","id":1,"method":"v1.session.prompt","params":{...}}` over `/api/v1/ws`
- **THEN** the server performs the same operation as `POST /api/v1/sessions/:sessionID/prompt` and replies with the same body as `result`

### Requirement: PTY routes and tickets
(P0) The server SHALL manage terminals with `GET|POST /api/v1/pty` and `GET|PUT|DELETE /api/v1/pty/:ptyID`. `POST /api/v1/pty/:ptyID/connect-token` SHALL require header `x-cyber-ticket: 1` and an allowed Origin, and SHALL return a single-use ticket valid for 60 seconds bound to the PTY, directory and workspace. `GET /api/v1/pty/:ptyID/connect?ticket=` SHALL upgrade to a WebSocket that replays up to 2 MiB of buffered output from `cursor` before streaming live output.

#### Scenario: Reused ticket
- **WHEN** a ticket that was already used is presented again
- **THEN** the upgrade is refused with `401`

### Requirement: Tagged error model
(P0) Declared failures SHALL be JSON objects tagged by `_tag`, with these statuses: `InvalidRequestError` 400, `InvalidCursorError` 400, `UnauthorizedError` 401, `ForbiddenError` 403, `EntitlementRequiredError` 402, `NotFoundError` variants 404 (`SessionNotFoundError`, `MessageNotFoundError`, `RunNotFoundError`, …), `ConflictError` 409, `SessionBusyError` 409, `RateLimitedError` 429 with `retryAfterMs`, `UnknownError` 500 with a `ref` of the form `err_<id>` logged server-side, and `ServiceUnavailableError` 503.

#### Scenario: Unexpected failure
- **WHEN** a handler fails with an undeclared error
- **THEN** the response is `500` with `{ "_tag": "UnknownError", "message": "...", "ref": "err_..." }` and the same `ref` appears in the server log

### Requirement: Pagination
(P0) List routes SHALL return `{ data, cursor: { previous?, next? } }` with opaque cursors, `limit` 1–200 (default 50) and `order` `asc` or `desc` (default `desc`). A malformed or expired cursor SHALL fail with `InvalidCursorError`.

#### Scenario: Next page
- **WHEN** a client passes the returned `cursor.next` to the same list route
- **THEN** the next page in the same order is returned without duplicates

### Requirement: OpenAPI document and versioning
(P0) The server SHALL serve the OpenAPI 3.1 document at `GET /api/v1/openapi.json`, with operation IDs of the form `v1.<group>.<operation>` and WebSocket operations marked `x-websocket: true`. Changes within `/api/v1` SHALL be additive only (new routes, optional fields, new event types). A breaking change SHALL require `/api/v2` served in parallel for at least two minor releases.

#### Scenario: Additive field
- **WHEN** a new optional response field is introduced
- **THEN** it is added to `/api/v1` without changing existing fields or operation IDs

### Requirement: Remote rate limiting
(P3) Requests authenticated by Bearer token or arriving through the Relay SHALL be rate-limited per principal to 60 requests per 10 seconds for mutating routes and 600 for reads, by default (`remote.rate_limit`). Local password and Unix-socket requests SHALL be exempt.

#### Scenario: Burst from a Device
- **WHEN** a remote Device sends 61 prompts within 10 seconds
- **THEN** the 61st request receives `429` `RateLimitedError` with `retryAfterMs`

### Requirement: Embedded transport
(P0) The server SHALL be constructible in-process with the identical router and no listener, so the TUI, `cyber exec` and SDK embedded mode can send requests through an in-memory transport using base URL `http://cyber.internal`. Authentication SHALL be disabled for the in-process transport only.

#### Scenario: TUI without a port
- **WHEN** the TUI starts with `--embedded`
- **THEN** it talks to an in-process server and no TCP or socket listener is opened

### Requirement: mDNS discovery
(P0) When `--mdns` is set and the bind host is not loopback, the server SHALL publish a `_cyber._tcp` service named `cyber-<port>` and SHALL unpublish it on shutdown. With a loopback bind it SHALL log a warning and skip publishing.

#### Scenario: Loopback with mDNS
- **WHEN** `cyber serve --mdns` binds `127.0.0.1`
- **THEN** no service is published and a warning is logged

### Requirement: Raw API command
(P0) `cyber api <operationId | METHOD /path>` SHALL send one request to the registered server, starting it if needed, with stored credentials. It SHALL accept `-d/--data` (default content type `application/json`), repeatable `-H/--header` (up to 100) and `--param key=value`, write the response body to stdout, and exit 1 on a non-2xx status.

#### Scenario: Call by operation ID
- **WHEN** the user runs `cyber api v1.session.list --param limit=5`
- **THEN** the first five Sessions are printed as JSON

### Requirement: Health and version
(P0) `GET /api/v1/health` SHALL be unauthenticated and return `{ healthy: true, version, apiVersion: "v1", features: [...] }`. `features` SHALL list the enabled capability groups so clients can adapt to phased builds.

#### Scenario: Feature probe
- **WHEN** a mobile client calls `/api/v1/health`
- **THEN** it learns whether `workflow`, `goal` and `remote` are available before showing those screens

### Requirement: Response compression
(P0) The server SHALL gzip or zstd-encode compressible responses of at least 1024 bytes when `Accept-Encoding` allows it. It SHALL NOT compress SSE streams, WebSocket frames or responses that already have an encoding.

#### Scenario: Large list
- **WHEN** a client with `Accept-Encoding: zstd` fetches a 40 KB session list
- **THEN** the response is zstd-encoded with `Content-Encoding: zstd`
