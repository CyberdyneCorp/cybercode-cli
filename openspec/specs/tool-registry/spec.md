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
(P1) When the materialized tool definitions exceed `tool_output.deferred_threshold_tokens` (default 10000 estimated tokens, 4 characters per token), the system SHALL advertise MCP and plugin tools as deferred: name and one-line description only, without parameter schemas. This key is the only deferral threshold; the `mcp` capability SHALL NOT define a separate one. It SHALL also expose a `tool_search` tool that takes `{ query?: string, select?: string[], limit?: number (default 5) }` and returns full schemas, which makes the selected tools callable for the rest of the Session.

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
(P0) The system SHALL write the complete text of every truncated output to an exclusively created file `<data>/tool-output/tool_<ulid>` and record the path in the call's `output_paths` metadata. Cleanup of unpinned files older than 7 days SHALL be performed by the storage GC (`storage-events` → Retention and garbage collection), which preserves references required by active Sessions, workflows, recovery records and backups. Failure to write the overflow file SHALL fail the call operationally instead of returning lossy output.

#### Scenario: Overflow file readable later
- **WHEN** a truncated call recorded `<data>/tool-output/tool_01J...`
- **THEN** the model can `read` that path without an `external_directory` prompt

### Requirement: Parallel tool calls
(P0) The system SHALL execute multiple tool calls from one Turn concurrently when every call in a group is `concurrency_safe` (by default the builtins `read`, `glob`, `grep`, `list`, `webfetch`, `websearch`, `lsp`). Other calls SHALL run serially in the model's emitted order, after all preceding parallel calls settle. At most `tool_output.max_parallel` (default 8) calls SHALL run at once.

#### Scenario: Reads parallel, edits serialized
- **WHEN** a Turn emits `read a`, `read b`, `edit a`
- **THEN** both reads run concurrently and `edit a` starts only after both settle

### Requirement: Eager execution during streaming
(P1) The system SHALL durably record each complete tool call no later than when the provider stream completes, and always before the call starts. It MAY start `concurrency_safe` calls before the provider stream ends. Starting one SHALL first record the answer streamed so far, and SHALL end automatic replay of that answer (`provider-catalog` → Retry policy). It SHALL await all started calls before the next Turn. A complete call of an attempt that is discarded before any call started SHALL NOT be recorded.

#### Scenario: Read starts before stream ends
- **WHEN** the model streams a complete `read` call followed by more text
- **THEN** the read may begin immediately and its result is present before the next Turn

#### Scenario: Complete call of a discarded attempt
- **WHEN** the model streams a complete `write` call, and the body fails with a `Transport` error before protocol completion
- **THEN** no `session.tool.called.1` is recorded for that attempt
- **AND** the replayed attempt's `write` call is recorded once, before it is dispatched

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

### Requirement: Scope-aware deferred materialization
(P1) Deferred materialization SHALL use explicit registration scope rather than name prefixes, after precedence and visibility filtering. The sole threshold SHALL be tool_output.deferred_threshold_tokens, default 10000, measured as four Unicode characters per estimated token with rounding up. Unloaded MCP/plugin tools SHALL be partitioned only when the total strictly exceeds the threshold; loaded selections, builtins and Session registrations SHALL retain full definitions. Deferred summaries SHALL contain only names and one-line descriptions and SHALL NOT disclose parameter schemas or registration capabilities.

#### Scenario: Client with MCP-looking name
- **WHEN** a higher Session client tool is named mcp__shared__read and the definition threshold is exceeded
- **THEN** its actual Session scope SHALL preserve its full definition without revealing a lower MCP registration

#### Scenario: Threshold and loaded selection
- **WHEN** an already effective catalog exceeds the configured threshold and a Session has loaded one MCP name
- **THEN** that name SHALL retain its full definition while other unloaded MCP/plugin schemas remain deferred

