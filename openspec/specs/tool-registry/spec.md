# tool-registry Specification

## Purpose
The Tool Registry is the single way locally executed tools are represented, registered, advertised to the model and settled. Each tool is an opaque value built from input/output JSON Schema codecs and one executor. Tools are named when registered at a scope (builtin, plugin, MCP, session), materialized once per Turn for the active agent's permissions, and settled through one boundary that validates input/output, bounds the model-visible output and keeps the full output in managed files. It merges OpenCode v2's tool registry (codecs, stale-call detection, managed output files), Claude Code's deferred tools with tool search, and Codex's parallel tool-call model.

## Requirements

### Requirement: Canonical tool values
(P0) The system SHALL represent every local tool as a value with `description`, an `input` JSON Schema, an optional `output` JSON Schema, an `execute` function, optional `to_model_output`, and `annotations` (`read_only`, `destructive`, `open_world`, `concurrency_safe`, all booleans defaulting to `false`). A tool value SHALL have no intrinsic name; it is named at registration.

#### Scenario: Tool advertised from its codecs
- **WHEN** a tool with an input schema `{ path: string }` is registered as `read`
- **THEN** the model receives a definition named `read` whose parameters are that JSON Schema and whose description is the tool's `description`

#### Scenario: Missing annotations default to conservative values
- **WHEN** a plugin registers a tool without `annotations`
- **THEN** the tool is treated as not read-only, not concurrency-safe and possibly destructive

### Requirement: Tool name validation
(P0) The system SHALL validate tool names against `^[A-Za-z][A-Za-z0-9_-]{0,63}$` at registration and SHALL reject a registration containing an invalid name with `ToolRegistrationError` before any tool from that registration becomes visible.

#### Scenario: Invalid name rejected atomically
- **WHEN** a plugin registers tools `lint` and `9bad` in one call
- **THEN** the registration fails with `ToolRegistrationError` naming `9bad`
- **AND** `lint` is not registered either

### Requirement: Scoped registration and precedence
(P0) The system SHALL bind each registration to a scope and SHALL resolve a name collision in precedence order builtin < plugin < MCP < session-scoped, where the higher scope wins and closing a registration reveals the next active registration. MCP tools SHALL be registered under the namespaced name `mcp__<server>__<tool>`, with characters outside `[A-Za-z0-9_-]` replaced by `_` and the result truncated to 64 characters.

#### Scenario: Plugin overrides builtin
- **WHEN** a plugin registers a tool named `grep` while the builtin `grep` exists
- **THEN** Turns use the plugin's `grep`
- **AND** unloading the plugin restores the builtin `grep` for the next Turn

#### Scenario: MCP tool namespacing
- **WHEN** MCP server `git hub` exposes tool `create.issue`
- **THEN** the tool is registered as `mcp__git_hub__create_issue`

### Requirement: Per-turn materialization
(P0) The system SHALL materialize the effective tool set once per Turn for the Turn's agent and Mode. It SHALL omit a tool whose permission action has, as its last matching agent rule, resource `*` with effect `deny`, and SHALL omit mutating tools (not `read_only`) in `plan` mode except the plan-file write. Omission controls visibility only; execution still runs permission checks.

#### Scenario: Fully denied tool hidden
- **WHEN** an agent's rules end with `{ action: "webfetch", resource: "*", effect: "deny" }`
- **THEN** `webfetch` is not included in that agent's tool definitions

#### Scenario: Partially denied tool stays visible
- **WHEN** rules deny `bash` only for resource `rm *`
- **THEN** `bash` stays visible and individual calls are evaluated per call

### Requirement: Deferred tools and tool search
(P1) When the materialized tool definitions exceed `tool_output.deferred_threshold_tokens` (default 10000 estimated tokens), the system SHALL advertise MCP and plugin tools as deferred: name and one-line description only, without parameter schemas. It SHALL also expose a `tool_search` tool that takes `{ query?: string, select?: string[], limit?: number (default 5) }` and returns full schemas, which makes the selected tools callable for the rest of the Session.

#### Scenario: Large MCP catalog deferred
- **WHEN** connected MCP servers contribute 300 tools totaling 60000 schema tokens
- **THEN** the model sees deferred names plus `tool_search`
- **AND** calling a deferred tool before loading it returns `Tool <name> is deferred. Load it with tool_search first.`

