## MODIFIED Requirements

### Requirement: Local password authentication
(P0) The server SHALL keep a password in `~/.local/state/cyber/password` (mode 0600), generated on first use as 32 random bytes encoded base64url. It SHALL require HTTP Basic credentials (username `cyber`) on TCP and Unix-socket owner API routes except the unauthenticated health route defined by "Health and version". (P1) The embedded dashboard shell/assets at `/` and `/app/*` MAY be public, but SHALL contain no server/Session data or credentials; static fallback SHALL NOT intercept API routes or read arbitrary filesystem paths. No other authentication exemption is introduced by the web client. (P1) A valid local IDE token ("Local IDE token issuance") SHALL authorize the routes in its scope ("Local IDE token scope") instead of Basic credentials. It is the only other local credential and SHALL NOT authorize any other owner route. An `auth_token` query parameter carrying base64 `cyber:<password>` SHALL be accepted only on `GET /api/v1/event`, `GET /api/v1/sessions/:sessionID/events` and PTY connect upgrades, where browsers cannot set headers; on any other route it SHALL be ignored, because query strings reach logs and histories. Unix-socket requests SHALL additionally require peer credentials identifying the server OS user; peer credentials alone SHALL NOT authorize owner API access. A Unix listener without a configured owner password SHALL fail closed. Explicitly trusted in-process Embedded calls MAY bypass local password authentication. `cyber service password [value]` SHALL print or replace the password, restarting the server on replacement.

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

#### Scenario: IDE token is not an owner credential
- **WHEN** a request with a valid IDE token as `Authorization: Bearer` calls `GET /api/v1/config/language`
- **THEN** the server responds `403` `ForbiddenError` with `reason: "ide_token_scope"` and the route handler does not run


### Requirement: Prompt attachment parts
(P0) `POST /api/v1/sessions/:sessionID/prompt` SHALL accept `parts` of these types:

- `text { text }`
- `image { media_type, data, name? }` and `document { media_type, data, name? }`, where `data` is base64
- `file { path, name?, range? }`, read by the server. (P1) A file whose content is UTF-8 text and not a supported media type SHALL be admitted as a text part `<file path="…">…</file>` (with `lines="<start>-<end>"` for a range); other files follow the media handling below. `range` is `{ start_line, end_line }`, 1-based and inclusive, with `1 <= start_line <= end_line`; only those lines SHALL be admitted, after the same checks as the whole file. A range past the end of the file SHALL be clamped to the last line, and a malformed range, or a range on a file that is not text, SHALL be refused with `InvalidRequestError`.
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

#### Scenario: Ranged file part
- **WHEN** a local client posts `file { path: "src/app.ts", range: { start_line: 12, end_line: 30 } }`
- **THEN** the server applies the `read` tool's checks to `src/app.ts` and admits only lines 12 through 30

#### Scenario: Malformed range
- **WHEN** a client posts `file { path: "src/app.ts", range: { start_line: 30, end_line: 12 } }`
- **THEN** the response is `400` `InvalidRequestError` and nothing is read or admitted

## ADDED Requirements

### Requirement: Local IDE token issuance
(P1) The server SHALL issue local IDE tokens, a scoped and revocable local credential for editor clients (IDs with prefix `itk_`). The routes, in the `ide` group, SHALL authorize only owner Basic credentials:
- `POST /api/v1/ide/tokens` takes `{ client: "vscode", name?, expires_in_days? }`. `expires_in_days` defaults to 30 and SHALL be 1–90. The route returns `{ data: { id, token, client, name, created_at, expires_at } }`. The `token` value SHALL appear only in this response.
- `GET /api/v1/ide/tokens` lists `{ id, client, name, created_at, expires_at, last_used_at, revoked_at }` without token values or hashes.
- `DELETE /api/v1/ide/tokens/:tokenID` revokes a token and is idempotent.

A token SHALL be `cyi_` followed by 32 random bytes in base64url. The server SHALL store only its SHA-256 and the record above in the server database, and SHALL compare presented tokens in constant time. Each `client` SHALL have at most one active token: issuing a new one SHALL revoke the previous one in the same transaction. `client` values other than `vscode` SHALL be refused with `InvalidRequestError`.

`cyber ide token create --client vscode [--name <text>] [--expires <days>]` SHALL call the issuing route as the owner. It SHALL write the token atomically (a temporary file, then rename) to `<state>/ide/<client>.token`, the token on the first line followed by an `expires_at=<RFC 3339>` line, with mode 0600 inside a 0700 directory, and SHALL print only the `id` and expiry, never the token. `cyber ide token list` SHALL print the records. `cyber ide token revoke <id>` SHALL revoke the token and delete `<state>/ide/<client>.token` when that file holds the revoked token. `<state>/ide/` is part of the private state directory, so model tools and sandboxed children SHALL NOT read it ("Local password authentication").

#### Scenario: Issue a token
- **WHEN** the owner runs `cyber ide token create --client vscode`
- **THEN** `<state>/ide/vscode.token` exists with mode 0600 and holds a `cyi_` token expiring in 30 days, stdout shows only its `itk_` ID and expiry, and the database holds only the token's SHA-256

#### Scenario: Reissue rotates
- **WHEN** a `vscode` token is active and the owner creates another for `vscode`
- **THEN** the old token is revoked and its next request gets `401`, and the file holds the new token

#### Scenario: A token cannot mint tokens
- **WHEN** a request authenticated by an IDE token calls `POST /api/v1/ide/tokens`
- **THEN** the server responds `403` `ForbiddenError` with `reason: "ide_token_scope"` and issues nothing

