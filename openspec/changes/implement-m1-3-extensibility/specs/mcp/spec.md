## MODIFIED Requirements

### Requirement: Project MCP file compatibility
(P0) The system SHALL also read `.mcp.json` (Claude Code format, `mcpServers` map) and `.cyber/mcp.json` at the project root, merging them below `cyber.jsonc` entries of the same name. Project-defined servers SHALL require user trust (recorded per server definition hash) before first start.

#### Scenario: Untrusted project server
- **WHEN** a cloned repo contains `.mcp.json` defining server `db`
- **THEN** `db` is not started and the user is asked once to trust it

#### Scenario: Effective server approval is independent and uncached
- **WHEN** a project-controlled server is selected for launch
- **THEN** current checkout approval and separate approval of the effective server name and definition SHALL both be required
- **AND** workspace or hook approval SHALL NOT grant individual server approval
- **AND** changed effective fields or revocation SHALL refuse reuse

#### Scenario: A project field overrides a global server
- **WHEN** a project layer overrides arguments or another field of a globally defined server
- **THEN** every contributing origin SHALL be checked and the effective definition SHALL require project server approval
- **AND** missing or unsupported provenance and malformed trust storage SHALL refuse authorization

#### Scenario: Selected profile retains MCP ownership
- **WHEN** a selected profile combines global MCP fields with project-controlled arguments
- **THEN** effective field provenance SHALL preserve the original global/project origins, including escaped profile names
- **AND** selecting the profile SHALL NOT remove the requirement for project server approval or change the configured sandbox scope

#### Scenario: Review and approve an exact named server
- **WHEN** the user reviews `cyber mcp definitions` and approves `cyber mcp trust NAME --digest DIGEST`
- **THEN** review SHALL preserve original field origins while redacting environment/header values, OAuth settings and URL queries without changing the effective digest
- **AND** approval SHALL require the exact currently loaded project server definition and current checkout approval
- **AND** review/approval SHALL NOT start servers or create Sessions

#### Scenario: Revoke an obsolete server approval
- **WHEN** the current configuration is invalid and the user runs `cyber mcp untrust --digest DIGEST`
- **THEN** the individual approval SHALL remain revocable without interpreting the invalid configuration


### Requirement: Server configuration
(P0) The system SHALL read MCP servers from the `mcp` config key, keyed by server name (`^[a-zA-Z0-9_-]{1,48}$`). Local entries SHALL be `{ type: "local", command, args?, env?, cwd?, enabled?, timeout?, tools? }` and remote entries `{ type: "remote", url, headers?, oauth?, enabled?, timeout?, tools? }`, where `tools` is `{ allow?: [glob], deny?: [glob] }`. `env` and `headers` values SHALL support `{env:NAME}` substitution.

#### Scenario: Local server
- **WHEN** config has `"mcp": { "fs": { "type": "local", "command": "npx", "args": ["-y", "@mcp/fs"] } }`
- **THEN** the server is spawned over stdio when the Location opens only after workspace-trust approves any project-controlled definition

#### Scenario: Tool deny filter
- **WHEN** a server entry sets `tools.deny: ["delete_*"]`
- **THEN** tools whose names match `delete_*` are not registered

#### Scenario: Initial paginated tool discovery
- **WHEN** a configured local server initializes with the tools capability
- **THEN** initial discovery SHALL follow opaque `nextCursor` values and apply allow/deny filters to remote tool names
- **AND** invalid, duplicate or cyclic catalogs SHALL refuse readiness and close the owned local transport with explicit termination evidence
- **AND** native acknowledgement SHALL NOT itself authorize cleanup before durable caller settlement

#### Scenario: Transport-owned remote headers are refused
- **WHEN** a remote entry configures a header named `Host`, `Content-Length`, `Transfer-Encoding`, `Connection`, `Accept`, `Content-Type`, `Mcp-Session-Id`, `MCP-Protocol-Version` or any `Proxy-*` name, in any case
- **THEN** the definition SHALL be refused at parse time without echoing header values
- **AND** the remote transport SHALL refuse the same names again before any request

#### Scenario: Substituted header values bind approval
- **WHEN** a remote header value uses `{env:NAME}` substitution
- **THEN** the approval digest SHALL hash the substituted value
- **AND** rotating the environment value SHALL change the digest, so a project-defined server SHALL require re-approval before it is contacted again
- **AND** for a definition that requires individual approval, `cyber mcp definitions` and `cyber mcp get` SHALL show each `url`, `headers` or `env` value that a project layer supplied with an `{env:}` or `{file:}` placeholder as its unsubstituted template, so the approver sees which local values would be sent, and SHALL NOT print the resolved value

### Requirement: Transports
(P0) The system SHALL connect local servers over stdio (stderr captured to `<data>/log/mcp/<server>.log`), and remote servers via Streamable HTTP first, falling back to SSE when the first attempt fails for a non-authentication reason. An authentication failure SHALL NOT trigger a fallback.

#### Scenario: SSE fallback
- **WHEN** a remote server rejects Streamable HTTP with 405
- **THEN** the system retries with the SSE transport

#### Scenario: Fallback is decided once per connect inside the original deadline
- **WHEN** the first initialize attempt of a remote connect fails before any valid initialize response with HTTP 400, 404 or 405, a content type that is not JSON or an event stream, or a transport error
- **THEN** the first bridge and its in-flight requests SHALL be stopped and joined before the other transport starts on a fresh bridge
- **AND** the retry SHALL stay inside the connect's original absolute `timeout`, which SHALL NOT be reset
- **AND** other failures, including timeouts, oversized bodies, invalid bodies, other statuses, cancellation and any failure after a valid initialize response, SHALL NOT fall back
- **AND** the transport that connected SHALL be recorded so a reconnect tries it first

#### Scenario: Legacy SSE message exchange
- **WHEN** a remote server is used over the legacy HTTP+SSE transport
- **THEN** the system SHALL open one GET with `Accept: text/event-stream` and the configured headers and wait for its first `endpoint` event
- **AND** an endpoint that does not resolve to the configured URL's scheme, host and port (or that changes its credentials) SHALL fail the connect closed before any request or header reaches it
- **AND** each client JSON-RPC message SHALL be one POST with `Content-Type: application/json` to that endpoint, whose 2xx body is discarded
- **AND** responses and notifications SHALL arrive as `message` events on the GET stream, bounded to 1 MiB per event, and the stream ending SHALL close the transport
- **AND** the endpoint SHALL leave through the same owned proxy and network decision as the configured URL

#### Scenario: Authentication failure does not fall back
- **WHEN** a remote server answers the initialize request, the legacy event stream or a legacy POST with 401 or 403
- **THEN** the connect SHALL fail with a typed authentication outcome without sending any further request and without trying the other transport
- **AND** the connection SHALL settle with status `needs_auth` as specified by the Status model

