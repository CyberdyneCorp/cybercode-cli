# mcp Specification

## Purpose
Cyber Code is a Model Context Protocol client, so tools, resources and prompts from local (stdio) and remote (Streamable HTTP, SSE) MCP servers become available to every agent. It is also an MCP server, so other agents and IDEs can drive Cyber Code sessions and workflows. Client behavior follows OpenCode v1 (config, OAuth, resources, prompts-as-commands) and Claude Code (tool search with deferred loading, elicitation, non-blocking startup). Server mode follows Codex's `mcp-server`.

## Requirements

### Requirement: Server configuration
(P0) The system SHALL read MCP servers from the `mcp` config key, keyed by server name (`^[a-zA-Z0-9_-]{1,48}$`). Local entries SHALL be `{ type: "local", command, args?, env?, cwd?, enabled?, timeout?, tools? }` and remote entries `{ type: "remote", url, headers?, oauth?, enabled?, timeout?, tools? }`, where `tools` is `{ allow?: [glob], deny?: [glob] }`. `env` and `headers` values SHALL support `{env:NAME}` substitution.

#### Scenario: Local server
- **WHEN** config has `"mcp": { "fs": { "type": "local", "command": "npx", "args": ["-y", "@mcp/fs"] } }`
- **THEN** the server is spawned over stdio when the Location opens only after workspace-trust approves any project-controlled definition

#### Scenario: Tool deny filter
- **WHEN** a server entry sets `tools.deny: ["delete_*"]`
- **THEN** tools whose names match `delete_*` are not registered

### Requirement: Project MCP file compatibility
(P0) The system SHALL also read `.mcp.json` (Claude Code format, `mcpServers` map) and `.cyber/mcp.json` at the project root, merging them below `cyber.jsonc` entries of the same name. Project-defined servers SHALL require user trust (recorded per server definition hash) before first start.

#### Scenario: Untrusted project server
- **WHEN** a cloned repo contains `.mcp.json` defining server `db`
- **THEN** `db` is not started and the user is asked once to trust it

### Requirement: Transports
(P0) The system SHALL connect local servers over stdio (stderr captured to `<data>/log/mcp/<server>.log`), and remote servers via Streamable HTTP first, falling back to SSE when the first attempt fails for a non-authentication reason. An authentication failure SHALL NOT trigger a fallback.

#### Scenario: SSE fallback
- **WHEN** a remote server rejects Streamable HTTP with 405
- **THEN** the system retries with the SSE transport

### Requirement: Non-blocking concurrent startup
(P0) The system SHALL start all enabled servers concurrently without blocking session start. A server's tools SHALL become available at the next Turn after it connects. The built-in tool `wait_for_mcp` SHALL let the model wait up to 60 s for named servers still connecting.

#### Scenario: Slow server
- **WHEN** a server takes 20 s to connect and the user prompts immediately
- **THEN** the first Turn runs without its tools and the model can call `wait_for_mcp` with that server name

### Requirement: Status model
(P0) The system SHALL report each server's status as `connecting`, `connected`, `disabled`, `failed` (with error), `needs_auth`, or `needs_client_registration`. It SHALL publish `mcp.status.changed.1` on every transition, and on connection loss SHALL set `failed`, remove the server's tools and retry with exponential backoff (1 s to 60 s, at most 10 attempts).

#### Scenario: Reconnect after drop
- **WHEN** a connected remote server drops the connection
- **THEN** its tools are removed, status becomes `failed`, and reconnection is retried with backoff

### Requirement: Timeouts
(P0) Connecting and initial listing SHALL time out after `timeout` (default 30 s). Each tool call SHALL time out after `timeout` or `mcp.tool_timeout` (default 300 s), and progress notifications SHALL reset the call timer.

#### Scenario: Progress extends timeout
- **WHEN** a long tool call sends progress every 10 s
- **THEN** the call is not timed out while progress continues

### Requirement: Tool naming and schema
(P0) The system SHALL expose each MCP tool as `mcp__<server>__<tool>`, replacing characters outside `[A-Za-z0-9_-]` with `_` and truncating to 64 characters with a stable hash suffix. Input schemas SHALL be forced to `type: "object"`, and tool annotations (`readOnlyHint`, `destructiveHint`) SHALL be retained for permission defaults.