#### Scenario: Selecting a deferred tool
- **WHEN** the model calls `tool_search` with `select: ["mcp__github__create_issue"]`
- **THEN** the result contains that tool's full schema and the tool is advertised with parameters on subsequent Turns

### Requirement: Unknown and stale calls
(P0) The system SHALL settle a call to a name not advertised in the Turn with the error result `Unknown tool: <name>`. A call whose advertised registration was removed or replaced before settlement SHALL get `Stale tool call: <name>`. Neither case SHALL invoke any executor. Before reporting an unknown tool, the system SHALL retry the lookup once with the lower-cased name.

#### Scenario: Case-insensitive recovery
- **WHEN** the model calls `Read` and only `read` is advertised
- **THEN** the call is dispatched to `read`

#### Scenario: Stale call after plugin reload
- **WHEN** a plugin tool is replaced between advertisement and settlement
- **THEN** the call settles with `Stale tool call: <name>` and the new executor is not invoked

### Requirement: Codec boundary on settlement
(P0) The system SHALL validate provider input against the tool's input schema before executing it, and SHALL validate the returned value against its output schema when one is declared. Invalid input SHALL settle as a model-visible error `Invalid tool input: <json-pointer>: <reason>` without executing. Invalid output SHALL settle as `Tool returned an invalid value for its output schema: <details>`.

#### Scenario: Invalid input returned to the model
- **WHEN** the model calls `edit` without `old_string`
- **THEN** the call settles with `Invalid tool input: /old_string: required` and the executor is not invoked

### Requirement: Failure versus defect semantics
(P0) The system SHALL turn an expected tool failure (`ToolFailure { message }`) into a model-visible error result carrying only its message. Panics, defects and interruptions SHALL NOT become tool results; they SHALL propagate to the session runtime, which records `outcome_unknown` for dispatched mutations without a known outcome, or an interrupted/crashed failure for calls known not to have mutated state, and logs defects with a reference ID.

#### Scenario: Defect not leaked to model
- **WHEN** a plugin tool throws an unexpected exception containing a stack trace
- **THEN** the model sees `Tool crashed: err_<id>` and the stack trace is only in the log

### Requirement: Invocation context
(P0) The system SHALL invoke every tool with a context `{ session_id, agent, message_id, call_id, location, mode, abort_signal, ask(permission_request), metadata(update) }`. The `abort_signal` SHALL fire when the Session is interrupted or the Turn is cancelled, and tools SHALL stop work within 2 seconds of the signal.

#### Scenario: Interrupt aborts a running tool
- **WHEN** the user interrupts a Session while `bash` is running
- **THEN** the abort signal fires, the process tree is terminated, and the call settles with its known outcome or `outcome_unknown` if dispatched side effects cannot be established

### Requirement: Model output projection
(P0) The system SHALL project a tool's result into model content using `to_model_output` when present. Otherwise a string output SHALL become one text part, and structured output SHALL be rendered as pretty-printed JSON text while also kept as structured data. Image and PDF parts SHALL be carried as base64 media parts with their MIME type and SHALL be dropped with a text placeholder `[<mime> omitted: model has no <modality> input]` when the Turn's model lacks that input modality.

#### Scenario: Image dropped for text-only model
- **WHEN** `read` returns a PNG and the model's catalog entry has no image input
- **THEN** the model receives `[image/png omitted: model has no image input]`

### Requirement: Output size budget
(P0) The system SHALL bound each successful result's model-facing text to `tool_output.max_lines` (default 2000) and `tool_output.max_bytes` (default 51200 UTF-8 bytes), whichever is reached first. Truncation SHALL keep the head by default (the tail for `bash`) and append `[output truncated: <n> lines / <m> bytes omitted; full output at <path>]`. Media parts and structured output SHALL be preserved.

#### Scenario: Oversized grep output truncated
- **WHEN** `grep` produces 9000 lines
- **THEN** the model receives the first 2000 lines plus the truncation notice with the overflow file path