#### Scenario: Streamable HTTP message exchange
- **WHEN** a configured remote server is connected
- **THEN** each client JSON-RPC message SHALL be one POST with `Content-Type: application/json` and `Accept: application/json, text/event-stream`
- **AND** a JSON body or each `message` event of a `text/event-stream` body SHALL be delivered to the shared protocol client, with 202 accepting notifications and responses
- **AND** the `Mcp-Session-Id` returned by initialize SHALL be sent on every later request, and `MCP-Protocol-Version` SHALL carry the negotiated version after initialize
- **AND** progress events in an event stream SHALL reset the call timer as for local servers
- **AND** response bodies and individual events over 1 MiB, redirects, other statuses and other content types SHALL close the transport without following a redirect

#### Scenario: Streamable HTTP event stream
- **WHEN** a Streamable HTTP server has accepted `notifications/initialized`
- **THEN** the system SHALL open one GET with `Accept: text/event-stream`, the configured headers, `Mcp-Session-Id` and `MCP-Protocol-Version`, and 405 or any other 4xx except 401, 403 and a 404 for a request carrying `Mcp-Session-Id` SHALL mean the server offers no stream, recorded as a diagnostic carrying only the status
- **AND** notifications, server requests and progress from that stream SHALL reach the same protocol client as POST responses, and client replies SHALL be POSTed back expecting 202
- **AND** delivery SHALL be flow-controlled by the bounded client pipe, so the idle pump handles a bounded batch per tick and no stream is buffered without bound
- **AND** while idle, `ping` SHALL be answered, `roots/list` SHALL be refused because remote servers receive no local roots, form elicitation SHALL be declined and sampling SHALL be refused

#### Scenario: Event stream resumption
- **WHEN** a Streamable HTTP GET event stream ends cleanly, or a request's POST event stream ends before that request's response
- **THEN** the system SHALL reopen it with a GET carrying the configured headers, `Mcp-Session-Id`, `MCP-Protocol-Version` and `Last-Event-ID` set to the stream's last event id, after the server's last `retry` delay (default 1 s, bounded to 100 ms..60 s)
- **AND** a GET stream SHALL be reopened on the same session without reporting loss, withdrawing tools or interrupting in-flight calls, and a reopen answered as "no stream" SHALL leave liveness to client pings
- **AND** a POST event stream that carried no event id, whose resumption is answered as "no stream", or whose resumptions end three consecutive times without a new event id SHALL fail as transport loss promptly instead of waiting out the call timeout

#### Scenario: Remote liveness signals
- **WHEN** a connected remote server's GET event stream reopen or legacy event stream ends or fails, a request fails at the transport, or a request carrying `Mcp-Session-Id` is answered 404
- **THEN** the connection SHALL be reported lost to the idle monitor, and a 404 SHALL be the typed session-expired outcome rather than a tool error, with no DELETE for the expired session
- **AND** a 401 or 403 on any request of an established connection, including a GET stream (re)open, SHALL settle `needs_auth` as specified by the Status model instead of reconnecting, except for a server with `headers_command` or a stored OAuth refresh token, where it SHALL be loss so the reconnect re-runs the command or renews the token once and settles `needs_auth` only if that connect is refused
- **AND** a Streamable HTTP server without a GET stream SHALL be pinged by the idle monitor at most every 10 s, and a ping unanswered within 5 s or failing at the transport SHALL be loss, while any JSON-RPC answer, including an error, SHALL prove liveness
- **AND** no client ping SHALL be sent while an event stream is open

#### Scenario: Remote network policy
- **WHEN** a remote server is selected by `sandbox.apply_to` with an effective policy other than full access, or runs as a sandboxed MCP-tool hook
- **THEN** network `off` SHALL refuse before contact, and network `proxy` SHALL refuse a URL host outside `allowed_domains` before contact
- **AND** every remote request, whether restricted or not, SHALL leave through an owned loopback TCP proxy that ignores ambient proxy variables and never retries or keeps idle connections
- **AND** stopping that proxy SHALL be part of the shutdown acknowledgement

#### Scenario: Remote project trust before contact
- **WHEN** a project-controlled remote definition lacks current checkout approval or individual digest approval
- **THEN** the remote endpoint SHALL NOT be contacted, and approval SHALL be rechecked immediately before the first request

#### Scenario: Remote diagnostics are redacted
- **WHEN** a remote connection or request fails
- **THEN** diagnostics, logs, status and durable connection records SHALL NOT contain the URL query or header values


### Requirement: Timeouts
(P0) Connecting and initial listing SHALL time out after `timeout` (default 30 s). Each tool call SHALL time out after `timeout` or `mcp.tool_timeout` (default 300 s), and progress notifications SHALL reset the call timer.

#### Scenario: Progress extends timeout
- **WHEN** a long tool call sends progress every 10 s
- **THEN** the call is not timed out while progress continues

#### Scenario: Initialization and listing share the startup timeout
- **WHEN** initialization and multiple listing pages consume the configured connect timeout
- **THEN** later pages SHALL NOT receive a fresh startup deadline
- **AND** matching listing progress SHALL NOT extend that absolute deadline


#### Scenario: Explicit server call timeout
- **WHEN** a server explicitly configures timeout
- **THEN** tool-call inactivity SHALL use that timeout instead of mcp.tool_timeout
- **AND** an omitted server timeout SHALL retain the 30-second connect default and use mcp.tool_timeout for calls
- **AND** approval identity SHALL distinguish an explicit call override from an omitted connect default

#### Scenario: Invalid progress does not extend inactivity
- **WHEN** progress repeats or regresses, has malformed optional fields, includes a request identity or refers to another token
- **THEN** it SHALL NOT reset the active request's inactivity deadline
- **AND** finite increasing fractional progress SHALL remain valid
- **AND** timeout SHALL preserve unresolved RPC ownership until independent native settlement

#### Scenario: Remote request lifetime and progress
- **WHEN** a remote call is waiting for its response
- **THEN** progress delivered on the POST event stream or on the GET event stream SHALL reset the call timer
- **AND** every HTTP request SHALL be owned by the connection's bridge, so a call that times out leaves the connection unresolved and closing it SHALL end every in-flight HTTP request instead of letting it outlive the RPC deadline

### Requirement: Tool naming and schema
(P0) The system SHALL expose each MCP tool as `mcp__<server>__<tool>`, replacing characters outside `[A-Za-z0-9_-]` with `_` and truncating to 64 characters with a stable hash suffix. Input schemas SHALL be forced to `type: "object"`, and tool annotations (`readOnlyHint`, `destructiveHint`) SHALL be retained for permission defaults.

