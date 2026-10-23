# mcp-server Specification

## Purpose
MCP serve exposes Cyber Code Sessions to external MCP clients (Claude Code, Codex, another Cyber, IDEs), so they can create and drive Sessions, exchange cross-session messages and split work by driving several Sessions. MCP clients are trusted local clients of the server API: the tools are a thin mapping over existing routes, with the same authentication, idempotency, errors and events, and approvals stay with human clients.

## Requirements

### Requirement: MCP serve transports
(P1) `cyber mcp serve [--transport stdio|http] [--client <label>]` SHALL expose Cyber Code as an MCP server over the shared local server; the default transport SHALL be `stdio` and `--port` SHALL NOT be accepted. The served endpoint SHALL be Streamable HTTP at `/api/v1/mcp/serve` on the existing listener (TCP and Unix socket), inside the `mcp` route group:
- `POST` SHALL carry one JSON-RPC message and follow the `2025-06-18` Streamable HTTP rules: a notification or response SHALL get `202 Accepted` with no body; a request (sent with `Accept: application/json, text/event-stream`) SHALL get `application/json` or, for a call that waits, `text/event-stream` with an SSE `: keep-alive` comment at least every 15 s. `GET` SHALL answer `405`, because the server sends no unsolicited messages. `DELETE` with `Mcp-Session-Id` SHALL end that MCP session.
- `initialize` SHALL mint an `Mcp-Session-Id` (one **MCP connection**). MCP sessions SHALL be in memory and SHALL end with a server restart. Later requests SHALL carry the id; a missing id SHALL get `400` `InvalidRequestError` `{ reason: "mcp_session_required" }` and an unknown id, or one with no request and no open response stream for 30 minutes, SHALL get `404` `NotFoundError` `{ reason: "mcp_session" }`. Streams SHALL NOT be resumable: a lost response is recovered through the prompt receipt ("Prompt admission over MCP").
- With `stdio`, the process SHALL be a bridge that never opens a database and reads only the global configuration: it SHALL connect to the registered server the way `cyber api` does (Unix socket with peer credentials when the registered socket answers, else `127.0.0.1` with the local password), starting it with `cyber service start` semantics when none is running, and SHALL forward each JSON-RPC frame from stdin to the endpoint and each response and notification to stdout. Stdout SHALL carry only MCP frames; logs SHALL go to stderr and `<data>/log/mcp-serve.log`.
- The bridge SHALL send `X-Cyber-MCP-Client: <label>` when `--client` is given (label `^[a-z0-9][a-z0-9_-]{0,47}$`, else exit 2), and the recursion markers of "MCP recursion guard". Without `--client`, the label SHALL be `clientInfo.name` lowercased, with characters outside `[a-z0-9_-]` mapped to `-` and truncated to 48 characters (fallback `mcp-client` when the result is empty or does not match the label pattern). The endpoint SHALL fix an MCP session's label on `initialize`: a valid `X-Cyber-MCP-Client`, else `clientInfo.name` by the same rule, so HTTP clients are labelled too. The label is attribution only, never an identity.
- When the server becomes unreachable, the bridge SHALL fail in-flight requests with JSON-RPC error -32010 `data: { _tag, reason?, httpStatus?, session_id?, message_id? }` and retry with backoff from 0.5 s to 5 s for up to 30 s, then exit 1. When a request gets `404` for its MCP session (for example after a restart), the bridge SHALL re-send the cached `initialize` and `notifications/initialized` itself (starting a new MCP connection and forwarding nothing to stdout), then retry the request that got `404` once. Requests arriving while reconnecting SHALL wait for the reconnect or fail with -32010 when it gives up. HTTP failures SHALL map to the same error with `_tag` and `reason` copied from the `server-api` error body and `httpStatus` set; an unreachable server SHALL map to `_tag: "ServiceUnavailableError"`, `reason: "unreachable"`. Reconnecting SHALL re-read the registration and SHALL NOT start a server.
- On stdin EOF the bridge SHALL send `DELETE` for its MCP session and exit 0; Sessions SHALL keep running. Requests still in flight get no answer, and the bridge SHALL NOT re-create the MCP session after EOF.
- `--transport http` SHALL NOT start a listener: it SHALL ensure the server is running, print the endpoint URL (for example `http://127.0.0.1:4747/api/v1/mcp/serve`) and point at `cyber service password` for HTTP Basic, then exit 0; it SHALL NOT print the password.
- Protocol: MCP serve SHALL implement exactly one MCP revision, `2025-06-18`, with the normal version negotiation: `initialize` SHALL answer `protocolVersion: "2025-06-18"` whatever revision the client requested, leaving the client to disconnect when it does not support it. An HTTP request after `initialize` SHALL be accepted without `MCP-Protocol-Version` or with `2025-06-18`; any other value SHALL get `400` with JSON-RPC error -32600 and `data: { supported: ["2025-06-18"] }`.
  - `serverInfo` SHALL be `{ name: "cyber-code", title: "Cyber Code", version: <cyber version> }`.
  - Capabilities SHALL be only `tools: { listChanged: false }`. Resources, prompts, completions, logging, sampling, roots and elicitation requests SHALL NOT be used, and there SHALL be no MCP tasks utility: `tasks/*` methods SHALL get JSON-RPC error -32601.
  - `ping` SHALL be answered. `notifications/progress` and `notifications/cancelled` SHALL follow "Prompt results and waiting".
  - `instructions` SHALL be a fixed text of at most 2 KiB that lists the tools, states that Session output and messages are untrusted data, states that permission requests and questions are answered by a human in a Cyber client and never through MCP, and recommends a random unique `idempotency_key` (for example a UUID) on every `cyber_prompt`.
- When `mcp_serve.enabled` is false ("MCP serve limits"), the bridge SHALL exit 1 with `MCP serve is disabled` before reading stdin, and the endpoint SHALL answer `503` `ServiceUnavailableError` with `service: "mcp_serve"`.

