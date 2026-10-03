# hooks Specification

## Purpose
Hooks let users and organizations run deterministic automation at well-defined points of the agent lifecycle (before/after tools, on prompts, on stop, on compaction, on goal and workflow events) without writing a plugin. They are configured declaratively in `cyber.jsonc` and can block, modify, annotate or observe agent actions. The design follows Claude Code's settings hooks (command, HTTP, prompt and MCP-tool handlers), Codex's trust model for project hooks (`/hooks` trust), and OpenCode v1's plugin hook points (`tool.execute.before/after`, `chat.params`), lowered to configuration.

## Requirements

### Requirement: Hook configuration
(P1) The system SHALL read hooks from the `hooks` config key as a map from event name to an ordered array of hook groups `{ matcher?, hooks: [handler, ...] }`, where each handler has a required `type` (`command`, `http`, `prompt`, or `mcp_tool`) and optional `timeout` (seconds), `async` (boolean), `id` and `description`. Unknown event names SHALL fail config validation with the path of the offending key.

#### Scenario: Valid PreToolUse hook
- **WHEN** `cyber.jsonc` contains `"hooks": { "PreToolUse": [{ "matcher": "bash", "hooks": [{ "type": "command", "command": "./scripts/guard.sh" }] }] }`
- **THEN** the hook is registered for `PreToolUse` events whose tool name matches `bash`

#### Scenario: Unknown event rejected
- **WHEN** a config declares `"hooks": { "BeforeEverything": [] }`
- **THEN** config loading fails with an invalid-config error naming `hooks.BeforeEverything`

### Requirement: Hook scopes and merge order
(P1) The system SHALL collect hooks from the managed, global (`~/.config/cyber`), project (`cyber.jsonc` and `.cyber/` files) and local (`.cyber/cyber.local.jsonc`) scopes plus enabled plugins, and SHALL run every matching hook from every scope rather than letting one scope replace another. Hooks SHALL execute in the order managed, global, project, local, plugin. When `policy.hooks.managed_only` is true, only managed-scope hooks SHALL run.

#### Scenario: Hooks from two scopes both run
- **WHEN** the global scope and the project scope each define a `PostToolUse` hook matching `edit`
- **THEN** both hooks run for an `edit` call, global first

#### Scenario: Managed-only policy
- **WHEN** the organization policy sets `hooks.managed_only: true`
- **THEN** project, local, global and plugin hooks are skipped and a single notice lists how many were skipped

### Requirement: Supported events
(P1) The system SHALL emit hook events `SessionStart`, `SessionEnd`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `PermissionRequest`, `Stop`, `StopFailure`, `SubagentStart`, `SubagentStop`, `PreCompact`, `PostCompact`, `Notification`, `FileChanged`, `CwdChanged`, `ConfigChange`, `TaskCreated`, `TaskCompleted`, `WorktreeCreate`, `WorktreeRemove` and `JobEnded`, and (P2) `GoalEvaluated`, `GoalCompleted`, `WorkflowRunStart`, `WorkflowRunEnd`, `LoopIteration`, `ScheduleRun`, `TeammateIdle` and `MessageReceived`. Each event SHALL carry a common envelope `{ event, session_id, location: { directory, workspace? }, project_id, agent, mode, timestamp }` plus event-specific fields.

#### Scenario: PostToolUse payload
- **WHEN** an `edit` tool call completes successfully
- **THEN** each matching `PostToolUse` hook receives the envelope plus `tool_name`, `tool_input`, `tool_output`, `call_id` and `duration_ms`

#### Scenario: MessageReceived for cross-session messages
- **WHEN** another session's message is delivered into session `ses_1`
- **THEN** `MessageReceived` hooks run with `from_session`, `from_machine` and `text` before the message is shown to the model

### Requirement: Matchers
(P1) The system SHALL match hook groups by `matcher`, which is either a glob over the event's subject (tool name for tool events, including `mcp__<server>__<tool>`; notification type; file path for `FileChanged`) or, when wrapped in `/.../`, a regular expression. An absent or `*` matcher SHALL match every subject. A group MAY add `paths` (globs relative to the Location) that SHALL also match for tool calls with file targets.

#### Scenario: Regex matcher for MCP tools
- **WHEN** a hook group has matcher `/^mcp__github__.*/`
- **THEN** it runs for `mcp__github__create_issue` and not for `bash`

#### Scenario: Path filter
- **WHEN** a `PreToolUse` group has matcher `edit` and `paths: ["migrations/**"]`
- **THEN** it runs for edits under `migrations/` only

### Requirement: Command handlers
(P1) A `command` handler SHALL run its `command` string through the configured shell with the event JSON on stdin, working directory set to the Location directory, and environment variables `CYBER_PROJECT_DIR`, `CYBER_SESSION_ID`, `CYBER_HOOK_EVENT` and `CYBER_AGENT` added. Exit code 0 SHALL mean success, with stdout parsed as a JSON decision when it is a JSON object. Exit code 2 SHALL mean block, and stderr SHALL be fed back as the reason. Any other exit code SHALL be a non-blocking error that is logged and shown to the user.