### Requirement: Tool output redaction
(P1) The system SHALL redact secrets from the text of every tool result before it reaches the model. This covers built-in tools (`read`, `grep`, `bash`, `powershell`, Job output reads, `webfetch`, `websearch`), MCP, browser, plugin and client tools, subagent results, user `!` shell output, Job completion notices and workflow run summaries.

The registry projection SHALL redact the raw result text first. Truncation (Output size budget) SHALL then measure and cut the redacted text, so a secret is hidden whole wherever the cut falls. Any later framing of the result SHALL wrap the redacted, truncated text.

The runtime SHALL redact the settled output once more, including failures and appended reminders, and SHALL store exactly that text as the `session.tool.settled` `output`. The model's history, the TUI, the web client, exports and replay therefore SHALL never differ. Redaction SHALL be idempotent: an existing marker is never matched again.

A managed overflow file and a Job log SHALL keep the raw, unredacted text on the local disk. Reading either back through a tool SHALL redact it again. `read` SHALL redact the whole file before it selects a page, so a secret cut by a page boundary stays hidden.

A Job completion notice and a workflow run summary SHALL redact the strings they embed before encoding them as JSON. An imported Session's call outputs SHALL be redacted on import.

Tool input that the model wrote, structured values and user-provided context (typed prompts, pasted text, `@` mentions, the text of `!` commands) SHALL NOT be redacted. `tool_output.redact.enabled: false` in user config SHALL turn redaction of tool output off.

#### Scenario: Shell output
- **WHEN** the model runs `bash` with `export GITHUB_TOKEN=$(cat token.txt); echo "token is $GITHUB_TOKEN"` and the file holds `ghp_…`
- **THEN** the model's tool result and the stored settlement both read `token is [REDACTED:github_token]`

#### Scenario: Credentials file
- **WHEN** the model reads an `.npmrc` with `//registry.npmjs.org/:_authToken=npm_…`
- **THEN** the result shows `:_authToken=[REDACTED:npm_token]` and keeps the other lines

#### Scenario: MCP result
- **WHEN** an MCP tool returns text with a Slack token and the value of a secret from its own `env`
- **THEN** the model and the stored settlement get `[REDACTED:slack_token]` and `[REDACTED:known_secret]`

#### Scenario: Overflowed result
- **WHEN** a `bash` result exceeds the output budget and its kept tail holds a PEM private key
- **THEN** the preview shows `[REDACTED:private_key]`, the overflow file holds the raw key, and a `read` of the overflow file shows the marker again, even for a page that starts inside the key

#### Scenario: Job handback
- **WHEN** a background `bash` Job prints a GitLab token and ends
- **THEN** the completion notice admitted for the model has `[REDACTED:gitlab_token]` in `output_tail`, and the Job log keeps the token

### Requirement: Secret detection
(P1) The system SHALL detect secrets with one shared detector list in priority order; the earliest kind names a match that several detectors find. The list SHALL be:

1. Credential formats:
   - AWS access keys (`AKIA…`, `ASIA…`);
   - GitHub tokens (`ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`, `github_pat_`);
   - Anthropic (`sk-ant-`) and OpenAI (`sk-`, `sk-proj-`, `sk-svcacct-`, `sk-admin-`) keys;
   - Slack tokens (`xox[abposr]-`);
   - Stripe live keys (`sk_live_`, `rk_live_`);
   - Google API keys (`AIza`);
   - GitLab (`glpat-`) and npm (`npm_`) tokens;
   - GitHub Copilot API tokens, peer grant tokens (`cyp_`) and JWTs;
   - PEM `-----BEGIN … PRIVATE KEY-----` blocks, through their `END` line, or through their base64 and header lines when the `END` line is missing.