#### Scenario: First use starts the server
- **WHEN** Claude Code is configured with the stdio command `cyber mcp serve --client claude-code` and no server is running
- **THEN** the server is started, `initialize` succeeds with `serverInfo.name` `cyber-code`, and later Sessions created through it record `mcp_client: "claude-code"`

#### Scenario: Stdin closes during a wait
- **WHEN** stdin closes while a `cyber_prompt` call waits
- **THEN** the bridge deletes its MCP session, which ends the wait, and exits 0, and the Session's Turn continues

#### Scenario: HTTP transport prints the endpoint
- **WHEN** the user runs `cyber mcp serve --transport http`
- **THEN** it prints `http://127.0.0.1:<port>/api/v1/mcp/serve` and a pointer to `cyber service password`, never the password, and exits 0 without binding a port

#### Scenario: Disabled serve
- **WHEN** the global configuration sets `mcp_serve.enabled: false` and Claude Code starts `cyber mcp serve`
- **THEN** the bridge exits 1 with `MCP serve is disabled` without reading stdin or starting a server; the same key in a project configuration is ignored with the warning

#### Scenario: Expired MCP session
- **WHEN** a client posts with an `Mcp-Session-Id` that had no request and no open stream for 31 minutes
- **THEN** the endpoint answers `404` and the client must initialize again

#### Scenario: Bridge survives a server restart
- **WHEN** the server restarts while the stdio bridge is connected and the client then calls `cyber_session_list`
- **THEN** calls in flight during the restart fail with -32010, the bridge re-initializes on `404` without writing a second `InitializeResult` to stdout, and the retried `cyber_session_list` call succeeds

#### Scenario: Newer client negotiates down
- **WHEN** a client initializes with `protocolVersion: "2025-11-25"`
- **THEN** the result has `protocolVersion: "2025-06-18"` and capabilities `{ tools: { listChanged: false } }` only

#### Scenario: Unsupported version header
- **WHEN** a request after `initialize` carries `MCP-Protocol-Version: 2024-11-05`
- **THEN** the endpoint answers `400` with JSON-RPC error -32600 and `data.supported` `["2025-06-18"]`

#### Scenario: No tasks utility
- **WHEN** a client sends `tasks/list`
- **THEN** it receives JSON-RPC error -32601 and nothing is executed

### Requirement: Local client trust model
(P1) An MCP client SHALL be an authenticated local client of the server API with the same trust as the TUI and the TypeScript SDK. MCP serve SHALL add no principals, grants, tokens or privilege model: every served tool SHALL be a mapping onto existing server API routes and SHALL be able to do only what those routes allow any local client, plus the narrower Mode ceiling, limits and recursion guard of this capability.
- Authentication SHALL be the `server-api` local authentication: Unix-socket peer credentials of the server's OS user, or HTTP Basic with the local password on TCP. A TCP request without valid credentials SHALL get `401` `UnauthorizedError` with `WWW-Authenticate: Basic realm="cyber"`; `auth_token` query parameters SHALL NOT be accepted.
- The endpoint SHALL accept only requests arriving directly on the Unix socket or on TCP from a loopback peer (`127.0.0.0/8`, `::1`) with local credentials, including when `cyber serve --hostname` binds another address; other TCP peers, Bearer-authenticated, Relay and `/api/v1/ws` requests SHALL get `403` `ForbiddenError` `{ reason: "loopback_only" }` until "Remote MCP serve access".
- A TCP request whose `Host` is not `localhost`, a `127.0.0.0/8` address or `[::1]` (with an optional port) SHALL get `403` `ForbiddenError` `{ reason: "host_not_allowed" }`, and `Origin` SHALL follow the `server-api` Origin and CORS policy.
- Sessions SHALL stay shared: Sessions created through MCP SHALL be ordinary Sessions that appear in the TUI, `cyber sessions` and the SDK, and MCP clients MAY read and drive any Session of the server as an SDK client can.

#### Scenario: Non-loopback peer refused
- **WHEN** the server runs with `--hostname 0.0.0.0` and a LAN host posts valid Basic credentials to `/api/v1/mcp/serve`
- **THEN** the endpoint answers `403` `ForbiddenError` `{ reason: "loopback_only" }` and no MCP session is created

#### Scenario: Authentication missing
- **WHEN** a TCP request to `/api/v1/mcp/serve` from `127.0.0.1` carries no credentials
- **THEN** the endpoint answers `401` `UnauthorizedError` with `WWW-Authenticate: Basic realm="cyber"`

#### Scenario: DNS rebinding refused
- **WHEN** a loopback request carries `Host: attacker.example:4747` and valid credentials
- **THEN** the endpoint answers `403` `ForbiddenError` `{ reason: "host_not_allowed" }`

#### Scenario: Same-user socket
- **WHEN** the stdio bridge connects over the Unix socket as the server's OS user
- **THEN** its requests are authenticated without a password

### Requirement: Served tools and errors
(P1) MCP serve SHALL offer exactly these tools, sorted by name, each with an `inputSchema` of `type: "object"` and `additionalProperties: false`, an `outputSchema`, and annotations `(readOnlyHint, destructiveHint, idempotentHint, openWorldHint)`:
- `cyber_session_list`, `cyber_session_read`, `cyber_session_transcript`, `cyber_prompt_wait`, `cyber_message_list`, `cyber_message_history`: `(true, false, true, false)`
- `cyber_session_create`, `cyber_message_send`: `(false, false, false, false)`
- `cyber_prompt`: `(false, true, false, true)`
- `cyber_interrupt`: `(false, true, true, false)`

The tools SHALL NOT be registered in the `builtin-tools` catalog or offered to Cyber's own agents except through a user-configured MCP server. A success SHALL return `structuredContent` conforming to the `outputSchema`, and `content[0]` as its serialized JSON. Results and notifications carrying model, tool or message text SHALL set `_meta["dev.cyber-code/untrusted"]: true`.