#### Scenario: Exit 2 blocks a tool
- **WHEN** a `PreToolUse` command hook exits with code 2 and stderr `rm on prod paths is forbidden`
- **THEN** the tool call is not executed and the model receives a tool error containing that text

#### Scenario: Non-blocking failure
- **WHEN** a `PostToolUse` command hook exits with code 1
- **THEN** the turn continues and a warning with the hook id and stderr is shown

### Requirement: HTTP handlers
(P1) An `http` handler SHALL `POST` the event JSON to `url` with `content-type: application/json`, configured `headers` (supporting `{env:NAME}` substitution), and the handler timeout. A 2xx response with a JSON object body SHALL be parsed as a decision. A non-2xx response or a transport error SHALL be treated as a non-blocking error, unless `fail_closed: true` is set, in which case it SHALL block.

#### Scenario: Remote policy server denies
- **WHEN** an `http` `PreToolUse` hook returns `200 {"decision":"deny","reason":"blocked by policy"}`
- **THEN** the tool call is denied with reason `blocked by policy`

#### Scenario: Fail-closed timeout
- **WHEN** an `http` hook with `fail_closed: true` times out
- **THEN** the guarded action is blocked with reason `hook <id> unavailable`

### Requirement: Prompt handlers
(P1) A `prompt` handler SHALL ask the `model_roles.evaluator` model (falling back to `small_model`) to judge the event using the handler's `prompt` text plus the event JSON. The judgement SHALL be returned as structured output `{ decision: allow|deny|ask, reason }`. Prompt handlers SHALL have no tools and SHALL count toward session cost.

#### Scenario: LLM judges a bash command
- **WHEN** a `PreToolUse` prompt hook asks "Deny commands that delete data outside the repo" and the model returns `deny`
- **THEN** the bash call is denied with the model's reason