#### Scenario: Long name truncated
- **WHEN** a tool name would exceed 64 characters
- **THEN** it is truncated with a 6-character hash suffix that is stable across restarts

#### Scenario: Normalized definitions retain remote identity
- **WHEN** discovery normalizes a tool name and forces its input schema to object
- **THEN** the original remote name SHALL remain available for RPC dispatch
- **AND** schema properties, annotations, output schemas and extension metadata SHALL remain retained

#### Scenario: Unlisted calls are refused locally
- **WHEN** a caller requests a tool absent from the successfully discovered catalog
- **THEN** the request SHALL be refused before tool RPC effects, even if the configured filter allows its name

#### Scenario: Explicit refreshed tool becomes stale
- **WHEN** an explicit local server refresh removes a previously exposed tool
- **THEN** a later exposed-name call SHALL fail with `Stale tool call: <exposed-name>` before RPC dispatch
- **AND** calls to retained tools SHALL use their original remote names and fresh server approval

#### Scenario: Refresh preserves remote identity
- **WHEN** refresh removes a tool and adds a different remote tool whose normalized name would reuse the removed exposed alias
- **THEN** the new tool SHALL receive a distinct exposed alias
- **AND** an older call to the removed alias SHALL remain stale rather than dispatch to the replacement
- **AND** restoring the original remote tool within that connection SHALL restore its original alias

### Requirement: Status model
(P0) The system SHALL report each server's status as `connecting`, `connected`, `disabled`, `failed` (with error), `needs_auth`, or `needs_client_registration`. It SHALL publish `mcp.status.changed.1` on every transition, and on connection loss SHALL set `failed`, remove the server's tools and retry with exponential backoff (1 s to 60 s, at most 10 attempts).

#### Scenario: Reconnect after drop
- **WHEN** a connected remote server drops the connection
- **THEN** its tools are removed, status becomes `failed`, and reconnection is retried with backoff

#### Scenario: Remote reconnect after loss
- **WHEN** a remote server is reported lost and its bridge and proxy are acknowledged stopped
- **THEN** its tools SHALL be withdrawn, the owner SHALL settle with status `failed`, and reconnection SHALL use the shared backoff (1 s doubling to 60 s, at most 10 attempts)
- **AND** each attempt SHALL be a new durable owner with a fresh session that re-resolves configuration and trust and re-authorizes before contact, publishing `connecting` then `connected` events without header values
- **AND** an authentication failure during reconnection SHALL stop further attempts and settle `needs_auth`
- **AND** `notifications/tools/list_changed` received over HTTP SHALL refresh the catalog through the same path as local servers, retiring removed tools as stale aliases without reconnecting

#### Scenario: Remote server needs authentication
- **WHEN** a remote connect or reconnect fails with the typed authentication outcome (HTTP 401/403), or an established remote connection is lost to that outcome, and its bridge and proxy are acknowledged stopped
- **THEN** the durable owner SHALL commit phase `settled`, `acknowledged: true` and status `needs_auth`, and `mcp.status.changed.1` SHALL publish that transition without header values
- **AND** an unacknowledged stop SHALL instead commit phase `unknown` with status `failed`, and the ownership validator SHALL reject `needs_auth` in any phase other than acknowledged `settled`
- **AND** GET `/api/v1/mcp` SHALL report `needs_auth` with the error ``MCP server <name> needs authentication; run `cyber mcp auth <name>```
- **AND** reconnection SHALL stop without further attempts, and a `required: true` server SHALL fail the first Turn with `McpRequiredError`

#### Scenario: Independent durable local connection owner
- **WHEN** a configured local server is admitted for a canonical Location and server name
- **THEN** connection transitions SHALL have independent durable ownership without borrowing or creating a Session
- **AND** another unsettled owner for that Location and name SHALL refuse duplicate native preparation
- **AND** historical consumed owner capabilities SHALL NOT authorize the current transition

#### Scenario: Unknown ownership survives disposal and restart
- **WHEN** preparation or a running local owner is disposed without verified settlement
- **THEN** durable ownership SHALL remain unresolved and fence replacement across database restart
- **AND** persisted observations SHALL NOT be treated as proof of a live actor
- **AND** late verified settlement SHALL require the same retained current owner capability

#### Scenario: Committed local status delivery
- **WHEN** a shared local owner commits a connection transition
- **THEN** the instance event stream SHALL publish `mcp.status.changed.1` with its independent connection aggregate and committed sequence
- **AND** failed commits SHALL NOT publish a transition
- **AND** Location filtering SHALL NOT query or fabricate a Session for the connection
- **AND** Session streams SHALL NOT adopt these independent events as Session history
- **AND** public payloads SHALL omit native owner capabilities and private launch diagnostics

#### Scenario: Read-only configured status snapshot
- **WHEN** a client requests GET `/api/v1/mcp` for a canonical Location
- **THEN** the snapshot SHALL combine freshly loaded configuration with retained runtime observations without starting servers or creating Sessions/events
- **AND** disabled, changed, unstarted and unsupported definitions SHALL report explicit states with no advertised discovery names
- **AND** an unresolved persisted owner without a retained runtime actor SHALL report failed/recovery-required even if its last committed phase was Running
- **AND** unconfigured unresolved owners SHALL remain visible for recovery without exposing private definitions or credentials

#### Scenario: Idle local native loss
- **WHEN** an idle local server leader exits or its ready stdio endpoint reaches EOF
- **THEN** the retained runtime monitor SHALL remove discovery without sending a tool call or heartbeat RPC
- **AND** process-group/job ownership SHALL remain retained until explicit native settlement
- **AND** acknowledged durable settlement SHALL precede owner disposal, while failed settlement remains retryable
- **AND** native inspection SHALL NOT overlap an active call or lose incomplete protocol frames

#### Scenario: Local catalog changes while idle or calling
- **WHEN** a local server sends notifications/tools/list_changed while idle or during a tool call
- **THEN** serialized runtime observation SHALL refresh the catalog after fresh configuration and server authorization checks
- **AND** discovery SHALL be withdrawn during refresh and calls approved against changed metadata SHALL be refused as stale
- **AND** removed aliases SHALL NOT be reassigned to different remote tools
- **AND** Location close SHALL interrupt blocked refresh and retain native ownership until explicit settlement

#### Scenario: Bounded idle frames and buffered EOF
- **WHEN** idle notifications precede EOF or an incomplete frame spans multiple reads
- **THEN** the reader SHALL preserve incomplete bytes across polling and future disposal within the frame limit
- **AND** each idle pass SHALL consume a bounded number of ready messages and observe EOF behind them
- **AND** unsolicited responses or malformed idle frames SHALL fail the connection without invoking tools

