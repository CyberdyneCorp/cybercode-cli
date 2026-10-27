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
(P0) The system SHALL provide `cyber service start|stop|restart|status`. Every binary SHALL embed a build identity, and SHALL stamp its executable (canonical path, length and modification time) when it starts. `start` SHALL reuse a registered server whose `GET /api/v1/health` answers within 2 seconds with this client's version and build, and, when the registration names this client's executable path, with this client's executable stamp. A registered server of another version, build or stamp is stale. A stale server SHALL be stopped and replaced only when its registration names this client's executable path and the authenticated `GET /api/v1/service` reports its registration `id`, no running Session and no active workflow run; the client SHALL then print `restarted the Cyber server: it was running an older build` to stderr. A stale server that runs work, belongs to another executable path, or cannot report its work SHALL be kept and used, and the client SHALL print `The Cyber server runs an older build; restart it with `cyber service restart` when idle` to stderr. With no healthy registered server, `start` SHALL spawn a detached `cyber serve --register` and poll every 50 ms, up to 100 attempts, for a healthy registration. A registered server SHALL write `~/.local/state/cyber/server.json` (mode 0600) with `id`, `version`, `build`, `binary`, `binary_stamp`, `url`, `socket` and `pid`, and SHALL remove it on shutdown when the file still holds its own `id`. Every client that starts the service (TUI, `exec`, `api`, `mcp serve` and the resource commands) SHALL use this rule.

#### Scenario: Reuse a healthy server
- **WHEN** `cyber service start` runs and `server.json` points to a server reporting the same version and build
- **THEN** no new process is spawned and the existing URL is printed

#### Scenario: Replace a stale server
- **WHEN** `cyber` was rebuilt or upgraded, and the registered server started from the same executable path reports another build or stamp with no running Session or workflow run
- **THEN** the old server is stopped, a new server is spawned from the new binary, `server.json` is rewritten with the new build, and stderr says `restarted the Cyber server: it was running an older build`

#### Scenario: Keep a busy server of an older build
- **WHEN** the registered server of another build has a running Turn
- **THEN** it keeps running, the client uses it, and stderr says it runs an older build and to run `cyber service restart` when idle

#### Scenario: Another install's server
- **WHEN** the registered server of another build was started from another executable path, or predates `GET /api/v1/service`
- **THEN** it is never stopped by `start`; the client uses it and prints the same warning

#### Scenario: Status
- **WHEN** the user runs `cyber service status`
- **THEN** it reports the registration, whether it is healthy for this version, and whether it is `stale` for this build

### Requirement: Listener defaults
(P0) The server SHALL bind `127.0.0.1` by default. It SHALL try port 4747 first and fall back to an OS-assigned free port. On macOS and Linux it SHALL also listen on the Unix domain socket `~/.local/state/cyber/cyber.sock` (mode 0600). `--hostname`, `--port` and `--socket` SHALL override these defaults, and `--no-tcp` SHALL disable the TCP listener. An overridden Unix socket SHALL be an absolute path under the private state directory, with an existing non-symlink parent and no symlink leaf or path traversal. A non-socket existing leaf SHALL NOT be replaced. Unsafe legacy socket registrations SHALL NOT receive owner credentials.

#### Scenario: Port 4747 busy
- **WHEN** port 4747 is already in use and no `--port` is given
- **THEN** the server binds a random free port and records it in `server.json`

