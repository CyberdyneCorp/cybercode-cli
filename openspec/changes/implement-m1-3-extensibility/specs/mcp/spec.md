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


### Requirement: Timeouts
(P0) Connecting and initial listing SHALL time out after `timeout` (default 30 s). Each tool call SHALL time out after `timeout` or `mcp.tool_timeout` (default 300 s), and progress notifications SHALL reset the call timer.

#### Scenario: Progress extends timeout
- **WHEN** a long tool call sends progress every 10 s
- **THEN** the call is not timed out while progress continues

#### Scenario: Initialization and listing share the startup timeout
- **WHEN** initialization and multiple listing pages consume the configured connect timeout
- **THEN** later pages SHALL NOT receive a fresh startup deadline
- **AND** matching listing progress SHALL NOT extend that absolute deadline


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

#### Scenario: Authenticated public Location close
- **WHEN** an authenticated client posts to `/api/v1/mcp/close` without a body
- **THEN** the selected canonical Location's retained MCP owners SHALL settle before a Located `{closed:true}` response
- **AND** unknown ownership SHALL return ConflictError without removing its admission fence
- **AND** requests without valid authentication SHALL cause no MCP shutdown effects
- **AND** hosts without the close capability SHALL return ServiceUnavailableError

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
