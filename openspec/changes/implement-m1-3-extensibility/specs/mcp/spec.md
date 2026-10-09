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