#### Scenario: Socket-only mode
- **WHEN** the server starts with `--no-tcp`
- **THEN** it accepts requests only on the Unix socket and `server.json` has no `url`

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
(P0) The server SHALL expose exactly these route groups under `/api/v1`: `health`, `location`, `session`, `message`, `permission`, `question`, `agent`, `model`, `provider`, `credential`, `tool`, `fs`, `pty`, `workflow`, `goal`, `loop`, `job`, `messaging` (cross-session messages), `remote`, `runner`, `routine`, `channel`, `worktree`, `memory`, `lsp`, `formatter`, `usage`, `vcs`, `service` (local service lifecycle), `system` (machine stats), `peer` (peer listener), `peers` (local peer management), `teams` (teams and definitions), `ide`, `policy`, `mcp`, `skill`, `command`, `hook`, `plugin`, `config`, `account`, `oauth`, `telemetry` (OpenTelemetry export status) and `event`. Group names are OpenAPI tags and operation-ID prefixes (e.g. `v1.session.prompt`). `/api/v1/files` belongs to `fs`; `/api/v1/team-definitions` belongs to `teams`; `/api/v1/peer/*` is served only on the peer listener. This list is the single registry of route groups; a capability SHALL NOT introduce a route outside these groups without adding the group here. URL paths SHALL use the plural collection form (`/api/v1/sessions/{id}`, `/api/v1/sessions/{id}/messages`, `/api/v1/permissions/requests`, `/api/v1/workflows/runs`, `/api/v1/channels`, `/api/v1/routines`, `/api/v1/runners`, `/api/v1/agents`, `/api/v1/worktrees`, `/api/v1/credentials`, `/api/v1/oauth-attempts`), except singletons and streams (`/api/v1/health`, `/api/v1/config`, `/api/v1/account`, `/api/v1/policy`, `/api/v1/vcs`, `/api/v1/system`, `/api/v1/usage`, `/api/v1/memory`, `/api/v1/lsp`, `/api/v1/formatters`, `/api/v1/ide`, `/api/v1/event`, `/api/v1/fs`, `/api/v1/messaging`, `/api/v1/telemetry`, `/api/v1/ws`). A group whose capability belongs to a later phase SHALL respond `503` `ServiceUnavailableError` with `service` naming the group until it is implemented.

#### Scenario: Unimplemented group
- **WHEN** a client calls `GET /api/v1/runners` on a build without runner support
- **THEN** the server responds `503` with `{ "_tag": "ServiceUnavailableError", "service": "runner" }`

#### Scenario: Telemetry status
- **WHEN** an owner client calls `GET /api/v1/telemetry` (`v1.telemetry.get`)
- **THEN** the server returns its OpenTelemetry export status (`observability-costs` → OpenTelemetry configuration), with every header value as `***`

### Requirement: Session and prompt routes
(P0) The server SHALL provide `GET|POST /api/v1/sessions`, `GET|PATCH|DELETE /api/v1/sessions/:sessionID`, `POST .../fork`, `POST .../prompt`, `POST .../interrupt`, `POST .../agent`, `POST .../model`, `POST .../mode`, `POST .../compact`, `POST .../rewind`, `POST .../revert/stage|clear|commit`, `GET .../messages`, `GET .../messages/:messageID`, `GET .../inbox`, `PATCH|DELETE .../inbox/:messageID`, `POST .../inbox/:messageID/release`, `POST .../inbox/:messageID/drop`, `GET .../context`, `GET .../diff`, `GET .../history` and `GET .../events`, and (P1) `GET|POST .../recap`. `POST .../prompt` SHALL accept `{ id?, parts, delivery?: "steer"|"queue"|"hold", resume?: boolean }`, durably admit the prompt, and respond `202` with `{ data: Admitted }`. It SHALL wake the Session's Drain unless `resume` is `false` or `delivery` is `hold`. The inbox release and drop routes SHALL be the only way to release or drop any held item, whatever its origin (user, cross-session message, channel event).

#### Scenario: Queue delivery
- **WHEN** a client posts a prompt with `delivery: "queue"` while the Session is running
- **THEN** the prompt is admitted and promoted only when the Session would otherwise become idle

#### Scenario: Hold delivery
- **WHEN** a prompt is admitted with `delivery: "hold"`
- **THEN** it is not promoted until a user approves it via `POST /api/v1/sessions/:sessionID/inbox/:messageID/release`

#### Scenario: Recap route
- **WHEN** a client calls `POST /api/v1/sessions/:sessionID/recap` on a Session with finished Turns
- **THEN** the response contains the recap text and the `through_seq` it covers

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
(P0) `GET /api/v1/sessions/:sessionID/events?after=<seq>` SHALL replay the Session's durable events with sequence greater than `after`, then tail newly committed events without gaps. It SHALL honor `Last-Event-ID` as an alternative to `after`. `GET .../history?after&limit` (limit 1–500, default 100) SHALL return `{ data, hasMore }`. Live-only deltas (text, reasoning and tool-input fragments) SHALL be excluded from replay.

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
(P0) Declared failures SHALL be JSON objects tagged by `_tag`, with these statuses: `InvalidRequestError` 400, `InvalidCursorError` 400, `UnauthorizedError` 401, `ForbiddenError` 403, `AccountRequiredError` 403, `EntitlementRequiredError` 402, `NotFoundError` variants 404 (`SessionNotFoundError`, `MessageNotFoundError`, `RunNotFoundError`, `AddressNotFoundError`, …), `ConflictError` 409, `SessionBusyError` 409, `AmbiguousAddressError` 409 with `candidates`, `MessageTooLargeError` 413 with `size` and `limit`, `AttachmentTooLargeError` 413 with `size` and `limit`, `UnsupportedAttachmentError` 415 with `media_type`, `ReviewTargetError` 422 with `code` (from `v1.command.run`, `vcs-integration` → Local review command), `UnsupportedModalityError` 422 with `model` and `modality` (`image` or `pdf`), `RateLimitedError` 429 with `retryAfterMs`, `UnknownError` 500 with a `ref` of the form `err_<id>` logged server-side, and `ServiceUnavailableError` 503.