#### Scenario: Long name truncated
- **WHEN** a tool name would exceed 64 characters
- **THEN** it is truncated with a 6-character hash suffix that is stable across restarts

### Requirement: Tool execution and permissions
(P0) MCP tool calls SHALL pass through hooks and the permission engine with permission action equal to the exposed tool name and resource `*`. Tools annotated `readOnlyHint: true` SHALL default to `allow`, and other tools SHALL default to `ask`. Text content SHALL become tool output subject to the tool output budget, images SHALL become attachments, `structuredContent` SHALL be retained as structured output, and `isError` SHALL fail the call with the joined error text.

#### Scenario: Read-only tool auto-allowed
- **WHEN** an MCP tool annotated `readOnlyHint: true` is called with no matching permission rule
- **THEN** it runs without prompting

### Requirement: Tool search and deferred loading
(P1) When the materialized tool definitions exceed `tool_output.deferred_threshold_tokens` (defined by `tool-registry`), the system SHALL defer MCP tool definitions. In that case it SHALL expose only their names and one-line descriptions plus the `tool_search` tool that returns matching full definitions and loads them into the following Turns.

#### Scenario: Many tools deferred
- **WHEN** 300 MCP tools exceed the threshold
- **THEN** the model sees a name list and calls `tool_search` with `query: "jira"` to load the Jira tools

### Requirement: Resources
(P0) When any connected server supports resources, the system SHALL add tools `mcp_list_resources`, `mcp_list_resource_templates` and `mcp_read_resource`, checked against permission `read` with resource `mcp:<server>:<uri>`. Resources SHALL also be referenceable in prompts as `@<server>:<uri>`, with binary blobs limited to 10 MiB.

#### Scenario: Resource mention
- **WHEN** a user prompt contains `@docs:file:///guide.md`
- **THEN** the resource is read and attached to the prompt

### Requirement: Prompts as commands
(P0) The system SHALL expose each server prompt as slash command `/<server>:<prompt>`, mapping declared arguments positionally to `$1..$N` and resolving prompt messages lazily at invocation.

#### Scenario: MCP prompt command
- **WHEN** server `gh` exposes prompt `review_pr` with argument `number`
- **THEN** `/gh:review_pr 42` fetches the prompt with `number=42` and submits it

### Requirement: Server instructions
(P0) The system SHALL include each connected server's `instructions` in a `<mcp_instructions>` block as a Context Source keyed `mcp/instructions`, omitting servers whose tools are all denied for the agent. Instruction changes SHALL arrive as Mid-Conversation System Messages.

#### Scenario: Instructions omitted when tools denied
- **WHEN** an agent's permissions deny `mcp__db__*`
- **THEN** the `db` server's instructions are not included for that agent

### Requirement: OAuth
(P1) For remote servers, unless `oauth: false`, the system SHALL perform OAuth 2.1 authorization code with PKCE S256 on a 401 response. It SHALL discover metadata via RFC 9728/8414 and use dynamic client registration (RFC 7591) when no `oauth.client_id` is configured. The loopback redirect SHALL be `http://127.0.0.1:<ephemeral>/mcp/oauth/callback`. Tokens SHALL be stored in the OS keyring (file fallback `<data>/mcp-auth.json` mode 0600), bound to the server URL, and refreshed when they expire within 5 minutes.

#### Scenario: Needs auth
- **WHEN** a remote server returns 401 on connect
- **THEN** status becomes `needs_auth` and the user is told to run `cyber mcp auth <name>` or `/mcp`

#### Scenario: URL change invalidates tokens
- **WHEN** the configured `url` for a server changes
- **THEN** stored tokens for the old URL are not sent

### Requirement: Elicitation
(P1) The system SHALL support MCP elicitation by presenting the server's requested schema as a question form through the question flow. It SHALL return `accept` with the values, `decline`, or `cancel`. In non-interactive sessions it SHALL answer `decline` and log the request.

#### Scenario: Form requested
- **WHEN** a server elicits `{ environment: enum[staging, prod] }`
- **THEN** the user sees a choice question and the selection is returned to the server

### Requirement: Roots and sampling
(P1) The system SHALL advertise the `roots` capability, returning the Location directory and any additional writable roots. It SHALL advertise `sampling` only when `mcp.sampling.enabled` is true, routing sampling requests to `model_roles.small` after an `mcp_sampling` permission check and attributing cost to the server.