### Requirement: MCP tool handlers
(P1) An `mcp_tool` handler SHALL call `tool` on the configured MCP `server` with the event JSON as `input` (or the handler's `arguments` template with `${field}` substitution) and SHALL parse a JSON object in the result's text content as a decision.

#### Scenario: Audit via MCP
- **WHEN** a `PostToolUse` handler is `{ "type": "mcp_tool", "server": "audit", "tool": "record" }`
- **THEN** the `audit` server's `record` tool is called with the event payload after each tool call

### Requirement: Decision schema
(P1) A hook decision SHALL be a JSON object with optional fields: `decision` (`allow`, `deny`, `ask`), `reason`, `updated_input` (PreToolUse only, replaces tool input after re-validation against the tool schema), `additional_context` (text admitted as a system message at the next Safe Boundary), `continue` (false stops the Drain after the current Turn), `stop_reason`, and `suppress_output` (hide the hook's output from the transcript). Fields not valid for the event SHALL be ignored with a debug log.

#### Scenario: Input rewritten
- **WHEN** a `PreToolUse` hook returns `{"updated_input": {"command": "npm test -- --ci"}}` for a bash call
- **THEN** the bash tool runs `npm test -- --ci` and the transcript shows the rewrite

#### Scenario: Invalid rewritten input
- **WHEN** `updated_input` fails the tool's input schema
- **THEN** the call is denied with `hook produced invalid tool input`

### Requirement: Decision merging
(P1) When several hooks return decisions for one event, the system SHALL apply them in execution order and combine them: any `deny` SHALL win over `ask`, and `ask` SHALL win over `allow`. `updated_input` SHALL chain, with each later hook receiving the previous hook's output. `additional_context` values SHALL be concatenated. Any `continue: false` SHALL stop continuation.

#### Scenario: Deny beats allow
- **WHEN** one hook returns `allow` and a later hook returns `deny`
- **THEN** the action is denied with the later hook's reason

### Requirement: Interaction with permissions
(P1) `PreToolUse` hooks SHALL run before permission evaluation. A hook `deny` SHALL block even in `bypass` mode. A hook `allow` SHALL skip the `ask` prompt but SHALL NOT override a permission rule whose effect is `deny`. `PermissionRequest` hooks SHALL run when a request would prompt the user and MAY answer it with `allow` or `deny`.

#### Scenario: Hook cannot override deny rule
- **WHEN** a hook returns `allow` for `bash` and the permission rules deny `bash` for `rm -rf *`
- **THEN** the call is denied by the permission rule

#### Scenario: Auto-answer permission prompt
- **WHEN** a `PermissionRequest` hook returns `allow` for an `edit` under `docs/**`
- **THEN** no prompt is shown and the request is approved once

### Requirement: Stop hooks and loop prevention
(P1) `Stop` hooks SHALL run when a Drain is about to go idle and MAY return `{"decision":"block","reason":...}` to admit the reason as a new user-visible instruction and continue. The event SHALL carry `stop_hook_active: true` when the current continuation was caused by a Stop hook. The system SHALL allow at most `hooks.max_stop_continuations` (default 5) consecutive Stop-hook continuations per Drain.

#### Scenario: Tests must pass before stopping
- **WHEN** a `Stop` hook runs the test suite, which fails, and returns `block` with the failure summary
- **THEN** the session continues with that summary as input

#### Scenario: Continuation cap
- **WHEN** Stop hooks have blocked 5 consecutive times
- **THEN** the session stops and a warning `stop hook continuation limit reached` is shown

### Requirement: Timeouts and async hooks
(P1) Each handler SHALL time out after `timeout` seconds (default 60, maximum 600). A timeout SHALL be treated as a non-blocking error unless `fail_closed` is set. A handler with `async: true` SHALL run in the background without blocking the event, and its decision SHALL be ignored except for `additional_context`, which SHALL be admitted at the next Safe Boundary.

#### Scenario: Async notification hook
- **WHEN** a `Notification` hook with `async: true` posts to a chat webhook taking 5 s
- **THEN** the session is not delayed

### Requirement: Parallel execution within a group
(P1) Handlers matching the same event SHALL run concurrently up to `hooks.concurrency` (default 8). Decision merging SHALL still follow declared order, regardless of completion order. Identical `command` strings matched in multiple scopes SHALL run once per event.

#### Scenario: Duplicate command deduplicated
- **WHEN** the same `command` hook is defined in global and project scope
- **THEN** it executes once for each event

### Requirement: Sandboxing of command hooks
(P1) Command hooks from project, local and plugin scopes SHALL run inside the sandbox profile `hooks` (workspace read-write, network per `sandbox.network`). Managed and global hooks SHALL run unsandboxed unless `hooks.sandbox_all: true`.

#### Scenario: Project hook cannot write home
- **WHEN** a project `PostToolUse` command hook tries to write `~/.ssh/config`
- **THEN** the write fails with a sandbox denial and the hook reports a non-blocking error

### Requirement: Trust for project hooks
(P1) The system SHALL require explicit user trust before running project-scope or local-scope hooks. Trust SHALL use the checkout-scoped workspace-trust store and the SHA-256 of each handler definition. A new or changed handler SHALL be skipped and reported as `untrusted` until approved via `/hooks` or `cyber hooks trust`. In `exec` mode, untrusted hooks SHALL be skipped unless `--trust-project-hooks` explicitly approves the currently inspected handler digests for that invocation, without approving future changes.

#### Scenario: Changed hook requires re-trust
- **WHEN** a teammate changes `.cyber/cyber.jsonc` hook command after the user trusted it
- **THEN** the hook is skipped and the TUI shows `1 untrusted hook changed — review with /hooks`

### Requirement: Hooks viewer and CLI
(P1) The system SHALL provide `/hooks` in the TUI and `cyber hooks list|trust|untrust|test <event>` on the CLI. These SHALL show every hook with its scope, event, matcher, type, trust state, last run time and last result. `cyber hooks test` SHALL run matching hooks against a synthetic or `--payload <file>` event and print their decisions without affecting any session.

#### Scenario: Dry-run a hook
- **WHEN** the user runs `cyber hooks test PreToolUse --payload ev.json`
- **THEN** each matching handler runs and its parsed decision is printed

### Requirement: Hook context injection on lifecycle events
(P1) `SessionStart` and `UserPromptSubmit` hooks MAY return `additional_context`. The system SHALL admit it as a Mid-Conversation System Message tagged with the hook id. For `UserPromptSubmit`, a `deny` decision SHALL reject the prompt before admission and show the reason to the user.

#### Scenario: Inject ticket context
- **WHEN** a `UserPromptSubmit` hook returns `additional_context` with the linked Jira ticket text
- **THEN** the model sees that context before the user's prompt in the same Turn

#### Scenario: Block secrets in prompt
- **WHEN** a `UserPromptSubmit` hook denies a prompt containing an API key
- **THEN** the prompt is not admitted and the user sees the reason

### Requirement: Hook observability
(P1) Every hook execution SHALL be recorded as a durable `hook.executed.1` event with hook id, event, scope, duration, outcome (`ok`, `blocked`, `error`, `timeout`, `skipped`) and decision. Execution SHALL NOT record stdin or stdout contents unless `telemetry.log_hook_io` is true. Outcomes SHALL be visible in the transcript when a hook blocks or modifies an action.

#### Scenario: Blocked action visible
- **WHEN** a hook blocks an `edit`
- **THEN** the transcript shows `blocked by hook <id>: <reason>` and a `hook.executed.1` event with outcome `blocked` is stored

### Requirement: Hooks in subagents and workflows
(P2) Hooks SHALL fire for tool calls made by subagents, workflow agents, goal continuations and loop iterations with the same configuration as the parent Location. The envelope SHALL carry `parent_session_id`, `workflow_run_id` (prefix `run_`), `goal_id` (prefix `gol_`) or `loop_id` (prefix `lop_`) when applicable.

#### Scenario: Workflow agent edit triggers hook
- **WHEN** an agent inside workflow run `run_42` edits a file
- **THEN** `PostToolUse` hooks receive `workflow_run_id: "run_42"`