#### Scenario: Unexpected failure
- **WHEN** a handler fails with an undeclared error
- **THEN** the response is `500` with `{ "_tag": "UnknownError", "message": "...", "ref": "err_..." }` and the same `ref` appears in the server log

#### Scenario: Ambiguous message address
- **WHEN** a client posts to `/api/v1/messaging/send` with `to: "migration"` and two Sessions have that name
- **THEN** the response is `409` with `_tag: "AmbiguousAddressError"` and `candidates` listing both Session IDs

#### Scenario: Oversized attachment
- **WHEN** a client posts a prompt with a 25 MiB PNG
- **THEN** the response is `413` with `_tag: "AttachmentTooLargeError"`, `limit` 20971520 and the message `Attachment exceeds 20 MiB`

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
(P0) The server SHALL be constructible in-process with the identical router and no listener, so `--embedded` clients and the Rust SDK can send requests through an in-memory transport using base URL `http://cyber.internal`. An embedded server SHALL use a private database and SHALL NOT open the shared database for writing. Authentication SHALL be disabled for the in-process transport only.

#### Scenario: TUI without a port
- **WHEN** the TUI starts with `--embedded` and an in-memory database
- **THEN** requests are served without a TCP or socket listener and the user's registered server is unaffected

### Requirement: mDNS discovery
(P0) When `--mdns` is set (or user config sets `peers.mdns`, P2), the server SHALL publish its peer listener as described in "Peer discovery" and SHALL unpublish it on shutdown; the main API SHALL never be published. When there is no peer listener or it is on loopback, the server SHALL log a warning and skip publishing.

#### Scenario: Loopback with mDNS
- **WHEN** `cyber serve --mdns` binds `127.0.0.1` and the peer listener is on loopback or not configured
- **THEN** no service is published and a warning is logged

### Requirement: Raw API command
(P0) `cyber api <operationId | METHOD /path>` SHALL send one request to the registered server, starting it if needed, with stored credentials. It SHALL accept `-d/--data` (default content type `application/json`), repeatable `-H/--header` (up to 100) and `--param key=value`, write the response body to stdout, and exit 1 on a non-2xx status.

#### Scenario: Call by operation ID
- **WHEN** the user runs `cyber api v1.session.list --param limit=5`
- **THEN** the first five Sessions are printed as JSON

### Requirement: Health and version
(P0) `GET /api/v1/health` SHALL be unauthenticated and return `{ healthy: true, version, build, apiVersion: "v1", features: [...] }`. `build` SHALL be the server binary's build identity. `features` SHALL list the enabled capability groups so clients can adapt to phased builds. The authenticated `GET /api/v1/service` SHALL return the listening server's `id`, `version`, `build`, `pid`, executable path, the number of Sessions running a Turn and the number of workflow runs queued, running or waiting for input; a transport that owns no listener SHALL answer `404 ServiceUnavailableError`.

#### Scenario: Feature probe
- **WHEN** a mobile client calls `/api/v1/health`
- **THEN** it learns whether `workflow`, `goal` and `remote` are available before showing those screens

#### Scenario: Build probe
- **WHEN** a client of another build calls `/api/v1/health`
- **THEN** the response's `build` differs from the client's, and `GET /api/v1/service` tells the client whether the server is running work

### Requirement: Response compression
(P0) The server SHALL gzip or zstd-encode compressible responses of at least 1024 bytes when `Accept-Encoding` allows it. It SHALL NOT compress SSE streams, WebSocket frames or responses that already have an encoding.

#### Scenario: Large list
- **WHEN** a client with `Accept-Encoding: zstd` fetches a 40 KB session list
- **THEN** the response is zstd-encoded with `Content-Encoding: zstd`