Errors:
- A failure inside a dispatched `tools/call` SHALL return `isError: true` with no `structuredContent` and `content[0]` as the JSON text `{ _tag, message, reason?, ...fields }`, using the `server-api` tagged error of the underlying route unchanged (`SessionNotFoundError`, `MessageNotFoundError`, `ConflictError`, `SessionBusyError`, `InvalidCursorError`, `UnknownError` with `ref`, and the messaging `AmbiguousAddressError` and `MessageTooLargeError`), plus the reasons this capability defines: `InvalidRequestError` with `schema` (and `path`), `too_large`, `exclusive_fields`, `session_settings_on_existing` or `not_a_directory`; `ForbiddenError` with `mode_ceiling`, `session_quota`, `depth_exceeded` or `self_target`; `RateLimitedError` with `concurrency`; `ConflictError` with `idempotency_mismatch`.
- An unknown tool name SHALL be JSON-RPC error -32602; malformed JSON-RPC SHALL be -32700 or -32600. Arguments that fail schema validation SHALL be a tool error so the calling model can correct them. Byte-size limits SHALL NOT be in `inputSchema`; oversized arguments SHALL fail with `too_large` or `MessageTooLargeError`.
- Transport failures decided before dispatch (authentication, `loopback_only`, `host_not_allowed`, disabled, missing or unknown MCP session, and a request body above 1 MiB as `400` `InvalidRequestError` `{ reason: "too_large" }`), except the protocol-version check, SHALL be HTTP responses with `server-api` bodies.

Idempotency: `cyber_session_create`, `cyber_prompt` and `cyber_message_send` SHALL accept `idempotency_key` (1..64 printable ASCII characters without spaces, as the `server-api` key allows; anything else is a `schema` error at `/idempotency_key`) and SHALL send it on each underlying write as the `server-api` `Idempotency-Key` `mcp:<tool>:<idempotency_key>`, except that `cyber_prompt` SHALL always key its prompt write `mcp:cyber_prompt:<idempotency_key>:prompt` and its Session creation `...:session`, and SHALL look up the stored `:prompt` response before any write; keys are shared by all local clients, so `idempotency_key` SHOULD be a random unique value (for example a UUID). Replay SHALL apply to each underlying write separately: a write whose key already has a stored response SHALL be replayed before any check of this capability except schema and `max_waits`, and is never re-checked or re-counted; a write without a stored response SHALL run the normal checks. A replay SHALL return the original result with `replayed: true` and no new effect. A key reused with a different call (`session_id`, `directory`, `title`, `agent`, `model`, `mode`, `prompt`, `delivery`, `to`, `text`) SHALL fail with `ConflictError` `{ reason: "idempotency_mismatch" }`, decided on a fingerprint of those arguments that MCP serve stores with the key before its first write (a stored response without a matching fingerprint is a mismatch); `wait` and `wait_seconds` SHALL never cause a conflict, and any server-generated prompt `id` SHALL be derived from the key. Retention SHALL be the `server-api` 24 hours.

#### Scenario: Schema violation is a tool error
- **WHEN** `cyber_session_list` receives `limit: 500`
- **THEN** it returns `isError` with the text content `InvalidRequestError` `{ reason: "schema", path: "/limit" }` and no `structuredContent`

#### Scenario: Unknown session
- **WHEN** `cyber_session_read` receives `session_id: "ses_missing"`
- **THEN** it returns `isError` with `SessionNotFoundError` in the text content

#### Scenario: Unknown tool
- **WHEN** a client calls tool `cyber_nope`
- **THEN** the server answers JSON-RPC error -32602

#### Scenario: Idempotent retry
- **WHEN** a client calls `cyber_session_create` twice with `idempotency_key: "k1"` and the same arguments
- **THEN** both calls return the same `session_id`, the second with `replayed: true`, and only one Session exists

#### Scenario: Key reused with different arguments
- **WHEN** a client reuses `idempotency_key: "k1"` on `cyber_session_create` with another `directory`
- **THEN** it returns `isError` with `ConflictError` `{ reason: "idempotency_mismatch" }` in the text content