#### Scenario: Retry safety after failed local startup
- **WHEN** local startup fails after preparation or native launch
- **THEN** retry safety SHALL require actual native/proxy acknowledgement, durable terminal settlement, checkout lease settlement and scratch cleanup
- **AND** native acknowledgement alone SHALL NOT grant retry authority after failed durable persistence
- **AND** pre-spawn prepared-resource failure SHALL explicitly join the proxy and preserve scratch until caller settlement

#### Scenario: Local reconnect after fully settled loss
- **WHEN** a retained local connection is lost and its native owner, durable receipt, checkout leases and scratch are fully settled
- **THEN** the retained monitor SHALL retry with exponential delays from 1 to 60 seconds and at most ten attempts per loss sequence
- **AND** every attempt SHALL freshly resolve configuration and authorization before claiming pins and admitting a new independent owner
- **AND** unsafe settlement or disposed initialization SHALL stop automatic replacement and preserve recovery fences
- **AND** old tool registrations SHALL remain stale and interrupted tool calls SHALL NOT be replayed
- **AND** Location close and runtime shutdown SHALL cancel backoff/admission and join the original retained monitor

#### Scenario: Named bounded startup wait
- **WHEN** wait_for_mcp is called for configured named servers with an optional timeout from zero to sixty seconds
- **THEN** the built-in SHALL wait only while requested servers are connecting, without starting servers or invoking peer RPCs
- **AND** the result SHALL report redacted observations and whether the wait deadline expired
- **AND** ordinary schema, agent, hook, permission and cancellation boundaries SHALL apply
- **AND** unknown names SHALL fail before waiting and readiness SHALL NOT authorize calls against a previous Turn catalog

### Requirement: Shutdown cleanup
(P0) When a Location closes or the server stops, the system SHALL close all MCP clients and terminate local servers and their process groups (SIGTERM, then SIGKILL after 5 s on POSIX; job object termination on Windows).

#### Scenario: Orphaned child prevented
- **WHEN** the server shuts down with a local MCP server that spawned children
- **THEN** the whole process group is terminated

#### Scenario: Local settlement preserves complete managed checkout ownership
- **WHEN** a local server uses a managed Location
- **THEN** its independent native owner SHALL pin all actual enclosing managed checkout identities before server spawn
- **AND** verified native/proxy shutdown SHALL commit durable acknowledgement before settling checkout activity and enabling scratch cleanup
- **AND** acknowledged checkout locks SHALL remain retained through scratch disposal

#### Scenario: Failed durable settlement remains retryable
- **WHEN** local native shutdown succeeds but its durable terminal commit fails
- **THEN** the retained object SHALL preserve its checkout pins and scratch for settlement retry
- **AND** its tools and calls SHALL remain unavailable while closing
- **AND** retry SHALL NOT require a replacement native connection or owner

#### Scenario: Explicit close survives current Turn catalog refresh
- **WHEN** a shared Location is explicitly closed during a native tool callback
- **THEN** subsequent tool catalog refresh in that Turn SHALL NOT start a replacement connection
- **AND** a later explicit Session or Turn open MAY reconnect with a fresh identity and authorization

#### Scenario: Authenticated public Location close
- **WHEN** an authenticated client posts to `/api/v1/mcp/close` without a body
- **THEN** the selected canonical Location's retained MCP owners SHALL settle before a Located `{closed:true}` response
- **AND** unknown ownership SHALL return ConflictError without removing its admission fence
- **AND** requests without valid authentication SHALL cause no MCP shutdown effects
- **AND** hosts without the close capability SHALL return ServiceUnavailableError

#### Scenario: POSIX graceful Location shutdown
- **WHEN** an idle connected local server is closed with its Location
- **THEN** its client IO SHALL close and the owned process group SHALL receive SIGTERM
- **AND** the unreaped leader identity SHALL remain retained for five seconds before SIGKILL and native acknowledgement
- **AND** cooperative descendants SHALL have time to finish cleanup even if the leader exits first
- **AND** stubborn descendants SHALL NOT survive acknowledged close

#### Scenario: Remote session close
- **WHEN** a connected remote server is closed with its Location or on host shutdown
- **THEN** its bridge and in-flight requests SHALL be stopped and joined, then the session SHALL be ended with DELETE carrying `Mcp-Session-Id` under a short deadline
- **AND** 405 SHALL count as success, and any other DELETE failure SHALL be a redacted diagnostic that does not withhold acknowledgement
- **AND** the owned proxy SHALL be stopped and joined before durable settlement

#### Scenario: Retained grace deadline and concurrent owners
- **WHEN** a graceful close future is cancelled and retried
- **THEN** the same native actor SHALL retain its original grace deadline and admission fence
- **AND** retry SHALL resume the remaining grace period
- **AND** independent owners SHALL close concurrently without abandoning other owners when one fails

### Requirement: Non-blocking concurrent startup
(P0) The system SHALL start all enabled servers concurrently without blocking session start. A server's tools SHALL become available at the next Turn after it connects. The built-in tool `wait_for_mcp` SHALL let the model wait up to 60 s for named servers still connecting.

#### Scenario: Slow server
- **WHEN** a server takes 20 s to connect and the user prompts immediately
- **THEN** the first Turn runs without its tools and the model can call `wait_for_mcp` with that server name

#### Scenario: Shared independent local startup
- **WHEN** multiple Sessions open the same canonical Location with a configured local server
- **THEN** startup SHALL NOT await that server's initialization
- **AND** those Sessions SHALL share one independent native connection and verified managed checkout pins
- **AND** ready tools SHALL become available only through later Turn materialization

### Requirement: Tool execution and permissions
(P0) MCP tool calls SHALL pass through hooks and the permission engine with permission action equal to the exposed tool name and resource `*`. Tools annotated `readOnlyHint: true` SHALL default to `allow`, and other tools SHALL default to `ask`. Text content SHALL become tool output subject to the tool output budget, images SHALL become attachments, `structuredContent` SHALL be retained as structured output, and `isError` SHALL fail the call with the joined error text.

#### Scenario: Read-only tool auto-allowed
- **WHEN** an MCP tool annotated `readOnlyHint: true` is called with no matching permission rule
- **THEN** it runs without prompting

#### Scenario: Explicit policy remains above annotation defaults
- **WHEN** a read-only annotated tool is denied by current explicit policy or a tool hook
- **THEN** annotation defaults SHALL NOT authorize its RPC effects
- **AND** non-read-only tools SHALL be omitted in plan Turns and still refused by plan permission checks

#### Scenario: Native registration changes while approval is pending
- **WHEN** a tool's advertised connection or remote identity changes before RPC dispatch
- **THEN** the old invocation SHALL settle as stale rather than reach a replacement owner
- **AND** current configuration and trust SHALL be rechecked after hooks, approval and native queueing