2. Exact values of at least 8 characters of known secrets:
   - environment variables of the `cyber` process whose names look secret (containing `TOKEN`, `SECRET`, `PASSWORD`, `PASSWD`, `API_KEY`, `ACCESS_KEY`, `PRIVATE_KEY`, `CREDENTIAL`, `AUTHORIZATION`, `COOKIE` or similar, not ending in `_PATH`, `_FILE`, `_DIR`, `_URL` or similar);
   - string values under secret-named config keys, such as `providers.<p>.api_key`, MCP server `env` and `headers`, with the bare token of a `Bearer <token>` value;
   - the resolved provider API keys.
3. The configured `tool_output.redact.patterns`.
4. `Authorization` header values, URL passwords (`scheme://user:<password>@`), and assignments `NAME=value` or `"name": "value"` whose name contains `token`, `secret`, `password`, `api_key`, `access_key`, `credential`, `private_key`, `auth_key`, `session_key` or `client_key`.

An assigned value SHALL be redacted only when it:
- is at least 8 characters long;
- does not start with `$`, `{`, `<`, `%`, `[`, `*`, `(` or `#`;
- is not all digits or one repeated character;
- contains two of lowercase letters, uppercase letters and digits;
- is not a dotted or `::` identifier path;
- contains a digit when it is unquoted after `:` or a spaced `=`;
- when unquoted, is followed by whitespace, `,`, `;`, `&`, a closing quote or bracket, a backslash or the end of the line.

A token detector SHALL also match right after a `\n`, `\r` or `\t` escape.

The system SHALL NOT redact text only because it has high entropy. A value matching an `allow` entry SHALL be kept. Known values SHALL never be logged or printed.

Redacting 1 MiB SHALL take well under 10 ms on a developer machine.

#### Scenario: Hashes are not secrets
- **WHEN** output contains a git SHA, a UUID, a base64 PNG data URL and lockfile `integrity` and `checksum` values
- **THEN** nothing is redacted and the text is returned unchanged

#### Scenario: Code is not an assignment
- **WHEN** a source file contains `token = self.next_token()`, `fn lex(token: TokenKind)` and `"max_tokens": 4096`
- **THEN** none of them is redacted, while `DB_PASSWORD=Hunter2Hunter2` in the same file becomes `DB_PASSWORD=[REDACTED:assignment]`

#### Scenario: Known secret values
- **WHEN** `DEPLOY_TOKEN=plain-deploy-value-0042` is set in the server's environment, `providers.corp.api_key` is configured, and a file printing both values is read
- **THEN** each value is replaced by `[REDACTED:known_secret]`

### Requirement: Redaction markers
(P1) The system SHALL replace each redacted value with `[REDACTED:<kind>]`, where `<kind>` is one of:
- `aws_access_key`, `github_token`, `anthropic_key`, `openai_key`;
- `slack_token`, `stripe_key`, `google_api_key`, `gitlab_token`;
- `npm_token`, `copilot_token`, `peer_grant`, `jwt`, `private_key`;
- `known_secret`, `custom`;
- `auth_header`, `url_password`, `assignment`.

An assignment SHALL keep its name, separator and quotes, so that only the value is replaced. A redacted value that spans line breaks SHALL be followed by the same number of line breaks, so line numbers and paging do not change.

#### Scenario: Private key keeps its lines
- **WHEN** a 4-line PEM block sits on lines 2 to 5 of a file
- **THEN** line 2 reads `[REDACTED:private_key]`, lines 3 to 5 are empty, and line 6 is the file's line 6

### Requirement: Redaction visibility
(P1) A settlement that hid anything SHALL carry `redactions: { count, kinds }`, with the number of hidden values and their distinct kinds, sorted. This applies to the `session.tool.settled` event and to the Session's call state, and so to `CallState` in the HTTP API and the SDK.

The count SHALL include values in parts of the raw result that truncation removed. A settlement that hid nothing SHALL omit the field.

#### Scenario: Count on the event
- **WHEN** a `bash` result has one GitHub token and one database password
- **THEN** the settlement has `"redactions": {"count": 2, "kinds": ["assignment", "github_token"]}`