#### Scenario: Sampling disabled by default
- **WHEN** a server sends `sampling/createMessage` without sampling enabled
- **THEN** the request fails with a method-not-supported error

### Requirement: List-changed refresh
(P0) The system SHALL refresh a server's tools, prompts or resources when it sends the corresponding `list_changed` notification, publishing `mcp.tools.changed.1`. Tools that disappear SHALL make later calls settle as stale.

#### Scenario: Tool removed mid-session
- **WHEN** a server removes tool `foo` and the model then calls `mcp__srv__foo`
- **THEN** the call settles with `Stale tool call: mcp__srv__foo`

### Requirement: MCP CLI and UI
(P0) The system SHALL provide `cyber mcp add <name> [--scope user|project|local] (--url <url> [--header K=V]... | -- <command> [args...] [--env K=V]...)`, `list`, `get <name>`, `remove <name>`, `auth <name>`, `logout <name>`, `debug <name>` and the in-session `/mcp` panel (status, tools, reconnect, auth). Edits SHALL preserve JSONC comments.

#### Scenario: Add remote server
- **WHEN** the user runs `cyber mcp add linear --url https://mcp.linear.app/mcp`
- **THEN** a remote entry is written to user-scope config and the server connects

### Requirement: Organization MCP controls
(P1) Org policy (`policy.mcp`) SHALL be able to allowlist or denylist servers by name, command or URL pattern, and to provide managed servers to every user. A blocked server SHALL report status `failed` with error `blocked by organization policy` and SHALL never be started.

#### Scenario: Denylisted server
- **WHEN** policy denies URL pattern `https://*.untrusted.dev/*`
- **THEN** a matching configured server is not contacted

### Requirement: Shutdown cleanup
(P0) When a Location closes or the server stops, the system SHALL close all MCP clients and terminate local servers and their process groups (SIGTERM, then SIGKILL after 5 s on POSIX; job object termination on Windows).

#### Scenario: Orphaned child prevented
- **WHEN** the server shuts down with a local MCP server that spawned children
- **THEN** the whole process group is terminated

### Requirement: MCP server mode
(P2) The system SHALL provide `cyber mcp serve [--transport stdio|http] [--port N]`, exposing Cyber Code as an MCP server with tools `cyber_session_create`, `cyber_prompt` (admit a prompt and wait for idle or `wait: false`), `cyber_session_read`, `cyber_session_list`, `cyber_workflow_run`, `cyber_goal_set` and `cyber_interrupt`. Permissions SHALL be enforced by the target session's mode, and over HTTP the same authentication as the server API SHALL be required.

#### Scenario: Another agent drives Cyber Code
- **WHEN** an external MCP client calls `cyber_prompt` with `{ directory: "/repo", prompt: "fix the failing test" }`
- **THEN** a session is created or reused in `/repo`, the prompt runs, and the final assistant text is returned as tool content

### Requirement: Server options
(P1) A server entry MAY set `required: true` (the Session's first Turn SHALL wait up to the connect `timeout` for it and fail with `McpRequiredError` naming the server if it is not connected), `headers_command` (a shell command, run outside the sandbox and outside project trust until approved, whose JSON stdout supplies `headers`, refreshed every `headers_refresh_seconds`, default 3600, and redacted from logs) and `output_token_limit` (per-server cap on model-visible tool output, default the global `tool_output` budget). `cyber mcp get <name>` SHALL display the resolved options with header values redacted.

#### Scenario: Required server missing
- **WHEN** server `db` has `required: true` and fails to connect within 30 seconds
- **THEN** the first Turn fails with `McpRequiredError: db` and the prompt stays retryable in the inbox

### Requirement: WebSocket transport
(P2) A remote entry whose `url` uses `ws://` or `wss://` SHALL connect over the MCP WebSocket transport, with the same OAuth, status, timeout and reconnection rules as Streamable HTTP. `ws://` to a non-loopback host SHALL be refused unless `insecure: true` is set.

#### Scenario: WebSocket server
- **WHEN** config defines `{ type: "remote", url: "wss://mcp.example.com/ws" }`
- **THEN** the server connects over WebSocket and its tools are registered as `mcp__<name>__<tool>`