### Requirement: Session creation over MCP
(P1) `cyber_session_create` SHALL accept `{ directory: absolute path, title?: string ≤ 200, agent?: string, model?: "provider/model[#variant]", mode?: Mode, idempotency_key? }`, create the Session through `POST /api/v1/sessions` at that Location, and return `{ session: SessionSummary, effective_mode: Mode, replayed?: boolean }`. A `directory` that is relative, missing or not a directory SHALL fail with `InvalidRequestError` `{ reason: "not_a_directory" }`. MCP serve SHALL keep `mcp_client` (the connection's label) for the Session in its own records keyed by `session_id`; it is otherwise an ordinary Session under the Location's configuration, workspace trust and sandbox, except that it SHALL carry the `exec-mode` "Never blocking on interaction" Session rules (deny `question`, `plan_enter` and `plan_exit`), so it never waits on a human answer.

Mode ceiling: Modes SHALL be ranked, least to most autonomous, `plan` < `dont-ask` < `default` < `accept-edits` < `auto` < `bypass`. The ceiling SHALL be `mcp_serve.max_mode`, which defaults to the `mode` of the user's global configuration (`default` when unset), and SHALL never reach `bypass` (a `bypass` default gives the ceiling `auto`).
- The Mode the server would resolve for the Session (explicit `mode`, else the agent's `permission_mode`, else the Location default) SHALL be refused with `ForbiddenError` `{ reason: "mode_ceiling", ceiling }` when above the ceiling and it came from an explicit `mode` or `agent` argument, and otherwise clamped to the ceiling; `bypass` SHALL never result.
- The agent SHALL be the `agent` argument, else the Location's `default_agent`, else `build`; the Location default Mode SHALL be the Location's configured `mode` (after workspace trust gating), else `default`. The resulting agent and the bounded Mode SHALL be passed explicitly to `POST /api/v1/sessions`, so the created Session has exactly the predicted Mode; an unknown predicted Mode SHALL count as above the ceiling.
- No served tool SHALL change a Session's Mode, agent, model or permission rules after creation.

`SessionSummary` SHALL be `{ session_id, title, directory, parent_id?, agent, model, mode, state: "idle"|"running"|"waiting_for_approval", mcp_client?, mcp_depth, created_at, updated_at }`, where `waiting_for_approval` means running with at least one pending permission or question request.

#### Scenario: Mode above the ceiling
- **WHEN** the user's global `mode` is unset and a client creates a Session with `mode: "auto"`
- **THEN** the call fails with `ForbiddenError` `{ reason: "mode_ceiling", ceiling: "default" }` and no Session is created

#### Scenario: Bypass always refused
- **WHEN** global config sets `mcp_serve.max_mode: "auto"` and a client requests `mode: "bypass"`
- **THEN** the call fails with `mode_ceiling`

#### Scenario: Project default clamped
- **WHEN** a project config sets `mode: "accept-edits"`, the ceiling is `default`, and a client creates a Session there without `mode`
- **THEN** the Session runs in `default` and `effective_mode` is `default`

#### Scenario: Default agent clamped
- **WHEN** the Location `default_agent` has `permission_mode: auto`, the ceiling is `default`, and a client creates a Session there without `mode` or `agent`
- **THEN** the Session runs in `default` and `effective_mode` is `default`

#### Scenario: No questions in MCP-created Sessions
- **WHEN** the agent of a Session created through MCP tries to call `question`
- **THEN** the tool is not offered to the model and no pending question is created

#### Scenario: Explicit agent above the ceiling
- **WHEN** the ceiling is `default` and a client creates a Session with `agent` naming an agent whose `permission_mode` is `auto`
- **THEN** the call fails with `ForbiddenError` `{ reason: "mode_ceiling", ceiling: "default" }` and no Session is created

#### Scenario: Safer Mode accepted
- **WHEN** the ceiling is `default` and a client creates a Session with `mode: "dont-ask"`
- **THEN** the Session is created with `effective_mode: "dont-ask"`

### Requirement: Session reading over MCP
(P1) The reading tools SHALL map onto the `server-api` session routes and SHALL see every Session of the server:
- `cyber_session_list` SHALL accept `{ directory?: absolute path, mcp_only?: boolean = false, state?: "idle"|"running"|"waiting_for_approval", cursor?, limit?: 1..200 = 50, max_bytes?: 1024..196608 = 65536 }`, ordered by `updated_at` descending, and return `{ sessions: SessionSummary[], next_cursor?, truncated: boolean }`, bounded by `max_bytes` with the `cyber_session_transcript` page rule (an item is cut by shortening its `title`, then its `directory`, with the marker). `mcp_only` SHALL keep Sessions created through MCP serve. Child Sessions SHALL be listed with their `parent_id`; archived Sessions SHALL NOT be listed. A malformed or expired cursor SHALL fail with `InvalidCursorError`.
- `cyber_session_read` SHALL accept `{ session_id }` and return `{ session: SessionSummary, last_turn?: { finished_at, stop_reason, final_text_preview (≤ 2 KiB) }, usage: Usage, pending: PendingRequest[] (≤ 20), inbox_pending: integer, recent_prompts: [{ message_id, admitted_at, status }] (the 20 newest prompts admitted through MCP serve) }`.
- `cyber_session_transcript` SHALL accept `{ session_id, cursor?, from?: "start"|"end" = "start", limit?: 1..100 = 20, include_tool_output?: boolean = false, max_bytes?: 1024..196608 = 65536 }` and return `{ items: [{ seq, message_id, role: "user"|"assistant"|"tool"|"notice", origin?, created_at, text?, tool?: { name, call_id, status, input_summary (≤ 512 characters), output? } }], next_cursor?, truncated: boolean }` from durable history only, never live deltas. `seq` SHALL be the durable event sequence that recorded the item, and an assistant entry SHALL give its text item followed by one `tool` item per call. Items in a page SHALL be in ascending `seq`; `from` SHALL apply only without a cursor, and after a first page from the end `next_cursor` SHALL continue toward older items. Each `text` and `output` SHALL be capped at 16 KiB with the marker `[… truncated N bytes]`. A page SHALL stop before its serialized `items` exceed `max_bytes`, and while items remain SHALL hold at least one item, cut further with the marker when needed, setting `truncated: true`.

`Usage` SHALL be `{ input_tokens, output_tokens, reasoning_tokens, cache_read_tokens, cache_write_tokens, cost_usd }` from the `observability-costs` token classes, with `cost_usd: null` when any counted step is unpriced. `PendingRequest` SHALL be `{ request_id, kind: "permission"|"question", session_id, summary (≤ 512 characters), created_at }`. Times in results (`created_at`, `updated_at`, `finished_at`, `admitted_at`) SHALL be Unix milliseconds.

#### Scenario: Paging a long transcript
- **WHEN** a Session has 300 small items and a client requests `limit: 100`
- **THEN** the result has `next_cursor`, and following it twice returns the remaining 200 items without duplicates

#### Scenario: Transcript size limit
- **WHEN** a client requests `max_bytes: 1024` and the next item has 16 KiB of `text`
- **THEN** the page holds that one item with `text` cut to fit and ending in the truncation marker, `truncated` is true and `next_cursor` points past it

#### Scenario: Reading the end of a transcript
- **WHEN** a client requests `from: "end"` and `limit: 2` on an idle Session whose only Turn was one prompt and one answer
- **THEN** the page holds the prompt and then the answer in ascending `seq`, with no `next_cursor`

#### Scenario: Pending request visible
- **WHEN** a client reads a Session blocked on a `bash` permission request answered by nobody yet
- **THEN** the result has `state: "waiting_for_approval"` and one `pending` entry with `kind: "permission"`

### Requirement: Prompt admission over MCP
(P1) `cyber_prompt` SHALL accept `{ session_id?, directory?: absolute path, prompt: non-empty string, delivery?: "steer"|"queue" = "steer", title?, agent?, model?, mode?, idempotency_key?, wait?: boolean = true, wait_seconds?: 1..3600 = 50 }`. In both prompt tools `wait_seconds` SHALL be clamped to `mcp_serve.limits.max_wait_seconds` at run time.
- Exactly one of `session_id` and `directory` SHALL be given, else `InvalidRequestError` `{ reason: "exclusive_fields" }`. `directory` SHALL create a new Session exactly as `cyber_session_create` (with `title`, `agent`, `model`, `mode`) and then prompt it; with `session_id`, any of those four fields SHALL fail with `InvalidRequestError` `{ reason: "session_settings_on_existing" }`.
- A prompt above 131072 bytes SHALL fail with `InvalidRequestError` `{ reason: "too_large" }`.
- Admission SHALL be `POST /api/v1/sessions/:sessionID/prompt` with `{ parts: [{ type: "text", text: prompt }], delivery }`, the same public user-admission path the SDK uses (so `UserPromptSubmit` hooks run and a hook deny returns that route's error), with `source: "user"` and no new origin kind. `prompt` SHALL be admitted as literal text and SHALL NOT be resolved as a slash command, so built-in, plugin, user and project commands and MCP prompts never run from `cyber_prompt`. MCP serve SHALL keep `mcp_client: <label>` per admitted `message_id` in its own records, which usage tagging reads.
- Receipt: after durable admission, the first progress notification SHALL carry the `message_id`, and `cyber_session_read` SHALL list it in `recent_prompts`. A client that loses the call SHALL re-attach with `cyber_prompt_wait` or by retrying with the same `idempotency_key`, which SHALL NOT admit again and SHALL wait on the original prompt with `replayed: true`.
- **MCP input** is a prompt or message admitted through MCP serve. Usage of runs started by MCP input SHALL be attributed with origin `mcp` and `mcp_client` (`cross-session-messaging` "Usage attribution").
- `cyber_prompt` SHALL also apply the Mode ceiling when it creates a Session ("Session creation over MCP"), the limits ("MCP serve limits") and the recursion guard ("MCP recursion guard").

#### Scenario: Another agent drives Cyber Code
- **WHEN** a client calls `cyber_prompt` with `{ directory: "/repo", prompt: "fix the failing test" }`
- **THEN** a new Session is created in `/repo`, the prompt runs, and the result is `completed` with `final_text`

#### Scenario: Exact retry after a dropped connection
- **WHEN** a client retries `cyber_prompt` with the same `idempotency_key` and arguments after its connection dropped
- **THEN** the result has `replayed: true`, the prompt appears once in history and the call waits for the same run

#### Scenario: Recovery retry without waiting
- **WHEN** a client retries `cyber_prompt` with the same `idempotency_key` and prompt but `wait: false`
- **THEN** the call replays without a conflict and returns the receipt with `replayed: true`

#### Scenario: Settings on an existing Session
- **WHEN** a client calls `cyber_prompt` with `session_id` and `mode: "plan"`
- **THEN** the call fails with `InvalidRequestError` `{ reason: "session_settings_on_existing" }`

#### Scenario: Oversized prompt
- **WHEN** a client sends a 200 KiB `prompt`
- **THEN** the call fails with `InvalidRequestError` `{ reason: "too_large" }` and nothing is admitted

### Requirement: Prompt results and waiting
(P1) `cyber_prompt` and `cyber_prompt_wait` SHALL return `PromptResult` `{ session_id, message_id, status, done: boolean, stop_reason?: "completed"|"error"|"budget_exceeded"|"interrupted", final_text? (≤ 64 KiB, head kept), truncated?, usage?: Usage, turns?, pending?: PendingRequest[], error?: { _tag, message }, replayed?, admitted_at, finished_at? }`.
- `status` SHALL be `queued` (admitted, not promoted), `running`, `waiting_for_approval` (running with a pending permission or question request, listed in `pending`), or one of the terminal values `completed`, `failed`, `interrupted`, `budget_exceeded` (from the run's `stop_reason`, with `error` → `failed`) and `refused` (the row left the inbox without promotion, with `error._tag: "ConflictError"`). `done` SHALL be true exactly for terminal values.
- A run SHALL be complete when the prompt was promoted, the Session became idle, and every background Job started after the promotion has settled and its handback Turn has gone idle, following the `exec-mode` owned-work rules restricted to those Jobs. `final_text` SHALL be the last assistant text after the promotion.
- With `wait: false`, `cyber_prompt` SHALL return right after admission with the current status (`queued` or `running`). Otherwise the call SHALL wait until the first of: the run is done; `wait_seconds` elapsed (default 50, below common 60 s client request timeouts); or this run has a pending permission or question request raised after the wait began, returning `waiting_for_approval`. Reaching `wait_seconds` SHALL NOT be an error: the result carries the non-terminal status and the receipt.
- `cyber_prompt_wait` SHALL accept `{ session_id, message_id, wait_seconds?: 0..3600 = 50 }`, derive the result from durable events (also after a server restart), and poll without waiting when `wait_seconds` is 0. An unknown Session SHALL fail with `SessionNotFoundError` and an unknown message with `MessageNotFoundError`.
- Progress: while a call carrying `_meta.progressToken` waits, the server SHALL send `notifications/progress` `{ progressToken, progress, message }` on its stream, with `progress` strictly increasing, at least every 15 s and at most 4 per second, none after the result. `message` SHALL be one status line of at most 200 characters. Each notification SHALL carry `_meta["dev.cyber-code/progress"]` `{ v: 1, kind: "admitted"|"promoted"|"tool_started"|"tool_finished"|"waiting_for_approval"|"turn_finished"|"keep_alive", message_id, tool?, request_id? }`; within `v: 1` kinds and fields SHALL only be added.
- Cancellation SHALL never abort or roll back a write: a `cyber_prompt` cancelled before admission SHALL finish creating and admitting, then stop waiting. `notifications/cancelled` for a waiting `cyber_prompt` or `cyber_prompt_wait`, a client closing the call's HTTP stream, `DELETE` or expiry of the MCP session, and the bridge's stdin EOF SHALL end the wait, send no result and free the call's wait slot, and SHALL leave the Session, its Drain, its inbox and the prompt untouched; the prompt SHALL stay retrievable with `cyber_prompt_wait`. No argument SHALL make cancellation interrupt.
- Derivation: the run SHALL stay `running` while an input admitted after the promotion still waits to be promoted (a Job's handback) and while a Job started at or after the promotion is running or has ended without its handback admitted (a handback still missing 30 s after its Job ended is presumed lost, as in `exec-mode`). An idle run without a recorded end of a Turn after the promotion (stopped by an interrupt or a restart) SHALL be `interrupted`. A `failed` result SHALL carry `error` `{ _tag: "UnknownError", message }` with the Turn's failure message. A `cyber_prompt` wait SHALL begin before its admission, so a request its run raises right after admission ends the wait; a replay and `cyber_prompt_wait` begin at the call. A retry SHALL also replay when the prompt with the key's derived `id` is already in the Session's inbox even though its response is not stored yet.
- Streams: a waiting call SHALL send `: keep-alive` when its stream opens and then every 10 s, each followed (with a progress token, once admitted) by a `keep_alive` progress notification. Once the prompt is admitted (or re-attached by `cyber_prompt_wait`), the stream SHALL carry one SSE event named `dev.cyber-code/receipt` with data `{ session_id, message_id }`, with or without a progress token; it is not a JSON-RPC message, so MCP clients, which read only unnamed and `message` events, ignore it. The stdio bridge SHALL NOT forward it and SHALL copy its ids into the -32010 `data` of a request whose stream ends without a response. A cancelled waiting call answered as JSON (without `Accept: text/event-stream`) SHALL get `202` with no body.
- `cyber_interrupt` SHALL accept `{ session_id, stop_background?: boolean = false }`, call `POST /api/v1/sessions/:sessionID/interrupt` with the same semantics (session-runtime Interrupt) and return `{ session_id, was_running: boolean }`. Interrupting an idle Session SHALL succeed with `was_running: false`. Waits on prompts whose run it stops SHALL end with `status: "interrupted"`.

#### Scenario: Wait timeout returns a receipt
- **WHEN** a run is still going after `wait_seconds: 60`
- **THEN** the call returns `status: "running"`, `done: false`, the `session_id` and `message_id`, and `isError` is false

#### Scenario: No-wait receipt then wait
- **WHEN** a client calls `cyber_prompt` with `wait: false` and later `cyber_prompt_wait` with the returned IDs
- **THEN** the first call returns `queued` or `running` with `done: false` immediately and the second returns `completed` once the run finishes

#### Scenario: Waiting for approval reported
- **WHEN** a TUI user is attached to the Session and the run raises a `bash` permission request during the wait
- **THEN** the call returns `status: "waiting_for_approval"` with one `pending` entry, and a following `cyber_prompt_wait` does not return early for that already-pending request and returns `completed` once the human approves and the run finishes

#### Scenario: Large final answer
- **WHEN** a run ends with a 200 KiB final answer
- **THEN** `final_text` holds the first 64 KiB and `truncated` is true

#### Scenario: Progress keep-alive
- **WHEN** a call with a progress token and `wait_seconds: 600` waits on a 10-minute Turn
- **THEN** consecutive notifications are at most 15 s apart, `progress` strictly increases, and the first has `kind: "admitted"` and the `message_id`

#### Scenario: Cancellation does not interrupt
- **WHEN** a client sends `notifications/cancelled` for a waiting `cyber_prompt` while the Turn runs `cargo test`
- **THEN** the call ends without a result, the command and Turn continue, and `cyber_prompt_wait` later returns the run's result

#### Scenario: Background Job keeps the run open
- **WHEN** a prompt's run starts a background Job and its Session goes idle before the Job ends
- **THEN** `cyber_prompt_wait` reports `running` until the Job ends and its handback Turn finishes, then `completed` with that Turn's answer as `final_text`

#### Scenario: Explicit interrupt
- **WHEN** a client calls `cyber_interrupt` while another call waits on that Session's run
- **THEN** the Session is interrupted, `was_running` is true, and the waiting call returns `status: "interrupted"`

#### Scenario: Interrupt idle Session
- **WHEN** a client calls `cyber_interrupt` on an idle Session
- **THEN** it succeeds with `was_running: false` and nothing changes

### Requirement: Messaging over MCP
(P1) The messaging tools SHALL map onto the `cross-session-messaging` Messaging API:
- `cyber_message_list` SHALL accept `{ directory?: absolute path }` and return `{ sessions: [{ address, name, kind, state, location, canReceive }] }` from `GET /api/v1/messaging/sessions`.
- `cyber_message_send` SHALL accept `{ to: string, text: string, idempotency_key? }`, send through `POST /api/v1/messaging/send` with addressing, inbound controls, rate limits and outcomes unchanged, and return `{ message_id, outcome: "delivered"|"held"|"refused", reason?, replayed? }`. When the caller Session is known ("MCP recursion guard"), the message SHALL be sent as that Session (origin `session`, from its address), exactly as its own `send_message`; otherwise it SHALL use the Messaging API origin `client`. Inbound rules SHALL apply to that origin unchanged. Messaging errors (`MessageTooLargeError` above 16 KiB, `AmbiguousAddressError`) SHALL be returned unchanged.
- `cyber_message_history` SHALL accept `{ session_id, cursor?, limit?: 1..100 = 50, max_bytes?: 1024..196608 = 65536 }` and return `{ messages: [{ message_id, direction: "in"|"out", from, to, outcome, size, text?, created_at }], next_cursor?, truncated: boolean }` from `GET /api/v1/sessions/:sessionID/messaging`, bounded by `max_bytes` with the `cyber_session_transcript` page rule.

An MCP client SHALL have no message address: the receiving model cannot reply to it, and replies SHALL be read through the transcript or a prompt result. Messages SHALL be delivered as untrusted data and SHALL NOT change the receiver's Mode or permissions.

#### Scenario: Send to a running Session
- **WHEN** a client sends `{ to: "migration", text: "schema v2 is merged" }` to a running local Session
- **THEN** the outcome is `delivered`, the Session reads it at its next Safe Boundary as a client message, and its history records origin `client`

#### Scenario: Cyber Session sends as itself
- **WHEN** Session A, through a configured `cyber mcp serve` server, calls `cyber_message_send` to B, and B refuses messages from A's address
- **THEN** the outcome is `refused`, as for A's own `send_message`

#### Scenario: Exec Session refuses
- **WHEN** a client messages a `cyber exec` Session started without `--accept-messages`
- **THEN** the outcome is `refused` with reason `non_interactive`

#### Scenario: Oversized message
- **WHEN** a client sends 20 KiB of text
- **THEN** the call returns `isError` with `MessageTooLargeError` in the text content and nothing is sent

### Requirement: Approvals stay with human clients
(P1) MCP serve SHALL NOT relay permission requests or questions to MCP clients: it SHALL send no elicitation requests and SHALL offer no tool that replies to a permission request, a question or a held inbox item. A request raised in a Session driven through MCP SHALL be handled exactly as for any API-created Session: when an interactive client (TUI, web or SDK) is attached, it waits for that human; otherwise `permissions-modes` "Non-interactive behavior" resolves a permission request by the Session's Mode, and Sessions created through MCP never raise questions ("Session creation over MCP"). MCP connections SHALL NOT count as attached interactive clients. While a request is pending, prompt results and `cyber_session_read` SHALL report it (`waiting_for_approval`, `pending`) so the calling agent knows a human must act. Prompt text SHALL be admitted as user input only and SHALL NOT approve requests or change the Mode.

#### Scenario: No human attached
- **WHEN** a Session created through MCP in Mode `default`, with no interactive client attached, needs approval for `bash`
- **THEN** the request is denied with the printed warning, the run continues, and no request is sent to the MCP client

#### Scenario: Human answers
- **WHEN** a TUI user is attached to a Session driven through MCP and approves its pending `edit` request
- **THEN** the run continues and the MCP client's next `cyber_prompt_wait` reports `running` or a terminal status

#### Scenario: Prompt cannot change the Mode
- **WHEN** a client sends the prompt `/mode bypass` to a `default` Session
- **THEN** no command runs, the text reaches the model as literal user input and the Session Mode stays `default`

### Requirement: MCP serve limits
(P1) The top-level key `mcp_serve` SHALL be read only from the global configuration; elsewhere (project files, profiles, environment content and `--config` overrides) it SHALL be ignored with the warning `mcp_serve is ignored outside the global config`. Its schema SHALL be `{ enabled: boolean = true, max_mode?: Mode (not bypass; default "Session creation over MCP"), limits: { sessions_per_connection: 1..64 = 8, max_waits: 1..32 = 8, max_running_sessions: 1..64 = 16, max_depth: 1..4 = 2, max_wait_seconds: 60..3600 = 600 } }`; unknown keys and out-of-range values SHALL fail validation naming the key. Changes SHALL apply to new requests.
- `sessions_per_connection`: a per-connection limit (not a fan-out bound) on Sessions created by one MCP connection (by `cyber_session_create` or `cyber_prompt` with `directory`); the count resets with the MCP connection. The next creation SHALL fail with `ForbiddenError` `{ reason: "session_quota", limit }`.
- `max_waits`: concurrent waiting `cyber_prompt` and `cyber_prompt_wait` calls per connection; the next SHALL fail with `RateLimitedError` `{ reason: "concurrency", retryAfterMs: 5000 }`.
- `max_running_sessions`: running Sessions that were created through MCP serve or whose current run was started by MCP input ("Prompt admission over MCP"), across all connections; a `cyber_prompt` that would start a run on an idle Session beyond this count SHALL fail with `RateLimitedError` `{ reason: "concurrency", retryAfterMs: 5000 }`. `cyber_session_create` SHALL never be refused by it; message fan-out is bounded by the depth guard and messaging rate limits.
- Fixed bounds: request body 1 MiB, prompt 128 KiB, message 16 KiB, `final_text` 64 KiB, transcript item 16 KiB, `pending` 20 entries. Each bound SHALL be applied by the rule that names it (`final_text` head kept, `pending` first 20, Session list, transcript and message history pages by `max_bytes`), and truncation SHALL never produce invalid JSON.

Apart from replays ("Served tools and errors"), every check of this capability (schema, size, exclusive fields, Mode ceiling, depth, self-target, quota, concurrency) SHALL run before the first underlying write; a refused call SHALL create and admit nothing.

#### Scenario: Refused call leaves nothing behind
- **WHEN** a connection already has `max_waits` waiting calls and calls `cyber_prompt` with `directory`
- **THEN** the call fails with `RateLimitedError` `{ reason: "concurrency" }`, no Session is created and nothing counts toward `session_quota`

#### Scenario: Session quota
- **WHEN** one MCP connection has created 8 Sessions under the default limit and calls `cyber_session_create`
- **THEN** the call fails with `ForbiddenError` `{ reason: "session_quota", limit: 8 }`

#### Scenario: Too many waits
- **WHEN** a connection has 8 waiting calls and starts a ninth `cyber_prompt_wait`
- **THEN** the ninth fails with `RateLimitedError` `{ reason: "concurrency", retryAfterMs: 5000 }` and the others keep waiting

#### Scenario: Project config ignored
- **WHEN** a project `cyber.jsonc` sets `mcp_serve.max_mode: "auto"`
- **THEN** the value is ignored with the warning and the ceiling is unchanged

### Requirement: MCP recursion guard
(P1) MCP serve SHALL bound Sessions driving their own server through MCP with a depth marker:
- Each admitted input (prompt or message) SHALL carry a depth, stored with it at admission: the request depth for MCP input (in MCP serve's records keyed by `message_id`); for a cross-session message delivered by the native `send_message`, the sender's `mcp_depth` at sending (in the runtime's record of that message); and 0 for any other input, including user prompts.
- Depth SHALL be scoped to the run, not the Session: a Session's `mcp_depth` SHALL be the depth of the input that started its current or most recent run, raised to the depth of any deeper input promoted into that run, and 0 before its first run. A later run started by user input therefore has depth 0. Creating a Session SHALL NOT set its depth. Child Sessions (subagents, forks, background Jobs) SHALL take their parent's `mcp_depth` when they start.
- Every process Cyber starts for a Session (tools, hooks, PTYs) SHALL receive `CYBER_MCP_DEPTH=<mcp_depth + 1>` next to the `cli-commands` process environment markers. The stdio bridge SHALL forward it as `X-Cyber-MCP-Depth` and `CYBER_SESSION_ID` as `X-Cyber-MCP-Caller`.
- Cyber's own MCP client SHALL add `_meta["dev.cyber-code/caller"]` `{ session_id, depth: mcp_depth + 1 }` to every `tools/call` it sends to a server whose `serverInfo.name` is `cyber-code`, so per-Location MCP servers carry the marker too.
- Request depth SHALL be the maximum of `X-Cyber-MCP-Depth`, the `_meta` depth and the caller Session's `mcp_depth + 1`, or 0 with no marker. `cyber_session_create`, `cyber_prompt` and `cyber_message_send` SHALL fail with `ForbiddenError` `{ reason: "depth_exceeded", depth, max_depth }` when it is ≥ `mcp_serve.limits.max_depth`; reading, waiting and interrupting SHALL never be depth-limited.
- The caller Session SHALL be the `_meta` caller when present, else `X-Cyber-MCP-Caller`. When it is known, `cyber_prompt`, `cyber_message_send` and `cyber_interrupt` targeting that Session or one of its ancestors SHALL fail with `ForbiddenError` `{ reason: "self_target" }`.
- The markers SHALL be accident guards for trusted local clients, not a security boundary.

#### Scenario: Recursion depth exceeded
- **WHEN** an external agent (depth 0) calls `cyber_prompt` with `directory`, creating Session A whose run has `mcp_depth` 0, the Turn in A calls `cyber_prompt` with `directory` through a configured `cyber mcp serve` server (depth 1), creating Session B whose run has `mcp_depth` 1, and the Turn in B calls `cyber_session_create` (depth 2)
- **THEN** A and B are created and run, and B's call fails with `ForbiddenError` `{ reason: "depth_exceeded", depth: 2, max_depth: 2 }`

#### Scenario: Ping-pong between existing Sessions
- **WHEN** ordinary Sessions A and B have the cyber-code MCP server configured, A's run was started by user input (`mcp_depth` 0), A's Turn calls `cyber_prompt` on idle B (depth 1, so B's run has `mcp_depth` 1), and B's Turn then calls `cyber_prompt` on A, or uses `send_message` (carrying depth 1) to wake idle A, whose new run has `mcp_depth` 1 and whose Turn then calls `cyber_prompt` on B
- **THEN** the second `cyber_prompt` call has depth 2 and fails with `ForbiddenError` `{ reason: "depth_exceeded", depth: 2, max_depth: 2 }`

#### Scenario: Messaged Session uses MCP again after its run
- **WHEN** a long-lived Session C is woken by a `send_message` from a Session whose run has `mcp_depth` 1, so C's run has `mcp_depth` 1, and after that run ends the user prompts C and C's Turn calls `cyber_prompt` with `directory`
- **THEN** C's new run has `mcp_depth` 0, the request has depth 1 and the call succeeds

#### Scenario: Subagent inherits depth
- **WHEN** a subagent of Session B (`mcp_depth` 1) runs `cyber mcp serve` from its `bash` tool and calls `cyber_prompt` with `directory`
- **THEN** the request has depth 2 and fails with `depth_exceeded`

#### Scenario: Session prompts itself
- **WHEN** Session A calls `cyber_prompt` on `session_id` A through MCP
- **THEN** the call fails with `ForbiddenError` `{ reason: "self_target" }` and nothing is admitted

### Requirement: Per-client grants for MCP serve
(P2) MCP serve SHALL support registered MCP client principals with grants that narrow what a client may do (read only, own Sessions only, messaging, prompting) and Location roots, managed with `cyber mcp clients`. Until then every MCP client has the trust of a local client ("Local client trust model").

#### Scenario: Read-only client
- **WHEN** a client registered with only a read grant calls `cyber_prompt`
- **THEN** the call fails with `ForbiddenError` `{ reason: "grant_required" }`

### Requirement: Approval relay to MCP clients
(P2) MCP serve SHALL optionally relay permission requests and questions of Sessions an MCP client drives to that client through MCP elicitation, opt-in per registered client, offering only `once` and `reject`, with protected-path, critical-path and Mode-change requests staying human-only.

#### Scenario: Relayed approval
- **WHEN** a client opted into relay drives a Session that requests `edit src/x.rs`
- **THEN** the client receives an elicitation and answering `once` lets the edit proceed

### Requirement: Team tasks over MCP
(P2) MCP serve SHALL expose the `agent-teams` shared task list to MCP clients: listing tasks, claiming, updating and completing them under the same claim and lease rules as teammates.

#### Scenario: External agent claims a task
- **WHEN** an MCP client claims an unclaimed task of a Team it drives
- **THEN** the task is marked claimed by that client and teammates see the claim

### Requirement: Workflow and goal tools over MCP
(P2) MCP serve SHALL offer `cyber_workflow_run`, `cyber_workflow_wait` and `cyber_goal_set`, mapped onto the `workflows` and `goals` routes, with the receipt, wait and cancellation semantics of `cyber_prompt` and `cyber_prompt_wait`.

#### Scenario: Run a saved workflow
- **WHEN** a client calls `cyber_workflow_run` with a saved workflow name and `wait: false`
- **THEN** it returns the run ID immediately and `cyber_workflow_wait` later returns the run result

### Requirement: Remote MCP serve access
(P3) MCP serve SHALL accept non-loopback and Relay connections only as an OAuth 2.1 protected resource that validates Cyber Account access tokens bound to the endpoint audience, publishes protected resource metadata, and requires registered client principals with grants.

#### Scenario: Remote client without a token
- **WHEN** a remote client connects without a valid token
- **THEN** the endpoint answers `401` with a `WWW-Authenticate` header pointing at the protected resource metadata