### Requirement: Stdio transport
(P0) `cyber serve --stdio` SHALL speak JSON-RPC 2.0 over stdin and stdout, carrying the same operation IDs, request bodies and event envelopes as the WebSocket transport, for SDK embedded mode and editor hosts. It SHALL run as an embedded private server with no listener, on the database named by `CYBER_DB` or an in-memory database, and SHALL exit when stdin closes. Authentication SHALL be disabled on this transport because the parent process owns both pipes.

#### Scenario: SDK embedded mode
- **WHEN** the TypeScript SDK starts `cyber serve --stdio` and sends `{"jsonrpc":"2.0","id":1,"method":"v1.session.create","params":{...}}`
- **THEN** a Session is created in the private database and the reply carries the same body as `POST /api/v1/sessions`

### Requirement: File browsing routes
(P1) The server SHALL provide, in the `fs` route group (`v1.fs.list`, `v1.fs.search`, `v1.fs.content`) and scoped to the request's Location, `GET /api/v1/files?path=<rel>&hidden=<bool>`, which returns one directory level as `entries[]` of `{name, path, kind: file|directory|symlink, size, git_status, ignored, target?, read: allow|ask|deny}`, plus `hidden` (dot and ignored entries left out), `changed` (the project's changed-file count) and `root` (the canonical project root that entry paths are relative to). It SHALL sort directories first and then by name, and SHALL cap results at 2,000 entries with `truncated: true`. A directory's `git_status` SHALL be `M` when any path below it changed. It SHALL also provide `GET /api/v1/files/search?directory=&query=&limit=` for fuzzy path search over non-ignored files, and `GET /api/v1/files/content?directory=&path=` returning `{text | binary: true, size, language, lines, truncated}`. These routes SHALL take `session_id` when present, so that the Session's permission rules and Mode apply; without it the Location's rules apply in `default` mode, and an `ask` SHALL be refused because no Session can be asked. Listing a level SHALL require `list` on it. A search SHALL require `list` on the project root and SHALL leave out every hit whose directory the rules do not allow listing without asking. Only a regular file or a directory SHALL be opened; any other kind (such as a FIFO or a device) SHALL be refused with `400` before it is opened, and no open SHALL wait. Content SHALL be served only when the `read` decision is `allow` (an `ask` from a client with a Session SHALL go through the normal permission request), and otherwise the route SHALL return `403 ForbiddenError {reason: permission, decision}`. Paths SHALL be resolved relative to the project root and canonicalized; any path or symlink target outside the project SHALL return `403 ForbiddenError {reason: outside_project}`. Content and listings SHALL be read from the file or directory the decision was made on; when the path names another object by the time it is read (for example a symlink or hard link swapped in during an `ask`), the route SHALL return `403 ForbiddenError {reason: changed}` without its content. Content SHALL be capped at 1 MiB.

#### Scenario: Listing a level
- **WHEN** a client lists `crates` in a project with a `.gitignore`d `target/`
- **THEN** the response has the crate directories with git status, and `target` appears only with `hidden=true`, marked `ignored`

#### Scenario: Path escape refused
- **WHEN** a client requests content for `../../etc/passwd` or a symlink pointing outside the project
- **THEN** the response is `403` with `reason: outside_project` and no bytes of the file

#### Scenario: Denied file
- **WHEN** the Session's rules deny `read` for `secrets/key.pem`
- **THEN** the listing shows it with `read: deny`, and the content route returns `403` with `reason: permission`

### Requirement: Peer listener
(P2) When user config sets `peers.listen`, the server SHALL open a second listener that serves only `/api/v1/peer/*` and answers `404` for every other path; the main API SHALL keep its own bind. A non-loopback peer listener SHALL use TLS with a self-signed ECDSA P-256 certificate the server generates on first use and keeps in `<state>/peer-tls/` (mode 0600); `tls: false` SHALL be refused unless the address is loopback. The fingerprint SHALL be the SHA-256 of the certificate's SubjectPublicKeyInfo and SHALL be recorded in `server.json`. Requests carrying an `Origin` header SHALL be refused with `403`. `peers` SHALL be accepted only from user config; project and local config setting it SHALL fail validation.

#### Scenario: Only peer routes are served
- **WHEN** a client with a valid peer token requests `GET /api/v1/sessions` on the peer listener
- **THEN** the response is `404`