#### Scenario: Listing hides secrets
- **WHEN** the owner calls `GET /api/v1/ide/tokens`
- **THEN** each record has its ID, client, name, timestamps and revocation time, and no token value or hash

### Requirement: Local IDE token scope
(P1) The server SHALL accept an IDE token only as `Authorization: Bearer cyi_…`, and only on the Unix socket or on the main TCP listener from a loopback peer address (`127.0.0.0/8` or `::1`). It SHALL answer `401` (`UnauthorizedError`, or the hub's `UnauthenticatedError` on the collaboration listener) when the token arrives:
- on the peer listener;
- on the collaboration listener;
- from any other peer address;
- in the `auth_token` query parameter.

A valid token SHALL authorize exactly these routes, plus the public health route:
- `GET /api/v1/service` and `GET /api/v1/models`
- `GET|POST /api/v1/sessions`, `GET /api/v1/sessions/:sessionID`, and these Session routes: `POST .../prompt`, `POST .../interrupt`, `POST .../mode`, `POST .../model`, `GET .../messages`, `GET .../events` and `GET .../diff`
- `POST /api/v1/sessions/:sessionID/interactive-clients` and `PUT|DELETE .../interactive-clients/:clientID`
- `GET /api/v1/permissions/requests` and `POST /api/v1/sessions/:sessionID/permissions/:requestID/reply`
- `GET /api/v1/questions/requests` and `POST /api/v1/sessions/:sessionID/questions/:requestID/reply`

Any other route, including the IDE token routes, `POST .../mode` with `bypass` or `dont-ask`, and `POST /api/v1/sessions` with `mode` `bypass` or `dont-ask`, a Session `rules` set, a `parent_id` or `unattended: true`, SHALL be refused with `403` `ForbiddenError` `{ reason: "ide_token_scope" }` before the handler runs. A Session created with an IDE token SHALL never run without human approval, whatever sets its Mode: its Mode SHALL be resolved at creation (the requested Mode, else the agent's `permission_mode`), and a creation that would resolve to `bypass` or `dont-ask` SHALL be refused the same way. An IDE token SHALL NOT prompt a Session whose selected, pending or running Mode is `bypass` or `dont-ask` (`403` `ide_token_scope`). A prompt is never interpreted as a slash command, and an `always` reply saves only the request's own `always_patterns`, as for a person approving in a client. Permission rules, Modes, sandbox, workspace trust and `file` part checks SHALL apply to an IDE token request exactly as to an owner request. The server SHALL log the token `id`, never its value, with every authorized request, and SHALL update `last_used_at` at most once per minute.

#### Scenario: Out-of-scope route
- **WHEN** an IDE token calls `POST /api/v1/providers/chatgpt/login`
- **THEN** the response is `403` with `reason: "ide_token_scope"` and no sign-in starts

#### Scenario: Bypass mode refused
- **WHEN** an IDE token calls `POST /api/v1/sessions/ses_abc/mode` with `bypass`
- **THEN** the response is `403` with `reason: "ide_token_scope"` and the Mode does not change

#### Scenario: Session create cannot remove approval
- **WHEN** an IDE token calls `POST /api/v1/sessions` with `mode: "bypass"`, with `rules: { "*": "allow" }`, with a `parent_id` or with `unattended: true`
- **THEN** the response is `403` with `reason: "ide_token_scope"` and no Session is created

#### Scenario: Agent default cannot make a token Session unattended
- **WHEN** the agent's `permission_mode` is `bypass` and an IDE token calls `POST /api/v1/sessions` without a `mode`
- **THEN** the response is `403` with `reason: "ide_token_scope"` and no Session is created, while the owner's same request still creates a `bypass` Session

#### Scenario: A token cannot drive an unattended Session
- **WHEN** the owner switched a Session to `dont-ask` and an IDE token posts a prompt to it
- **THEN** the response is `403` with `reason: "ide_token_scope"` and nothing is admitted

#### Scenario: Token from the LAN refused
- **WHEN** the server listens on `0.0.0.0` with TLS and a request from `192.168.1.20` carries a valid IDE token
- **THEN** the response is `401` `UnauthorizedError`

#### Scenario: Approval with an IDE token
- **WHEN** an IDE token replies `once` to a pending permission of a Session in its Location
- **THEN** the reply is applied as an owner reply would be, and the server log names the token's `itk_` ID

### Requirement: Local IDE token revocation and expiry
(P1) A revoked or expired IDE token SHALL get `401` `UnauthorizedError` on its next request. Revocation SHALL close that token's open event streams within 1 second and SHALL end the interactive presence leases it created. Replacing the owner password (`cyber service password <value>`) SHALL revoke every IDE token. A server restart SHALL NOT revoke tokens. Revoked and expired records SHALL stay listable for 30 days and then be deleted.

#### Scenario: Revocation closes streams
- **WHEN** the owner runs `cyber ide token revoke itk_1` while VS Code follows a Session's events with that token
- **THEN** the stream closes within 1 second, the lease ends, and the next request with the token gets `401`

#### Scenario: Expiry
- **WHEN** a token created with `--expires 1` is used 25 hours later
- **THEN** the server responds `401` and lists the token as expired

#### Scenario: Password replacement revokes IDE tokens
- **WHEN** the owner runs `cyber service password <new>`
- **THEN** every IDE token is revoked and the server restarts with the new password