#### Scenario: Interrupted unresolved local call
- **WHEN** a dispatched local RPC is interrupted without a response establishing completion
- **THEN** its registration SHALL be removed from subsequent materializations
- **AND** the retained local native owner SHALL attempt explicit process/proxy shutdown with durable acknowledgement or unresolved evidence

### Requirement: Roots and sampling
(P1) The system SHALL advertise the `roots` capability, returning the Location directory and any additional writable roots. It SHALL advertise `sampling` only when `mcp.sampling.enabled` is true, routing sampling requests to `model_roles.small` after an `mcp_sampling` permission check and attributing cost to the server.

#### Scenario: Sampling disabled by default
- **WHEN** a server sends `sampling/createMessage` without sampling enabled
- **THEN** the request fails with a method-not-supported error

#### Scenario: Captured local roots
- **WHEN** an authorized local connection initializes
- **THEN** its roots capability SHALL describe the canonical Location and configured writable paths using deduplicated file URIs
- **AND** private scratch, unrelated readable paths and sibling managed worktrees SHALL NOT be added implicitly
- **AND** advertising roots SHALL NOT expand process sandbox grants or tool execution permission

#### Scenario: Roots callback during an active request
- **WHEN** a server requests `roots/list` during initialization, a tool call or idle processing
- **THEN** the client SHALL return the captured connection roots with the server's request identity
- **AND** invalid callback parameter shapes SHALL receive a JSON-RPC invalid-parameters response without losing the active client request

#### Scenario: Immutable roots snapshot
- **WHEN** the connection advertises `roots.listChanged: false`
- **THEN** its root selection SHALL remain an immutable connection snapshot
- **AND** a fresh connection SHALL capture current authorized configuration

#### Scenario: Sampling configuration and bounded input
- **WHEN** sampling settings or basic request messages are parsed
- **THEN** sampling SHALL default disabled and malformed settings SHALL fail without echoing private values
- **AND** a typed server named sampling SHALL remain a server rather than a settings object
- **AND** request conversion SHALL retain bounded text/image messages, declared user/assistant roles, token allowance and valid temperature/stop sequences
- **AND** unsupported tools, audio and ambient-server-context requests SHALL fail before provider work

#### Scenario: Sampling preserves the selected model and privacy
- **WHEN** an authorized active call requests basic sampling
- **THEN** model preferences SHALL NOT override the configured small model
- **AND** the generated provider request SHALL contain only the server-supplied messages and system prompt, with tools disabled and output bounded by the model allowance
- **AND** conflicting provider body overlays, Session history, Context Sources and Session cache identity SHALL NOT be forwarded
- **AND** Session/ancestor budgets and server-attributed observed usage SHALL apply before settlement

#### Scenario: Sampling local transport and cancellation
- **WHEN** a native small-model sampling stream times out or its active MCP Location closes
- **THEN** its retained task SHALL close the local provider transport and record observed usage before native MCP settlement
- **AND** an attempted call without final usage SHALL remain unpriced rather than be reported free
- **AND** protocol refusals and durable Session events SHALL omit provider credentials and sampling input
- **AND** local transport closure SHALL NOT claim remote processing or final unobserved usage was rolled back
- **AND** malformed or immediately denied callbacks SHALL NOT extend call inactivity; only an actual client request or attempted nested provider call SHALL pause that budget

#### Scenario: Sampling budget changes during approval
- **WHEN** a Session or ancestor budget is exhausted while a sampling approval is pending
- **THEN** approval SHALL NOT admit a small-model request
- **AND** a completed sampling request that crosses a soft budget SHALL record observed usage but withhold its completion from the server


### Requirement: Server instructions
(P0) The system SHALL include each connected server's `instructions` in a `<mcp_instructions>` block as a Context Source keyed `mcp/instructions`, omitting servers whose tools are all denied for the agent. Instruction changes SHALL arrive as Mid-Conversation System Messages.

#### Scenario: Instructions omitted when tools denied
- **WHEN** an agent's permissions deny `mcp__db__*`
- **THEN** the `db` server's instructions are not included for that agent

#### Scenario: Effective registration controls source visibility
- **WHEN** MCP registrations are filtered by agent or Mode controls, withdrawn, stale, or shadowed by higher client registrations
- **THEN** those registrations SHALL NOT make their server instructions eligible for the current source observation
- **AND** eligible deferred MCP registrations SHALL retain their server guidance without loading their schemas

#### Scenario: Reconnection replaces instructions
- **WHEN** a fresh authorized connection supplies different instructions
- **THEN** the changed `mcp/instructions` source SHALL append a Mid-Conversation System Message
- **AND** the existing Epoch baseline SHALL remain byte-stable

#### Scenario: All instruction sources withdrawn
- **WHEN** no connected authorized server has instructions and an effective MCP registration
- **THEN** the source SHALL explicitly report absence so the runtime withdraws its applicability in a Mid-Conversation System Message