#### Scenario: Plain listener off loopback
- **WHEN** user config sets `peers.listen: { address: "0.0.0.0:4748", tls: false }`
- **THEN** the server refuses to start the peer listener and logs why, and the main API still starts

#### Scenario: Browser request
- **WHEN** a request to the peer listener has `Origin: http://192.168.1.20:8080`
- **THEN** the response is `403`

### Requirement: Peer discovery
(P2) With `peers.mdns: true` (default `false`) and a non-loopback peer listener, the server SHALL publish `_cyber._tcp` named `cyber-<machine>` with TXT `role=peer`, `v=1`, `machine` and `fp`, and SHALL unpublish it on shutdown. `cyber peers discover [--timeout <duration>]` SHALL list discovered servers with machine, address, fingerprint and whether each is already a remote. Discovery SHALL NOT add a peer or grant trust; `cyber peers add` SHALL fail when a discovered fingerprint for the same address differs from the pairing code's.

#### Scenario: Discover a desktop
- **WHEN** `desk` publishes its peer listener and `laptop` runs `cyber peers discover`
- **THEN** `desk` is listed with its address and fingerprint and `not added`

### Requirement: Session branch routes
(P1) The server SHALL provide:

- `GET /api/v1/sessions/:sessionID/tree`, returning user-message nodes `{ id, parent_id, preview, created_at, labels, active, tip, descendants }`.
- `POST .../checkout` with `{ message_id?, label?, exact?: boolean, code?: boolean }`, returning the new active tip and, when `code` is true, the applied revert state.
- `PUT .../labels/:name` with `{ message_id }`, and `DELETE .../labels/:name`.

`GET .../messages` SHALL return only the active path unless `branches=all` is given, in which case every message SHALL include `parent_message_id`. Checkout on a busy Session SHALL fail with `409` and `SessionBusyError`. An unknown message or label SHALL fail with `404`.

#### Scenario: Linear view for existing clients
- **WHEN** a client that does not know about branches calls `GET .../messages` on a Session with two branches
- **THEN** it receives the active path in order, exactly as a linear conversation

#### Scenario: Checkout by label
- **WHEN** a client posts `{ "label": "fix-a" }` to `.../checkout` on an idle Session
- **THEN** the path ending at the labeled message's newest tip becomes active and the response names that tip

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

### Requirement: Prompt attachment parts
(P0) `POST /api/v1/sessions/:sessionID/prompt` SHALL accept `parts` of these types:

- `text { text }`
- `image { media_type, data, name? }` and `document { media_type, data, name? }`, where `data` is base64
- `file { path, name? }`, read by the server
- `attachment { sha256 }`, naming a blob this Session already stores

Any other part type SHALL be refused with `InvalidRequestError`. The route SHALL accept request bodies up to 64 MiB. Each prompt SHALL carry at most 20 media parts.

Media handling:

- **Before admission:** every media part SHALL be decoded, sniffed, size-checked, normalized and stored as `session-runtime` → Attachment normalization and `storage-events` → Attachment storage define. The admitted message SHALL hold `attachment { sha256, media_type, name, size }` references only.
- **`file` parts:** read through the `read` tool's checks (the agent profile's tools and a loaded skill's tool limits, `external_directory`, the `read` rule, hooks, hidden paths), with asks reaching the attached client. They SHALL be refused on the peer listener.
- **`attachment` parts:** a `sha256` that this Session does not store SHALL be refused with `InvalidRequestError`.
- **Message history:** `GET .../messages` SHALL return the references, not the bytes.
- **Digest:** the admission digest SHALL replace inline `data` with its SHA-256, so a retried request with the same bytes replays the same receipt.

The OpenAPI document SHALL describe the part union and `AttachmentRef`.

#### Scenario: Inline screenshot stored once
- **WHEN** a client posts a prompt with an 8 MiB base64 PNG `image` part
- **THEN** the prompt is admitted, the stored event holds an `attachment` reference without base64 data, and the model receives the image

#### Scenario: Unknown part type
- **WHEN** a client posts a prompt whose parts contain `{ "type": "tool_call", … }`
- **THEN** the response is `400` `InvalidRequestError` and nothing is admitted

#### Scenario: File part denied
- **WHEN** an exec Session in `dont-ask` mode receives `file { path: "/etc/shadow" }`
- **THEN** the response is a permission denial naming the rule or mode, and nothing is read or admitted