### Requirement: Managed tool output files
(P0) The system SHALL write the complete text of every truncated output to an exclusively created file `<data>/tool-output/tool_<ulid>` and record the path in the call's `output_paths` metadata. It SHALL delete unpinned files older than 7 days in an hourly cleanup, preserving references required by active Sessions, workflows, recovery records and backups per storage-events. Failure to write the overflow file SHALL fail the call operationally instead of returning lossy output.

#### Scenario: Overflow file readable later
- **WHEN** a truncated call recorded `<data>/tool-output/tool_01J...`
- **THEN** the model can `read` that path without an `external_directory` prompt

### Requirement: Parallel tool calls
(P0) The system SHALL execute multiple tool calls from one Turn concurrently when every call in a group is `concurrency_safe` (by default the builtins `read`, `glob`, `grep`, `list`, `webfetch`, `websearch`, `lsp`). Other calls SHALL run serially in the model's emitted order, after all preceding parallel calls settle. At most `tool_output.max_parallel` (default 8) calls SHALL run at once.

#### Scenario: Reads parallel, edits serialized
- **WHEN** a Turn emits `read a`, `read b`, `edit a`
- **THEN** both reads run concurrently and `edit a` starts only after both settle

### Requirement: Eager execution during streaming
(P1) The system SHALL durably record each complete tool call as soon as its input finishes streaming and MAY start `concurrency_safe` calls before the provider stream ends. It SHALL await all started calls before the next Turn.

#### Scenario: Read starts before stream ends
- **WHEN** the model streams a complete `read` call followed by more text
- **THEN** the read may begin immediately and its result is present before the next Turn

### Requirement: Annotations drive modes and hooks
(P1) The system SHALL expose tool annotations to permission modes, hooks and the auto-mode classifier. `read_only` tools SHALL be auto-allowed in `plan` mode subject to path rules. `destructive` tools SHALL always be evaluated by the classifier in `auto` mode. `open_world` tools SHALL trigger a `network` permission check when the sandbox network is off.

#### Scenario: Open-world tool under offline sandbox
- **WHEN** the sandbox has network disabled and the model calls an `open_world` MCP tool
- **THEN** a `network` permission request is raised before execution

### Requirement: Tool registry events and listing
(P0) The system SHALL publish `tool.registered.1`, `tool.unregistered.1` and `tool.catalog_changed.1` events. It SHALL expose `GET /api/v1/tools?location[directory]=...&agent=...`, which returns the materialized definitions with their scope, annotations and deferred flag.

#### Scenario: Listing tools for an agent
- **WHEN** a client requests the tool list for agent `plan`
- **THEN** the response lists only tools visible to `plan`, each with `scope` and `annotations`

### Requirement: Invalid tool fallback
(P0) The system SHALL register a hidden `invalid` tool. Malformed tool calls whose arguments are not parseable JSON SHALL be routed to it, and it SHALL return `The arguments provided to the tool are invalid: <error>` so the model can retry.

#### Scenario: Unparseable arguments
- **WHEN** the provider emits a call to `write` with truncated JSON arguments
- **THEN** the call settles through `invalid` with the parse error and the drain continues

### Requirement: Tool recovery contract
(P0) Each tool SHALL declare `retry_safety: read_only|idempotent|reconcile|never` (default `never`). Before dispatch the runtime SHALL persist the call ID, input digest, permission decision, and attempt state. Idempotent external APIs SHALL receive a stable operation key across retries. Dispatched calls with no durable result SHALL enter `outcome_unknown`; read-only calls may be retried, idempotent calls only with a documented provider guarantee, and other calls only after read-only reconciliation establishes the outcome or the user explicitly authorizes another attempt. A read-only reconciliation handler MAY report `succeeded`, `not_applied` or `unknown` with evidence. Shell commands SHALL default to `never`; a process exit or termination alone SHALL NOT prove absence of effects. Permission checks SHALL be repeated for a retry. The model SHALL NOT evade a recovery hold by issuing an equivalent mutation under a new call ID; when equivalence cannot be safely established, the Session SHALL pause mutating execution pending reconciliation.

#### Scenario: PR created before crash
- **WHEN** a create-PR call succeeds remotely and the server crashes before settlement
- **THEN** resume records an unknown outcome, queries for the PR using the operation evidence, and does not blindly create a second PR