### Requirement: Server options
(P1) A server entry MAY set `required: true` (the Session's first Turn SHALL wait up to the connect `timeout` for it and fail with `McpRequiredError` naming the server if it is not connected), `headers_command` (a shell command, run outside the sandbox and outside project trust until approved, whose JSON stdout supplies `headers`, refreshed every `headers_refresh_seconds`, default 3600, and redacted from logs) and `output_token_limit` (per-server cap on model-visible tool output, default the global `tool_output` budget). `cyber mcp get <name>` SHALL display the resolved options with header values redacted.

#### Scenario: Required server missing
- **WHEN** server `db` has `required: true` and fails to connect within 30 seconds
- **THEN** the first Turn fails with `McpRequiredError: db` and the prompt stays retryable in the inbox

#### Scenario: Required local discovery precedes first prompt promotion
- **WHEN** a Session's first Turn has an authorized required local server still connecting
- **THEN** it SHALL await connected publication up to that server's configured timeout before promoting the prompt or calling the model
- **AND** missing, disabled or failed required servers SHALL produce a named McpRequiredError while preserving the pending prompt

#### Scenario: Cancellation preserves independently owned startup
- **WHEN** the first-Turn readiness wait is cancelled
- **THEN** it SHALL leave the prompt pending and SHALL NOT dispose the independently owned Location startup
- **AND** a later wake MAY retry readiness without replaying an admitted model call

#### Scenario: First-Turn readiness survives replay and rewind
- **WHEN** an existing Session has durably started its first model step
- **THEN** replay or conversation rewind SHALL NOT repeat the first-Turn readiness gate
- **AND** a newly forked Session SHALL perform its own first-Turn readiness check

#### Scenario: Local per-server output settlement
- **WHEN** a local MCP result exceeds a configured output_token_limit
- **THEN** model-facing text SHALL use the four-character estimate and the global line/byte ceilings to bound its payload before appending the standard overflow notice
- **AND** full original text SHALL be preserved in a managed overflow file and structured data SHALL remain unchanged
- **AND** failed overflow persistence SHALL fail settlement instead of returning lossy success

#### Scenario: Remote header command
- **WHEN** a remote entry sets `headers_command`
- **THEN** when the server is selected by `sandbox.apply_to` with an effective policy other than full access, or is connected for a sandboxed MCP-tool hook, the connect SHALL fail closed with a fixed diagnostic before the command runs or any request is sent
- **AND** otherwise, only after durable admission and fresh approval, and before any request, the system SHALL run it once outside the sandbox (full access; `/bin/sh -c` on POSIX, PowerShell on Windows under owned Job-object process ownership) in the Location directory with stdin closed, bounded by the remaining connect deadline, and SHALL recheck approval before first contact
- **AND** its stdout SHALL be at most 64 KiB and exactly one JSON object whose values are all strings; those headers SHALL override static `headers` case-insensitively and SHALL be refused when a name is transport-owned or a name or value is invalid
- **AND** a launch failure, non-zero exit, timeout, cancellation or invalid output SHALL fail the connect closed before contact; diagnostics SHALL be fixed messages plus at most the exit status, and command output and header values SHALL NOT reach diagnostics, logs, events, status or durable records
- **AND** `headers_command` and `headers_refresh_seconds` (1 to 86400, default 3600, accepted only with `headers_command`) SHALL be part of the approval digest, so a project-defined command runs only after workspace and individual digest approval

#### Scenario: Remote header refresh
- **WHEN** `headers_refresh_seconds` have elapsed since the last run on a connected remote server
- **THEN** before the next tool call, catalog refresh or idle ping, the system SHALL recheck approval, re-run the command and atomically replace the bridge's headers, so later requests on the same session use them without reconnecting, even when a busy connection skips the pool monitor's tick
- **AND** a failed refresh SHALL stay due and be reported as connection loss, so no request reuses the stale headers, the existing reconnect backoff re-runs the command and stale credentials are never used past their refresh window
- **AND** a command whose termination is not acknowledged SHALL prevent the connection's shutdown acknowledgement

#### Scenario: Resolved server review has no effects
- **WHEN** cyber mcp get <name> reviews a loaded server
- **THEN** it SHALL display resolved required and output-budget settings, provenance and authorization observations with header values and URL queries redacted, and the effective `headers_refresh_seconds` when a command is set
- **AND** a project-defined `headers_command` (one that requires individual digest approval) SHALL be displayed verbatim, as a local `command` is, because it is code that runs outside the sandbox once approved, while a command from the user's own configuration SHALL be redacted
- **AND** it SHALL NOT connect, execute commands, start model work or create a database

### Requirement: Elicitation
(P1) The system SHALL support MCP elicitation by presenting the server's requested schema as a question form through the question flow. It SHALL return `accept` with the values, `decline`, or `cancel`. In non-interactive sessions it SHALL answer `decline` and log the request.

#### Scenario: Form requested
- **WHEN** a server elicits `{ environment: enum[staging, prod] }`
- **THEN** the user sees a choice question and the selection is returned to the server

#### Scenario: Call-bound form ownership and authorization
- **WHEN** a configured local tool call receives a form elicitation request
- **THEN** its question flow SHALL identify the requesting server and use only that call's captured Session/message ownership
- **AND** fresh native registration/server authorization SHALL be checked before presentation and before returning accepted content
- **AND** changed authorization SHALL cancel without forwarding answers

#### Scenario: Unowned or unattended callback
- **WHEN** a valid form arrives during initialization, idle processing or a non-interactive call
- **THEN** it SHALL receive decline without creating a Session question
- **AND** non-interactive decline logging SHALL omit form text and answers

#### Scenario: Typed form settlement and cancellation
- **WHEN** the user completes a form and accepts sharing
- **THEN** typed fields, enumerated values, optional omission, defaults, constraints and formats SHALL validate before values are sent
- **AND** decline/cancel or invalid answers SHALL send no content
- **AND** native Location close SHALL clear the exact call's pending questions while preserving unrelated requests

#### Scenario: Human interaction pauses inactivity
- **WHEN** an attended form waits for the user
- **THEN** that actual interaction time SHALL pause tool-call inactivity
- **AND** invalid, unowned and unattended callbacks SHALL NOT extend the timer

### Requirement: OAuth
(P1) For remote servers, unless `oauth: false`, the system SHALL support OAuth 2.1 authorization code with PKCE S256 for servers that answer 401. The flow SHALL be a user action: a runtime connect, reconnect or call (including `cyber exec`, the app host and hook connections) SHALL NOT run discovery, registration or an authorization request and SHALL NOT open a browser; a refusal settles `needs_auth` or `needs_client_registration` and the user runs `cyber mcp auth <name>`. That command SHALL discover metadata via RFC 9728/8414 and use dynamic client registration (RFC 7591) when no `oauth.client_id` is configured. The loopback redirect SHALL be `http://127.0.0.1:<port>/mcp/oauth/callback`, with an ephemeral port unless `--port` or `oauth.redirect_port` names one. Tokens SHALL be stored in the owner-only file `<data>/mcp-auth.json` (mode 0600), bound to the server URL; an OS keyring backend, preferred over the file when one is available, is deferred and not part of this increment. Tokens SHALL be and refreshed when they expire within 5 minutes (or within half their lifetime when that is shorter).

The remote `oauth` field SHALL be `false` (disabled), `true` or omitted (enabled with defaults), or an object `{ client_id?, client_secret?, scopes?: [scope], redirect_port? }` with unknown fields refused, `client_secret` (which supports `{env:NAME}`) only together with `client_id`, nonempty whitespace-free scope tokens and `redirect_port` between 1024 and 65535. Every form SHALL be bound into the server's approval digest unchanged. Review SHALL show `client_id`, `scopes` and `redirect_port`, redact `client_secret` and, for a project-defined server, show only its unsubstituted `{env:}` template.

A 401/403 SHALL carry the server's parsed `Bearer` `WWW-Authenticate` challenge (`resource_metadata`, `scope`, `error`) internally on the Streamable HTTP, event-stream and legacy SSE paths; diagnostics SHALL NOT contain header values.

Protected-resource metadata SHALL be fetched from the challenge's `resource_metadata`, then `/.well-known/oauth-protected-resource<path>`, then `/.well-known/oauth-protected-resource`, and its `resource` SHALL equal the canonical server URL without its query. The server URL itself SHALL be HTTPS, or plain HTTP only to a loopback host: `cyber mcp auth` SHALL refuse any other server before any request, and a connect SHALL fail closed rather than send a stored bearer to it. Authorization-server metadata SHALL be fetched from the RFC 8414 location, then OpenID Connect discovery, and its `issuer` SHALL equal the named issuer. The metadata URLs, issuer and authorization, token and registration endpoints SHALL be HTTPS, or plain HTTP only to a loopback host, without credentials or fragments. A local host (loopback, unspecified or a `localhost` name) SHALL be accepted only when the URL that named it is local too: the `resource_metadata` URL and issuer only for a local MCP server URL, and the endpoints only for a local issuer, so remote content never directs a request to a service on the user's machine. Every OAuth request SHALL be abandoned after 30 seconds. And `code_challenge_methods_supported` SHALL include `S256`; any violation SHALL fail closed. Without a configured `client_id` and without a `registration_endpoint` the outcome SHALL be the typed `needs_client_registration` condition.

The authorization and token requests SHALL carry the RFC 8707 `resource`: the server URL without its query, since a query may carry a credential that SHALL NOT be sent to the authorization server or printed in the authorization URL; the credential stays keyed by the full URL. The PKCE verifier and `state` SHALL come from the OS random source. The loopback listener SHALL bind 127.0.0.1 only, answer other paths with 404, compare `state` in constant time, fail the flow on a `state` mismatch, an `error=` response or a missing code, time out, and be closed when the wait returns, so a second callback cannot connect.

`cyber mcp auth <name> [--port N]` SHALL check server authorization (trust and, for project-defined servers, the individual digest approval) before any request, including the unauthenticated probe and metadata requests, and recheck it before the token request. All requests SHALL use the scoped network-policy client and its owned proxy, joined before the command returns. The command SHALL print the authorization URL to stderr and SHALL NOT open a browser. Tokens SHALL be stored keyed by server name, canonical URL and issuer in the owner-only credential file, written atomically under a lock, and never in configuration, logs, events, status or command output. A credential file accessible to other users SHALL be refused. Until an owner-only ACL backend exists, the file backend SHALL refuse on Windows.

Unless `oauth: false`, every connect SHALL look up the credential stored for the server name at exactly its canonical URL, after durable admission and the approval recheck and before any request to the server; a credential stored for another URL SHALL never be sent, and there SHALL be no fallback by name alone. A stored issuer or token endpoint that fails the same HTTPS and local-referrer rules or a token that is not visible ASCII SHALL fail the connect closed. Request headers SHALL be layered in the same order on every platform: static `headers`, then `Authorization: Bearer <access token>`, then `headers_command` output, each later layer replacing the same header name. An access token is due when it expires within 5 minutes, or within half its issued lifetime when that is shorter, and never within 10 seconds of being issued. A due token SHALL be refreshed with the stored client and the RFC 8707 `resource` through the server's network-policy client after rechecking approval, and the response, including a rotated refresh token, SHALL be persisted before the new access token is used. Refreshes of one credential (server name and URL) SHALL be serialized across connections and processes by an exclusive per-credential refresh lock held across the token request, so a refresh of another credential never waits for it; under it the stored credential is re-read, and a credential another holder already rotated and that is not due is adopted without a request. The response SHALL be persisted only while the stored credential is still the one refreshed: a credential removed by `cyber mcp logout` SHALL fail the refresh closed (nothing is sent when it is already gone, nothing is written back when it went during the request), and one replaced by `cyber mcp auth` wins. Before every tool call, catalog refresh and idle ping of a connected server, the stored credential SHALL be re-read and a changed one adopted with its issuer and token endpoint re-validated, and a removed one SHALL end the connection and settle `needs_auth` without being sent again, whether or not a reconnect reaches it, (never sending one issuer's refresh token or client secret to another issuer's endpoint), and a due token SHALL be refreshed in place the same way, bounded by the server's `timeout`, replacing the bridge's headers on the same session. A failed or timed-out refresh SHALL end the connection and settle `needs_auth` and is never retried: no later request on that connection contacts the authorization server. A 401 to a stored token that has a refresh token SHALL be retried exactly once inside the same connect after a refresh; a refusal after that refresh SHALL settle `needs_auth`. A 401 or 403 that ends an established connection holding a refresh token SHALL be loss, so the one reconnect renews the token; a request refused in-session is not replayed. No in-session refusal SHALL run discovery or an authorization flow. Until an owner-only Windows backend exists, Windows connects without stored tokens.

`POST /api/v1/mcp/{name}/reconnect` (no body, Location-scoped) SHALL re-admit a server whose retained connection settled `needs_auth` or `needs_client_registration` and whose startup, monitor and native owner have stopped, or a running server whose stored OAuth credential has been removed, through its admitting Session with a fresh durable owner, and SHALL return the server's status observed right after re-admission (`connecting`), with the following transitions published as `mcp.status.changed.1`. An unconfigured server SHALL be 404, a server not authorized by trust or its digest approval 403 without any request, and any other server, a changed definition (except for a `needs_client_registration` server, whose remedy changes `oauth`, re-admitted under its currently authorized digest), or a closing Location 409. After storing tokens, `cyber mcp auth` SHALL request this reconnect from a healthy registered server, if one exists, report the outcome without tokens, and never start a server.

When `cyber mcp auth` finds neither a configured `client_id` nor a `registration_endpoint`, it SHALL fail with a `needs_client_registration` diagnostic naming `oauth.client_id`, make no registration or authorization request, and record that condition for the server name and exact URL in the credential store. The runtime SHALL NOT run discovery: a connect with no stored credential and no configured `client_id` whose recorded condition is present SHALL settle a refusal as acknowledged `needs_client_registration` (valid only as acknowledged `settled`, like `needs_auth`), stop retrying, and report ``MCP server <name> needs OAuth client registration; ...`` naming `oauth.client_id` and `cyber mcp auth <name>`. A successful `cyber mcp auth` SHALL clear the recorded conditions for that URL.

A 403 whose `Bearer` challenge has `error="insufficient_scope"`, at connect or on an established connection, SHALL settle `needs_auth` whatever credential layers exist, without any refresh, reconnect, discovery or authorization request, and SHALL record the union of the token's granted scopes and the challenge's `scope` tokens for that URL. The next `cyber mcp auth` SHALL request its scope together with every recorded scope. Nothing re-authorizes non-interactively.

`cyber mcp logout <name>` SHALL remove every stored credential and recorded condition for the server name, at every URL and from every issuer, without loading configuration (tokens are not revoked at the authorization server), report how many credentials it removed, and, when it removed any, request the reconnect from a healthy registered server, so a server running on a removed credential reconnects without it and settles `needs_auth`; a running connection the reconnect does not reach (another Location, a busy server) ends at its next request or idle ping and settles `needs_auth`. Server names that are not `[A-Za-z0-9_-]{1,48}` SHALL be refused.

#### Scenario: Discovery order
- **WHEN** `cyber mcp auth` probes a server whose 401 challenge names `resource_metadata`
- **THEN** protected-resource metadata is fetched from that URL first, then `/.well-known/oauth-protected-resource<path>`, then `/.well-known/oauth-protected-resource`, and the first document found decides
- **AND** authorization-server metadata for its first `authorization_servers` issuer is fetched from `/.well-known/oauth-authorization-server<issuer path>`, then `/.well-known/openid-configuration<issuer path>`, then `<issuer>/.well-known/openid-configuration`
- **AND** any non-success answer moves to the next location, and when none answers discovery fails with no authorization request

#### Scenario: Exact identities and HTTPS or loopback endpoints
- **WHEN** discovery reads protected-resource and authorization-server metadata
- **THEN** the `resource` SHALL equal the canonical server URL without its query and the `issuer` SHALL equal the issuer the resource named, exactly, not by origin or prefix
- **AND** the `resource_metadata` URL, issuer and authorization, token and registration endpoints SHALL be HTTPS, or plain HTTP only to a loopback IP address or `localhost`, without user information or a fragment, and the issuer without a query
- **AND** a remote MCP server SHALL NOT name a local `resource_metadata` URL or issuer, and a remote issuer SHALL NOT name a local endpoint, over HTTP or HTTPS
- **AND** requests follow no redirects, so a redirect to another origin is a missing document rather than a followed one

#### Scenario: No browser is opened
- **WHEN** a remote server refuses a connect from `cyber exec`, the app host or a reconnect
- **THEN** the server settles `needs_auth` naming `cyber mcp auth <name>`, and no metadata, registration or authorization request is made and no browser is started
- **AND** `cyber mcp auth <name>` prints the authorization URL to stderr and waits up to 5 minutes for the loopback callback without starting a browser

#### Scenario: Owner-only file fallback
- **WHEN** no OS keyring backend is available (none is implemented yet, so this is every platform)
- **THEN** credentials are written atomically under a lock to `<data>/mcp-auth.json` created with mode 0600, keyed by server name, canonical URL and issuer, together with recorded `needs_client_registration` and required-scope conditions
- **AND** a credential file whose mode grants any group or other access is refused rather than read, and on Windows, until an owner-only ACL backend exists, the file backend refuses and connects send no stored token

#### Scenario: Needs auth
- **WHEN** a remote server returns 401 on connect
- **THEN** status becomes `needs_auth` and the user is told to run `cyber mcp auth <name>`

#### Scenario: Scope step-up on 403
- **WHEN** a connected server answers a tool call with 403 and `WWW-Authenticate: Bearer error="insufficient_scope", scope="a b"`
- **THEN** the connection settles `needs_auth` with no refresh, reconnect or authorization request, and no token appears in status
- **AND** the next `cyber mcp auth` authorization URL requests `a`, `b` and the previously granted scopes

#### Scenario: Needs client registration
- **WHEN** the authorization server offers no `registration_endpoint` and no `oauth.client_id` is configured
- **THEN** `cyber mcp auth` fails with a `needs_client_registration` diagnostic and makes no authorization request, and the server's next connect settles `needs_client_registration`
- **AND** after `oauth.client_id` is configured and `cyber mcp auth` succeeds, the reconnect connects the server

#### Scenario: Logout
- **WHEN** `cyber mcp logout <name>` runs while a registered server is connected with that server's stored credential
- **THEN** every credential stored for the name is removed, the server reconnects without an `Authorization` header and settles `needs_auth`, and no token appears in the command output

#### Scenario: Reconnect after authorization
- **WHEN** a remote server settled `needs_auth`, a token is then stored for its URL, and `POST /api/v1/mcp/<name>/reconnect` is called
- **THEN** the response reports `connecting`, the server becomes `connected` with a fresh durable owner, and its tool calls carry `Authorization: Bearer <token>`
- **AND** no token appears in events, status, durable records, logs or command output

#### Scenario: Refresh before expiry
- **WHEN** a stored access token is due (within 5 minutes of expiry, or half its lifetime when shorter) at connect, or while connected before a call, catalog refresh or idle ping
- **THEN** it is refreshed with the RFC 8707 `resource` after an approval recheck, the rotated refresh token is persisted before the new access token is sent, and a connected session keeps its `Mcp-Session-Id`
- **AND** a rejected, failed or timed-out refresh settles `needs_auth` without contacting the server or the authorization server again, and Location close is never blocked by a token request
- **AND** a token issued with a 300 s lifetime is refreshed once, not on every monitor tick

#### Scenario: Shared credentials refresh once
- **WHEN** two connections (two Locations, or the app host and `cyber exec`) hold the same stored credential and it becomes due
- **THEN** exactly one refresh request presents the refresh token, and the other connection adopts the rotated credential without a request

#### Scenario: Logout or re-authorization during a refresh
- **WHEN** `cyber mcp logout` removes the credential before or while a connection refreshes it
- **THEN** nothing is written back to the store and the connection settles `needs_auth`
- **AND** when `cyber mcp auth` stored a credential from another issuer meanwhile, the connection adopts it with its own token endpoint and stores it under its own issuer

#### Scenario: Retry once after refresh on 401
- **WHEN** an established connection whose stored credential has a refresh token is refused with 401
- **THEN** the connection is lost, the reconnect's refused initialize is followed by exactly one refresh and one more initialize, and the server connects with the renewed token
- **AND** a refusal after that refresh settles `needs_auth` with no further reconnect or refresh

#### Scenario: Disabled OAuth
- **WHEN** a remote server sets `oauth: false`
- **THEN** stored tokens are neither sent nor refreshed and the authorization server receives no request

#### Scenario: URL change invalidates tokens
- **WHEN** the configured `url` for a server changes
- **THEN** stored tokens for the old URL are not sent

#### Scenario: Authorization through the CLI
- **WHEN** `cyber mcp auth remote` runs against a server that answers 401 with `resource_metadata` and an authorization server that supports registration and S256
- **THEN** the authorization URL is printed to stderr, and after the browser reaches the loopback callback the code is exchanged with the PKCE verifier and `resource`
- **AND** the token is stored in `<data>/mcp-auth.json` with mode 0600 under the exact server URL, and no token appears in stdout, stderr or configuration

#### Scenario: Untrusted project server
- **WHEN** `cyber mcp auth` names a project-defined server whose checkout or digest is not approved
- **THEN** it fails without any request to the MCP server or an authorization server

#### Scenario: Metadata mismatch fails closed
- **WHEN** the protected resource names another resource, the metadata names another issuer, an endpoint is plain HTTP to a non-loopback host, or S256 is not offered
- **THEN** discovery fails and no authorization request is made