#### Scenario: Peer cannot send paths
- **WHEN** a paired Device posts a prompt with a `file` part through the peer listener
- **THEN** the request is refused with `InvalidRequestError` and no file is read

#### Scenario: Peer resubmits a stored blob
- **WHEN** a paired Device posts a prompt with an `attachment` part naming a blob the Session stores (a queued input taken back in `cyber attach`)
- **THEN** the part passes the peer listener and is handled as on the local API

### Requirement: Media on child and inbox edit routes
(P0) A subtask's or delegation's `attachments` and an edited queued input's `parts` (`PATCH .../inbox/:messageID`) SHALL take the prompt part union with the same handling, except that `file` parts SHALL be refused. These routes SHALL accept bodies up to 64 MiB. Their media SHALL suit the model of the Session that admits them (the child, or the edited Session), and a refused request SHALL remove the blobs it wrote.

#### Scenario: Delegated PDF stored by reference
- **WHEN** `cyber exec "@explore read it" --file spec.pdf` delegates to a child whose model takes PDFs
- **THEN** the child's events hold an `attachment` reference without base64 data; on a model without PDF input the request is refused and nothing is left behind

#### Scenario: Edited input with a mislabelled reference
- **WHEN** a client edits a queued input with `attachment { sha256, media_type: "application/pdf" }` naming a stored PNG
- **THEN** the edited input holds an `image/png` reference rebuilt from the blob

### Requirement: Output style routes
(P0) `POST /api/v1/sessions` SHALL accept an optional `output_style`. `POST /api/v1/sessions/:sessionID/output-style` SHALL accept `{ style: string | null }`, record the switch and respond with the Session. Both SHALL respond `400` `InvalidRequestError` with the reason for an invalid, unknown or held style, and for a Session with a parent. `GET /api/v1/config/output-styles` SHALL be Location-scoped and SHALL return:
- `styles`, each with `name`, `description`, `origin` (`builtin`, `user` or `project`), and `path` and `shadows` for files;
- `issues`, each with `path` and `message`;
- `configured`, the configured name and the layer that set it, or `null`;
- `effective`, the style a new primary Session there would use.

`PUT /api/v1/config/output-style` SHALL accept `{ style: string | null }` and set `output_style` in the user's global configuration. `null` SHALL set it to `default`. Only built-in and user styles SHALL be accepted, and every other byte of the document SHALL be kept. These operations SHALL be in the OpenAPI document.

#### Scenario: Listing reports a held project style
- **WHEN** a client calls `GET /api/v1/config/output-styles` for an untrusted checkout with `.cyber/output-styles/reviewer.md`
- **THEN** `styles` has the four built-ins and the user styles, and `issues` has the trust hold for the project directory

#### Scenario: Global default must not name a project style
- **WHEN** a client calls `PUT /api/v1/config/output-style` with `{ "style": "reviewer" }` and `reviewer` is defined only by a project
- **THEN** the server responds `400` and the global configuration is unchanged

### Requirement: Separate project collaboration API authority
(P3) Hub project/member/participant/message/task/event operations SHALL be served by a distinct collaboration listener and authorization state, documented with listener metadata in OpenAPI and generated SDK groups. They SHALL require validated account or scoped agent credentials and current project/visibility authorization for every request and stream; `x-cyber-directory`, local Session IDs and the local owner's password SHALL NOT authorize them. Local join/bind/publish/inbox integration SHALL remain owner-authenticated on the local API. The hub listener SHALL expose no local Session/tool/file/permission-approval routes. Mutations SHALL use stable operation IDs and expected entity revisions, return matching committed outcomes on retries and reject changed content with the same key. Defaults SHALL bound request bodies to 64 KiB, active connections to 128, requests to 120 per member per minute and 3,000 per project per minute; streams SHALL be bounded and reauthorized. Rate-limit replies SHALL include a retry delay. Unknown/inaccessible project resources SHALL reveal no metadata, while authorized role/revision/replay errors SHALL be typed.

#### Scenario: Access another project's cursor
- **WHEN** a member supplies a valid event cursor for a project it does not belong to
- **THEN** the hub refuses access without disclosing its events or snapshot

#### Scenario: Same operation with changed text
- **WHEN** a caller repeats a committed message operation ID with different text
- **THEN** the hub returns a conflict and does not replace or duplicate the original message

#### Scenario: Local API through the hub
- **WHEN** a hub member requests a Session prompt, shell execution or another member's permission approval through the hub listener
- **THEN** no such local execution route is exposed or authorized

#### Scenario: Revocation races a mutation
- **WHEN** a member was authorized before its removal but removal commits before its attempted task/message mutation
- **THEN** authorization is rechecked in the mutation transaction and no later unauthorized mutation or idempotent-result disclosure commits

### Requirement: Private owner socket paths
(P0) The owner listener SHALL accept custom Unix socket paths only as absolute, non-traversing paths under its protected state directory with an existing non-symlink parent and no symlink leaf. Existing non-socket files SHALL NOT be replaced. First-party clients SHALL reject unsafe legacy socket registrations before transmitting owner credentials. The protected state root SHALL refuse reads, writes and ancestor replacement from ordinary model tools and sandboxed children even when the root is inside an otherwise writable project.

#### Scenario: Model-writable socket path
- **WHEN** an owner configures a socket in a workspace or through a symlink outside protected state
- **THEN** startup fails before readiness or socket replacement, with a migration diagnostic

### Requirement: Explicit Session inbox wake
(P1) The authenticated server SHALL expose POST `/api/v1/sessions/{sessionID}/wake` and generated SDK `session.wake` to dispatch existing promotable inbox input without submitting a duplicate prompt. An idle child SHALL use exclusive execution ownership, verified checkout preparation and a schema-preserving fresh attempt. Held release SHALL atomically convert the existing row and reset its attempt only after preparation succeeds. A completed child without promotable input SHALL retain its result without new inference or checkout recreation.

#### Scenario: Deferred structured child prompt
- **WHEN** a completed structured child has an existing queued prompt admitted with `resume: false` and the client wakes it
- **THEN** the server starts a fresh owned attempt, preserves message identity and admission sequence and returns 204

#### Scenario: Held child input with busy result ownership
- **WHEN** a client releases a held child input while another tool or Job owns its terminal result
- **THEN** release is refused and both the held row and terminal result remain unchanged

### Requirement: Explicit subtree stop API
(P1) The authenticated server SHALL expose POST `/api/v1/sessions/{sessionID}/stop-subtree` and generated SDK `session.stopSubtree`. It SHALL return the runtime-owned report with original Session and scope identity, acknowledgement status, problems and receipt persistence. Unknown outcomes SHALL remain explicit successful report responses, never an empty success implying cancellation. The operation SHALL leave admission closed even after local acknowledgement, preserve inbox rows and restrict cancellation to the requested subtree. Ordinary Session interruption SHALL retain its background-child independence. Reopening and reviewed recovery remain separate required contracts.

#### Scenario: Scoped explicit stop
- **WHEN** a client explicitly stops a child subtree while an unrelated background Job runs
- **THEN** scoped Jobs settle, unrelated work continues and queued input remains durable
- **AND** the report retains the requested Session identity and admission remains closed

#### Scenario: Missing acknowledgement
- **WHEN** held result ownership prevents acknowledgement
- **THEN** the API and SDK return an Unknown report with problems and persistence evidence
- **AND** a new stop request may reassess the same closure after ownership settles without reopening admission

#### Scenario: Authentication and identity
- **WHEN** an unauthenticated caller requests subtree stop
- **THEN** authentication fails before closure or cancellation
- **AND** an authenticated request for a missing Session returns the tagged Session-not-found error

### Requirement: Reviewed subtree reopening API
(P1) Stop reports SHALL return their persisted receipt ID and owned sweep ID when recorded. The authenticated server SHALL expose POST `/api/v1/sessions/{sessionID}/reopen-subtree` and generated SDK `session.reopenSubtree`, taking the reviewed `scope_id` and `stop_receipt_id`. It SHALL perform matched durable/native verification, return the reopening receipt identity and leave queued input undispatched. Stale, Unknown or changed evidence SHALL return a conflict without opening admission. Idempotency replay SHALL preserve the original response without clearing a later close scope.

#### Scenario: Reviewed local stop
- **WHEN** a client submits the acknowledged stop report's scope and receipt to reopening
- **THEN** the server verifies current evidence and returns the original identities plus a reopening receipt

#### Scenario: Stale request after another stop
- **WHEN** an earlier reopening review is submitted after a new close scope exists
- **THEN** admission stays closed and the request is refused
- **AND** replaying the original idempotency key returns only the historical response
