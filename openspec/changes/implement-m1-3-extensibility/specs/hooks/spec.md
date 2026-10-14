## MODIFIED Requirements

### Requirement: MCP tool handlers
(P1) An `mcp_tool` handler SHALL call `tool` on the configured MCP `server` with the event JSON as `input` (or the handler's `arguments` template with `${field}` substitution) and SHALL parse a JSON object in the result's text content as a decision.

#### Scenario: Audit via MCP
- **WHEN** a `PostToolUse` handler is `{ "type": "mcp_tool", "server": "audit", "tool": "record" }`
- **THEN** the `audit` server's `record` tool is called with the event payload after each tool call

#### Scenario: Typed templates and unambiguous textual policy
- **WHEN** an MCP handler arguments template references protected event fields
- **THEN** exact placeholders SHALL preserve JSON types and embedded placeholders SHALL become text
- **AND** missing fields or a nonobject argument result SHALL be refused before the call
- **AND** failed, missing, oversized or ambiguous text decision objects SHALL NOT authorize the guarded action

#### Scenario: Framed MCP requests retain unresolved authority
- **WHEN** a framed MCP request times out, is disposed or has invalid/partial framing
- **THEN** pending identity SHALL remain unresolved and another request SHALL NOT borrow its execution
- **AND** explicit cancellation SHALL reference only a sent pending non-initialize request without claiming remote/native acknowledgement
- **AND** incoming/outgoing RPC messages SHALL remain bounded to 1 MiB
- **AND** only matching numeric progress SHALL extend request inactivity
- **AND** callbacks not advertised by the client SHALL NOT execute

#### Scenario: Native stdio ownership survives protocol interruption
- **WHEN** an already authorized stdio server writes excess stderr or its request/initialization is interrupted
- **THEN** stderr SHALL drain concurrently with at most 1 MiB retained
- **AND** explicit shutdown SHALL close protocol IO and report bounded native tree termination acknowledgement separately from the RPC outcome
- **AND** failed initialization SHALL retain stop evidence and disposal SHALL NOT imply acknowledgement

#### Scenario: Dedicated local hook connection
- **WHEN** a trusted synchronous local MCP-tool hook runs
- **THEN** its hook receipt SHALL own a dedicated connection with current handler/server authorization and required scope sandboxing
- **AND** discovered input schemas and server tool filters SHALL apply before tools/call
- **AND** native/process/proxy shutdown SHALL be acknowledged before durable settlement permits scratch cleanup
- **AND** startup/discovery and active-call cancellation SHALL NOT dispose native ownership without a verified or explicitly Unknown result
- **AND** callbacks SHALL NOT borrow a Session's requester or private context

### Requirement: Prompt handlers
(P1) A `prompt` handler SHALL ask the `model_roles.evaluator` model (falling back to `small_model`) to judge the event using the handler's `prompt` text plus the event JSON. The judgement SHALL be returned as structured output `{ decision: allow|deny|ask, reason }`. Prompt handlers SHALL have no tools and SHALL count toward session cost.

#### Scenario: LLM judges a bash command
- **WHEN** a `PreToolUse` prompt hook asks "Deny commands that delete data outside the repo" and the model returns `deny`
- **THEN** the bash call is denied with the model's reason

#### Scenario: Evaluator overlays cannot grant tools or replace evidence
- **WHEN** a trusted prompt handler uses a model with configured request overlays
- **THEN** the request SHALL keep the trusted policy separate from protected flat event evidence and force tools disabled after overlays
- **AND** the bounded response SHALL require a complete tool-free stream and strict decision/reason object
- **AND** unexpected tool calls, incomplete finish and invalid decisions SHALL NOT authorize the guarded action

#### Scenario: Observed prompt usage survives cancellation
- **WHEN** a Session prompt evaluator reports usage before cancellation or timeout
- **THEN** observed usage SHALL be billed through hidden Session and ancestor accounting
- **AND** budget ceilings SHALL be checked before inference and before returning permission to proceed
- **AND** missing local transport-shutdown acknowledgement SHALL retain unknown receipt/activity evidence rather than assert remote completion or final billing

#### Scenario: Native prompt transport settlement and raw response bounds
- **WHEN** a prompt hook uses a native OpenAI Responses, OpenAI-compatible or Anthropic adapter
- **THEN** it SHALL use an explicit owned proxy/client with ambient proxies, redirects and automatic retries disabled
- **AND** raw SSE and non-2xx bodies SHALL be bounded to 1 MiB before decoder/error buffering
- **AND** timeout SHALL close and join local sockets before applying the configured nonblocking/fail-closed policy
- **AND** subtree cancellation SHALL retain the execution until local transport shutdown is acknowledged
- **AND** disposal SHALL close owned sockets while retaining unknown receipt/activity evidence
- **AND** local acknowledgement SHALL NOT claim remote processing rollback or final provider usage

### Requirement: Hook configuration
(P1) The system SHALL read hooks from the `hooks` config key as a map from event name to an ordered array of hook groups `{ matcher?, hooks: [handler, ...] }`, where each handler has a required `type` (`command`, `http`, `prompt`, or `mcp_tool`) and optional `timeout` (seconds), `async` (boolean), `id` and `description`. Unknown event names SHALL fail config validation with the path of the offending key.

#### Scenario: Valid PreToolUse hook
- **WHEN** `cyber.jsonc` contains a `PreToolUse` group with matcher `bash` and a command handler
- **THEN** the definition SHALL resolve with its declared matcher and command

#### Scenario: Unknown event rejected
- **WHEN** a configuration declares `hooks.BeforeEverything`
- **THEN** configuration loading SHALL fail with an invalid-config error naming that key

#### Scenario: Invalid handler contract rejected
- **WHEN** a trusted hook has an unsupported type, a missing type-specific field, an invalid selector or a timeout outside 1 through 600 seconds
- **THEN** configuration loading SHALL fail with the hook definition path
- **AND** validation SHALL execute no handlers

### Requirement: Hook scopes and merge order
(P1) The system SHALL collect hooks from the managed, global (`~/.config/cyber`), project (`cyber.jsonc` and `.cyber/` files) and local (`.cyber/cyber.local.jsonc`) scopes plus enabled plugins, and SHALL run every matching hook from every scope rather than letting one scope replace another. Hooks SHALL execute in the order managed, global, project, local, plugin. When `policy.hooks.managed_only` is true, only managed-scope hooks SHALL run.

#### Scenario: Hooks from two scopes both run
- **WHEN** global and project scopes each define a `PostToolUse` hook matching `edit`
- **THEN** both contributions SHALL survive configuration resolution and run in scope order

#### Scenario: Per-handler origins remain available
- **WHEN** hook groups from different configuration files are appended
- **THEN** every group and handler SHALL retain its own source label at its resolved index
- **AND** an empty later event array SHALL NOT erase earlier groups

#### Scenario: Managed-only policy
- **WHEN** the organization policy sets `hooks.managed_only: true`
- **THEN** project, local, global and plugin hooks SHALL be skipped and a single notice SHALL list how many were skipped
- **AND** invocation hooks from `--config` or `CYBER_CONFIG*` SHALL also be skipped, and each matching skipped handler SHALL leave a `skipped` receipt whose reason is `managed-only policy`
- **AND** the notice SHALL be shown once per event per Session as `N hooks skipped by managed-only policy`
- **AND** `/hooks`, `cyber hooks list` and `GET /hooks` SHALL list the skipped definitions with that reason, from the same catalog dispatch uses

#### Scenario: Managed policy source
- **WHEN** the OS-managed directory (`/etc/cyber`, `/Library/Application Support/cyber` on macOS, `%ProgramData%\cyber` on Windows) holds `cyber-policy.json` or `cyber-policy.jsonc`
- **THEN** its `hooks` event arrays SHALL resolve with `managed:<file>` origins in the managed scope, run first and unsandboxed unless `hooks.sandbox_all`, need no handler approval and never be substituted
- **AND** `policy.hooks.managed_only` SHALL be read only from that document; `policy` in user, project, profile and CLI layers SHALL never restrict or relax hooks
- **AND** on Unix a document not owned by root SHALL be ignored with the warning `managed policy file not root-owned; ignored`, and a root-owned document writable by group or others with `managed policy file writable by non-root users; ignored`
- **AND** on Windows `%ProgramData%` SHALL come from the known-folder store, not the environment, and a document SHALL be ignored with the warning `managed policy file not admin-owned; ignored` unless the file and its `cyber` directory are owned by SYSTEM, Administrators or TrustedInstaller, neither has a NULL DACL, no other principal may write, append to, delete or re-ACL the file, and no other principal may delete, delete children of or re-ACL the directory
- **AND** `CYBER_MANAGED_POLICY_DIR` MAY name another directory for tests and packaging, and SHALL be ignored with a warning while the OS-managed directory holds a policy document
- **AND** a managed `mcp_tool` handler's server, and a managed `prompt` handler's evaluator reference and its provider, SHALL be defined by global configuration alone; when any project, profile, `--config` or `CYBER_CONFIG*` layer contributes to them the handler SHALL be unavailable (refusing when `fail_closed`) and SHALL never call the shadowing server or model

#### Scenario: Malformed managed policy fails closed
- **WHEN** a managed policy document cannot be read or parsed, is not an object, sets `policy.hooks.managed_only` to a non-boolean, uses a top-level key other than `$schema`, `hooks` or `policy`, a `policy` key other than `hooks` or an unsupported `policy.hooks` key, or holds an invalid or non-event `hooks` entry
- **THEN** no managed hook from it SHALL run and every non-managed hook SHALL be skipped as though `managed_only` were set
- **AND** the notice, receipts and review SHALL name the reason `managed policy is malformed (<diagnostic>)`

#### Scenario: Selected profile preserves hook origins
- **WHEN** global, project and local files contribute hooks to the same selected profile
- **THEN** all contributions SHALL be appended to ordinary hook definitions
- **AND** every selected group, handler and handler field SHALL retain its defining file origin rather than a generic profile label
- **AND** untrusted or changed project definitions SHALL remain withheld and an empty later profile event array SHALL NOT erase earlier definitions

### Requirement: Trust for project hooks
(P1) The system SHALL require explicit user trust before running project-scope or local-scope hooks. Trust SHALL use the checkout-scoped workspace-trust store and the SHA-256 of each handler definition. A new or changed handler SHALL be skipped and reported as `untrusted` until approved via `/hooks` or `cyber hooks trust`. In `exec` mode, untrusted hooks SHALL be skipped unless `--trust-project-hooks` explicitly approves the currently inspected handler digests for that invocation, without approving future changes.

#### Scenario: Changed hook requires re-trust
- **WHEN** a teammate changes `.cyber/cyber.jsonc` hook command after the user trusted it
- **THEN** the hook is skipped and the TUI shows `1 untrusted hook changed — review with /hooks`

#### Scenario: Untrusted skips are aggregated and recorded
- **WHEN** one or more matching project or local handlers are skipped as untrusted during a Session
- **THEN** the Session SHALL receive one notice `N untrusted hook(s) changed — review with /hooks` (singular `hook` for one), where N counts every resolved handler still awaiting approval
- **AND** each skipped handler SHALL leave a settled `skipped` receipt with reason `untrusted` that claims no `once` slot

#### Scenario: Exec approves inspected hooks for one run
- **WHEN** the user runs `cyber exec --embedded --trust-project-hooks` (or `--ephemeral`) in a checkout whose configuration is approved
- **THEN** only the project/local handler digests resolved at startup SHALL run for that invocation, and no handler approval SHALL be stored
- **AND** the flag SHALL be refused as a usage error without `--embedded` or `--ephemeral`, because a shared server cannot take an approval for one run

#### Scenario: Workspace approval does not approve handlers
- **WHEN** a checkout's configuration is approved but its hook handler digest is not
- **THEN** the shared trust store SHALL report that handler as unapproved
- **AND** adding a handler approval SHALL preserve the workspace approval and other approved handler digests

#### Scenario: Invocation approval remains temporary
- **WHEN** an invocation approves inspected handler digests
- **THEN** only those digests in that canonical checkout SHALL be approved for that invocation
- **AND** changed definitions and other checkouts SHALL remain unapproved
- **AND** no durable handler approval SHALL be written

#### Scenario: Shared trust storage is serialized
- **WHEN** multiple processes update workspace and handler approvals concurrently
- **THEN** all independent approvals SHALL survive in the private shared store
- **AND** legacy workspace-only files SHALL remain readable
- **AND** checkout revocation SHALL remove that checkout's workspace and handler approvals

### Requirement: Hooks viewer and CLI
(P1) The system SHALL provide `/hooks` in the TUI and `cyber hooks list|trust|untrust|test <event>` on the CLI. These SHALL show every hook with its scope, event, matcher, type, trust state, last run time and last result. `cyber hooks test` SHALL run matching hooks against a synthetic or `--payload <file>` event and print their decisions without affecting any session.

#### Scenario: Dry-run a hook
- **WHEN** the user runs `cyber hooks test PreToolUse --payload ev.json`
- **THEN** each matching handler runs and its parsed decision is printed

#### Scenario: Bounded payload loading and explicit synthetic test outcomes
- **WHEN** the user invokes `cyber hooks test <event>` with an optional payload file
- **THEN** the CLI SHALL load a regular JSON file of at most 1 MiB containing an object of event-specific fields, or use an empty object when no file is supplied
- **AND** malformed, oversized, nonobject, spoofed-envelope and unknown-event inputs SHALL be rejected before opening the execution database
- **AND** the CLI SHALL use standalone host ownership without application startup, model calls or Session recovery
- **AND** per-handler and merged decisions SHALL be reported in declared order, including explicit skipped/untrusted and unsupported/error outcomes
- **AND** an `async` handler SHALL run to completion and be reported as a background handler (`background: true`) whose decision contributes only `additional_context` to the merged decision, rather than as unsupported or incomplete
- **AND** incomplete execution, unknown termination or unsupported handlers SHALL produce a nonzero exit status rather than claim successful execution
- **AND** Ctrl-C SHALL signal cancellation and retain the execution future until launched process owners settle

#### Scenario: Inspect and approve a resolved handler
- **WHEN** a checkout's configuration is trusted and the user lists hooks
- **THEN** resolved handlers SHALL show source, scope, definition digest and individual trust state with credentials redacted
- **AND** trust SHALL approve only a currently resolved project/local definition digest
- **AND** a changed definition SHALL remain unapproved

#### Scenario: Revoke an obsolete handler
- **WHEN** the user untrusts a previously approved handler digest after the definition is removed or current configuration becomes malformed
- **THEN** the approval SHALL be revoked without resolving executable configuration

#### Scenario: CLI execution history does not reconcile owners
- **WHEN** the user runs `cyber hooks history --session <id>` with optional page limit and cursor
- **THEN** the CLI SHALL read committed execution observations without launching handlers or rebuilding runtime owners
- **AND** malformed executable configuration SHALL NOT prevent inspection
- **AND** pages SHALL preserve Session scope, receipt IO policy and unresolved statuses without adding durable events
- **AND** text output SHALL omit raw IO and distinguish recorded running observations from verified live processes

#### Scenario: TUI receipt inspection preserves unresolved observations
- **WHEN** the user opens `/hooks history` in the current Session
- **THEN** the viewer SHALL page authenticated committed receipts and support refresh, scrolling and dismissal
- **AND** raw IO SHALL be excluded from viewer state and display
- **AND** running and unknown labels SHALL NOT imply verified live execution or authorize replay
- **AND** superseded, dismissed or foreign-Session responses SHALL NOT replace the visible observation page

#### Scenario: Authenticated Location definition catalog
- **WHEN** a client reads GET `/hooks` for a Location
- **THEN** the API SHALL require authentication and return freshly resolved definitions with their source, scope, original digest, current trust state and sandbox requirements
- **AND** credentials in headers and recognized secret fields SHALL be redacted without changing the original approval digest
- **AND** untrusted checkout definitions SHALL remain withheld and their paths SHALL be reported separately
- **AND** inspection SHALL NOT admit Session, hook execution or model work

#### Scenario: Synthetic execution receipts remain independent of Sessions
- **WHEN** a synthetic hook execution is admitted and settled
- **THEN** its `hook.started.1` and `hook.executed.1` facts SHALL mark synthetic identity and use a separate projection without creating or binding a Session
- **AND** a real Session identity or borrowed Session admission bindings SHALL be refused atomically
- **AND** Session sequences, receipt history and ordinary last-run observations SHALL remain unchanged
- **AND** IO policy and identity SHALL be pinned at admission and immutable through settlement
- **AND** disposal without acknowledged settlement SHALL preserve unknown outcome and mandatory-stop observations without raw IO
- **AND** once claims SHALL be exclusive within one synthetic invocation identity and SHALL NOT affect a Session's once claims

#### Scenario: Synthetic payloads cannot borrow execution identity
- **WHEN** a hook test event is constructed from caller-supplied JSON
- **THEN** the system SHALL require an object containing event-specific fields and generate a fresh synthetic invocation identity
- **AND** event, Session identity, Location, project, agent, Mode, timestamp and the synthetic marker SHALL come from the test invocation rather than the payload
- **AND** attempts to supply those protected fields SHALL be rejected, including attempts to set the synthetic marker on an ordinary event
- **AND** chained tool-input rewrites SHALL preserve the generated identity and synthetic marker without modifying prior event input

#### Scenario: Synthetic commands retain native checkout ownership
- **WHEN** a trusted matching command handler runs against a synthetic event in a managed checkout
- **THEN** it SHALL acquire a receipt-specific checkout lease before launch without constructing or binding a Session
- **AND** concurrent handlers in the same test invocation SHALL use distinct lease identities
- **AND** removal SHALL remain refused until native termination is acknowledged and ownership is settled
- **AND** cancellation SHALL stop the owned process tree and record its acknowledged or unknown result
- **AND** disposal without acknowledgement SHALL retain unknown receipt and checkout activity evidence, including when forced removal is requested
- **AND** repeated once handlers SHALL be skipped within that test invocation without recording another admission or launching another command

#### Scenario: Last-run observations match the current handler and checkout
- **WHEN** a client inspects a loaded hook through API/SDK, CLI list or TUI
- **THEN** the system SHALL show the latest recorded execution with the same effective handler digest, event and scope in the current canonical checkout
- **AND** ordering SHALL use admission time and receipt id, including newer running/unknown observations rather than falling back to an older completed result
- **AND** correlation SHALL use the receipt's captured execution Location, not the Session's rebound directory
- **AND** summaries SHALL omit raw IO and decision content even when receipt IO logging was enabled
- **AND** running/unknown observations SHALL remain explicitly unverified/recovery-required and inspection SHALL NOT launch handlers, reconcile owners or admit durable events
- **AND** an absent local database SHALL NOT be created merely to inspect last-run metadata

#### Scenario: Review withheld hook sections without interpretation
- **WHEN** checkout configuration is untrusted and the user inspects hooks through the catalog API, CLI list or TUI viewer
- **THEN** original project/local top-level and profile hook sections SHALL be available as redacted literal JSON with file origins and escaped JSON pointers
- **AND** host environment and file substitutions, handler validation and execution SHALL NOT run for this review
- **AND** malformed handler schemas SHALL remain inspectable without activation
- **AND** these raw sections SHALL NOT receive executable handler digests or individual approval controls
- **AND** disabled project configuration SHALL remain undiscovered and approved checkout sections SHALL NOT be duplicated as withheld

#### Scenario: TUI definition review and exact-digest confirmation
- **WHEN** the user opens `/hooks`
- **THEN** the viewer SHALL inspect the current Location's loaded definitions, file origins, selectors, redacted handlers, original digests and trust/sandbox requirements through authenticated public APIs
- **AND** withheld checkout paths SHALL be reported without interpreting their handlers
- **AND** changing a project/local approval SHALL require confirmation of the displayed selected digest and reload current metadata after success
- **AND** refresh, dismissal or Session/Location changes SHALL invalidate pending confirmation and stale responses
- **AND** other scopes SHALL NOT offer individual checkout approvals

#### Scenario: Authenticated exact-digest trust mutation
- **WHEN** a client posts a digest to `/hooks/trust` for a Location
- **THEN** the API SHALL require authentication and current checkout trust and approve only a freshly resolved project/local digest
- **AND** global, unknown, obsolete or withheld digests SHALL be refused
- **AND** unknown body fields SHALL be refused
- **WHEN** a client posts an obsolete digest to `/hooks/untrust`
- **THEN** revocation SHALL remain checkout-scoped and available with malformed configuration, reporting whether an approval existed
- **AND** neither operation SHALL execute handlers or admit Session, hook execution or model work

### Requirement: Matchers
(P1) The system SHALL match hook groups by `matcher`, which is either a glob over the event's subject (tool name for tool events, including `mcp__<server>__<tool>`; notification type; file path for `FileChanged`) or, when wrapped in `/.../`, a regular expression. An absent or `*` matcher SHALL match every subject. A group MAY add `paths` (globs relative to the Location) that SHALL also match for tool calls with file targets.

#### Scenario: Regex matcher for MCP tools
- **WHEN** a hook group has matcher `/^mcp__github__.*/`
- **THEN** it SHALL match `mcp__github__create_issue` and not `bash`

#### Scenario: Path filter
- **WHEN** a `PreToolUse` group has matcher `edit` and `paths: ["migrations/**"]`
- **THEN** it SHALL match edits under `migrations/` only
- **AND** absolute or escaping path candidates SHALL NOT satisfy a Location-relative path filter

### Requirement: Decision schema
(P1) A hook decision SHALL be a JSON object with optional fields: `decision` (`allow`, `deny`, `ask`), `reason`, `updated_input` (PreToolUse only, replaces tool input after re-validation against the tool schema), `additional_context` (text admitted as a system message at the next Safe Boundary; for `PreCompact` it is appended to the summary instructions instead), `continue` (false stops the Drain after the current Turn), `stop_reason`, and `suppress_output` (hide the hook's output from the transcript). Fields not valid for the event SHALL be ignored with a debug log. The explicit Stop `block` and PermissionDenied `retry`/`updated_input` contracts SHALL remain valid for those events.

#### Scenario: Input rewritten
- **WHEN** a `PreToolUse` hook returns `{"updated_input": {"command": "npm test -- --ci"}}` for a bash call
- **THEN** the bash tool SHALL run `npm test -- --ci` and the transcript SHALL show the rewrite

#### Scenario: Invalid rewritten input
- **WHEN** `updated_input` fails the tool's input schema
- **THEN** the call SHALL be denied with `hook produced invalid tool input`

#### Scenario: Event-specific fields
- **WHEN** a PostToolUse decision contains updated_input or a non-Stop decision contains block
- **THEN** those fields SHALL be ignored and identified for debug diagnostics
- **AND** malformed applicable decision fields SHALL fail decision validation

#### Scenario: Tool-event context commits with the call's settlement
- **WHEN** `PreToolUse`, `PermissionRequest`, `PostToolUse` or `PostToolUseFailure` handlers return `additional_context` for a call
- **THEN** each handler's context SHALL be tagged `[hook <id>]`, accumulated in declared order across the call's events, and committed as a system message with reason `hook_context` in the same commit as the call's settlement, immediately after it
- **AND** the model SHALL see it from the next request on, and a recreated runtime SHALL replay it without admitting it again
- **AND** returning context SHALL NOT make a `fail_closed` handler unavailable

#### Scenario: Stop request ends the Drain after the current Turn
- **WHEN** the merged decision of a tool event, `Stop` or `SessionStart` carries `continue: false`
- **THEN** the Drain SHALL end after the current Turn: a tool event's remaining calls in that Turn still run and settle and no further request is made, a `Stop` SHALL admit no continuation even with `decision: block`, and `SessionStart` SHALL end it once the first Turn completes (durably, across a restart)
- **AND** the `stop_reason` (or `continue: false`), tagged `[hook <id>]`, SHALL be recorded durably as a system message with reason `hook_stop` and shown as a transient notice from that hook
- **AND** events that cannot apply context or a stop request yet (including `UserPromptSubmit` `continue: false`) SHALL leave a notice, and a `fail_closed` handler SHALL refuse rather than drop it

#### Scenario: Suppressed output
- **WHEN** a handler's decision sets `suppress_output: true`
- **THEN** its `system_message` and its blocked/rewrite notices SHALL NOT be shown
- **AND** its durable receipt, the model-facing deny reason and any stop reason SHALL remain, and diagnostics for failures SHALL still be shown

### Requirement: Decision merging
(P1) When several hooks return decisions for one event, the system SHALL apply them in execution order and combine them: any `deny` SHALL win over `ask`, and `ask` SHALL win over `allow`. `updated_input` SHALL chain, with each later hook receiving the previous hook's output. `additional_context` values SHALL be concatenated. Any `continue: false` SHALL stop continuation.

#### Scenario: Deny beats allow
- **WHEN** one hook returns `allow` and a later hook returns `deny`
- **THEN** the action SHALL be denied with the later hook's reason
- **AND** another later allow SHALL NOT erase that denial

#### Scenario: Ordered context and continuation
- **WHEN** successive hooks add context, rewrite input and set continue false
- **THEN** context SHALL retain declared order and the next hook SHALL receive the rewritten input
- **AND** a later continue true SHALL NOT erase the stop request

### Requirement: Interaction with permissions
(P1) `PreToolUse` hooks SHALL run before permission evaluation. A hook `deny` SHALL block even in `bypass` mode. A hook `allow` SHALL skip the `ask` prompt but SHALL NOT override a permission rule whose effect is `deny`. `PermissionRequest` hooks SHALL run when a request would prompt the user and MAY answer it with `allow` or `deny`.

#### Scenario: Hook cannot override deny rule
- **WHEN** a hook returns `allow` for `bash` and the permission rules deny `bash` for `rm -rf *`
- **THEN** the call is denied by the permission rule

#### Scenario: Auto-answer permission prompt
- **WHEN** a `PermissionRequest` hook returns `allow` for an `edit` under `docs/**`
- **THEN** no prompt is shown and the request is approved once

#### Scenario: Permission request command approves only the current ask
- **WHEN** an allowed built-in invocation requires approval and a matching command PermissionRequest hook returns allow
- **THEN** the current ask SHALL be approved without publishing a pending user request or saving a permission rule
- **AND** the event SHALL carry the captured invocation identity, final tool input and current permission ask
- **AND** updated_input SHALL be ignored for this event

#### Scenario: Permission request cannot override a hard denial
- **WHEN** ordinary deny rules or a hard Mode ceiling refuse a built-in invocation
- **THEN** PermissionRequest handlers SHALL NOT execute to approve that refused action

#### Scenario: Explicit hook ask preserves auto block ceilings
- **WHEN** a PreToolUse command returns ask and current or ancestor auto-mode policy always-blocks the action
- **THEN** the call SHALL remain blocked before any PermissionRequest handler can allow it

### Requirement: Supported events
(P1) The system SHALL emit hook events `Setup` (first Session in a Location after install or `cyber --init`), `SessionStart`, `SessionEnd`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `PostToolBatch` (after all calls of one Turn settle), `PermissionRequest`, `PermissionDenied` (MAY return `{ "decision": "retry", "updated_input" }` to re-issue the call once), `Stop`, `StopFailure`, `Interrupt`, `SubagentStart`, `SubagentStop`, `PreCompact`, `PostCompact`, `InstructionsLoaded` (an instruction or rule file entered the context), `PreModelSwitch`, `PostModelSwitch`, `Elicitation`, `ElicitationResult`, `DirectoryAdded`, `Notification`, `FileChanged`, `CwdChanged`, `ConfigChange`, `TaskCreated`, `TaskCompleted`, `WorktreeCreate`, `WorktreeRemove` and `JobEnded`, and (P2) `GoalEvaluated`, `GoalCompleted`, `WorkflowRunStart`, `WorkflowRunEnd`, `LoopIteration`, `ScheduleRun`, `TeammateIdle` and `MessageReceived`. Each event SHALL carry a common envelope `{ event, session_id, location: { directory, workspace? }, project_id, agent, mode, timestamp }` plus event-specific fields.

#### Scenario: PostToolUse payload
- **WHEN** an `edit` tool call completes successfully
- **THEN** each matching `PostToolUse` hook receives the envelope plus `tool_name`, `tool_input`, `tool_output`, `call_id` and `duration_ms`

#### Scenario: MessageReceived for cross-session messages
- **WHEN** another session's message is delivered into session `ses_1`
- **THEN** `MessageReceived` hooks run with `from_session`, `from_machine` and `text` before the message is shown to the model

#### Scenario: Denied call retried by a hook
- **WHEN** a `PermissionDenied` hook returns `{ "decision": "retry", "updated_input": { "command": "npm test -- --ci" } }`
- **THEN** the rewritten call is evaluated once more and runs if the rules allow it

#### Scenario: Payload cannot replace captured identity
- **WHEN** event-specific data contains event, session_id, location, project_id, agent, mode, timestamp or parent_session_id
- **THEN** envelope construction SHALL refuse that payload rather than overwriting the captured identity

#### Scenario: Envelope carries the Session's project identity
- **WHEN** a hook event is built for a Session whose Location is in a git checkout
- **THEN** `project_id` SHALL be that checkout's project ID (`global` outside git), `location.workspace` SHALL be the checkout root, and a child Session's events SHALL carry `parent_session_id` (absent for a root Session)
- **AND** the identity SHALL be resolved from the Session's Location off the async runtime once per Location, and only when a definition exists for the event
- **AND** durable hook admission SHALL refuse an event whose project, workspace or parent differs from the Session's own, and the receipt SHALL record them so replay revalidates and settlement cannot rebind them

#### Scenario: Runtime lifecycle events with an existing producer
- **WHEN** a Session is interrupted, switches model, compacts, settles a Turn's tool calls, is archived or deleted, registers or settles a background Job, or loads instruction files into its context
- **THEN** the runtime SHALL emit `Interrupt` (`running`), `PreModelSwitch`/`PostModelSwitch` (`from`, `to`, `automatic`; the target model as matcher subject), `PreCompact`/`PostCompact` (`trigger` as matcher subject, `instructions`; `tokens_before`/`tokens_after` after), `PostToolBatch` (`message_id` and each call's `call_id`, `tool_name` and `status`), `SessionEnd` (`reason` `archive` or `delete`), `TaskCreated`/`TaskCompleted` (the Job JSON as `job`, `status`; the Job kind as matcher subject) and `InstructionsLoaded` (`source`, `load_reason` `session_start` or `changed`, `file_paths`, `digest`) through the runtime lifecycle-hook port
- **AND** each hook SHALL be captured under the Session lock and awaited only after it and the runtime lifecycle guard are released
- **AND** `PreModelSwitch` `deny` SHALL refuse a user switch with `blocked by hook <id>: <reason>` and record nothing, while an automatic profile-driven switch SHALL only be observed
- **AND** `PreCompact` `additional_context` SHALL be appended to the summary instructions and never admitted as a message, and an unavailable `fail_closed` `PreCompact` handler SHALL refuse the compaction
- **AND** `PostToolBatch` SHALL run once per Turn after every group settled, interrupted calls included; its context SHALL commit as a `hook_context` system message, its `continue: false` SHALL end the Drain after the Turn, and an unavailable `fail_closed` handler SHALL fail the Drain
- **AND** `PostToolBatch` SHALL run Drain-owned so an interrupt cancels its handlers cooperatively; a Drain interrupted while it runs SHALL commit nothing from it, and after an interrupt it SHALL only be observed, in a runtime-owned task the interrupt never waits for
- **AND** a Session deletion SHALL fence the Session and its descendants against new Drains and child Jobs and make every refusal before `SessionEnd`, so hooks observe only a deletion that then happens, once; before deleting it SHALL cancel and await the subtree's in-flight hook executions
- **AND** `InstructionsLoaded` `file_paths` SHALL list the files actually read for the source, never paths parsed from instruction content
- **AND** `InstructionsLoaded` SHALL run once per changed instruction source text, when the source first enters a Session's context or changes
- **AND** `Interrupt`, `PostModelSwitch`, `PostCompact`, `SessionEnd`, `TaskCreated`, `TaskCompleted` and `InstructionsLoaded` SHALL be observational: decisions SHALL be ignored and handler failures, including an unavailable `fail_closed` handler, SHALL only be logged and never block the interrupt, switch, compaction, archive, deletion or Job
- **AND** an interrupt, Job stop or shutdown SHALL cancel in-flight Safe Boundary (`PreModelSwitch`, `PostModelSwitch`, `InstructionsLoaded`, `PreCompact`, `PostCompact`), `TaskCreated`, `Elicitation` and `ElicitationResult` handlers cooperatively and let their receipts settle rather than dropping a launched handler, so the Session stays interruptible without recovery; a Job SHALL stop waiting for its `TaskCreated` handlers when cancelled
- **AND** `WorktreeCreate` handlers and the worktree permission prompt SHALL run before runtime shutdown admission is held for the git effects
- **AND** `Interrupt` SHALL NOT run for an owner-initiated child interrupt, `TaskCreated` SHALL be abandoned rather than awaited when its Job is cancelled or the runtime shuts down, and `TaskCompleted` SHALL follow `JobEnded`'s suppression rules, running immediately before it

#### Scenario: Events deferred for lack of a producer
- **WHEN** the P1 event set is compared with the runtime's producers
- **THEN** `Setup`, `DirectoryAdded`, `Notification`, `FileChanged`, `CwdChanged` and `ConfigChange` SHALL remain unemitted until a first-run marker, directory addition, notification, file watcher, working-directory change or configuration reload producer exists
- **AND** `WorktreeRemove`, whose producer exists, SHALL remain unemitted until it is wired to the same dispatch, and `Notification` SHALL be emitted only for the permission-prompt and question producers until idle, authentication and other notification producers exist

#### Scenario: Tool-side events with an existing producer
- **WHEN** an attended permission prompt or `question` tool question is about to be published, an MCP server sends a form elicitation to an attended call, or a worktree is created
- **THEN** `Notification` (`notification_type` `permission_prompt` or `question` as matcher subject, `message`, `tool_name`, `call_id`) SHALL run before the prompt is published, and SHALL be observational: decisions SHALL be ignored and no handler failure SHALL block or withhold the prompt
- **AND** `Elicitation` (`mcp_server_name` as matcher subject, `message`, `requested_schema`, `elicitation_mode`) SHALL run before the user is asked; a `deny`, or an unavailable `fail_closed` handler, SHALL decline the request without asking the user
- **AND** `ElicitationResult` (`mcp_server_name`, `action`, `content`) SHALL observe the reply about to be sent, including a hook decline, and SHALL ignore decisions
- **AND** `WorktreeCreate` (`name` as payload field and matcher subject) SHALL run after worktree permission and before any git effect; a `deny` or an unavailable `fail_closed` handler SHALL refuse the creation with `blocked by hook <id>: <reason>`, while `allow` and `ask` SHALL neither grant nor prompt

#### Scenario: PermissionDenied runs at denial boundaries and retries once
- **WHEN** a tool call is refused by a deny rule or Mode ceiling (`denied_by` `rule`), an auto-mode block (`auto`), a user rejection (`user`) or the lack of an approver (`unattended`)
- **THEN** `PermissionDenied` SHALL run once for the call with the tool fields, `reason`, `denied_by` and the refused `permission` (`action`, `resources`), chaining `updated_input` like `PreToolUse`
- **AND** a merged `retry` (which any later `ask` or `deny` overrides) SHALL re-issue the call once with its `updated_input`, or the denied input when absent, after tool-schema validation (`hook produced invalid tool input` on failure), the auto-mode override check and full permission evaluation; a `PreToolUse` `allow` for the original input SHALL NOT carry over, while an `ask` SHALL
- **AND** when the re-issued input differs from the denied input, `PreToolUse` SHALL run on it first like on a first attempt: its `deny` or `block` SHALL refuse the retry, its `ask` SHALL force a prompt and its `updated_input` SHALL apply, while its `allow` SHALL NOT skip permission evaluation
- **AND** the retry SHALL NOT bypass deny rules, Mode ceilings or auto blocks, its denial SHALL be final without running `PermissionDenied` again, and a refused retry SHALL leave no effect of either input
- **AND** a `PermissionRequest` hook denial SHALL NOT run `PermissionDenied`, and context and `continue: false` SHALL commit with the call's settlement like other tool events

#### Scenario: Chained rewrite preserves envelope
- **WHEN** a PreToolUse or PermissionDenied decision rewrites tool input
- **THEN** the next handler SHALL receive that input with unchanged Session, Location, agent, Mode and timestamp
- **AND** the original event SHALL remain unchanged
- **AND** tool-schema validation and permission evaluation SHALL still be required before execution

#### Scenario: SubagentStart precedes every child effect
- **WHEN** a model `agent` call, forked skill or user/durable delegation (including `cyber review`, TUI `/review` and `@agent`) has passed delegation permission checks
- **THEN** `SubagentStart` SHALL run with the parent Session envelope, the agent profile name as matcher subject and `agent_type`, `subagent_name`, `description`, `fork`, `background`, `isolation`, `resume`, `child_session_id` (pre-assigned or resumed), `user_requested` and `skill`
- **AND** it SHALL run before child execution claims, concurrency permits, durable delegation launch, child Session or worktree creation
- **AND** a `deny` SHALL refuse the launch with `blocked by hook <id>: <reason>` even in `bypass` Mode, and a refused durable delegation SHALL settle failed with that reason
- **AND** an unavailable `fail_closed` handler SHALL refuse the launch, while `allow` and `ask` SHALL neither grant nor prompt for delegation

#### Scenario: SubagentStop observes child handback
- **WHEN** a foreground child settles, or a background child settles before its `job.ended.1`
- **THEN** `SubagentStop` SHALL run once with the parent Session envelope, the agent profile name as matcher subject and `child_session_id`, `agent_type`, `subagent_name`, `status` (`completed` or `error`) and `error`, without the child result text
- **AND** its decisions SHALL be ignored and an interrupted child SHALL NOT emit it
- **AND** only an unavailable `fail_closed` handler SHALL fail the handback; other handler errors SHALL produce a transient notice

#### Scenario: StopFailure observes a failed Turn or Drain
- **WHEN** a provider error ends a Turn, or a runtime error ends a Drain, in a Session that was not interrupted
- **THEN** `StopFailure` SHALL run once with the Session envelope, the error kind as matcher subject and `error_kind` and `error` equal to the published error
- **AND** `Stop` SHALL NOT run for that Drain
- **AND** its decisions, including `block`, SHALL be ignored, its handler failures SHALL NOT change the published error and it SHALL NOT trigger another lifecycle event

#### Scenario: JobEnded observes a terminal background Job
- **WHEN** a background Job's `job.ended.1` or `job.cancelled.1` is committed, including a Job that recovery marks interrupted
- **THEN** `JobEnded` SHALL run once with the parent Session envelope, the Job kind as matcher subject, the Job JSON as `job` and `exit_code`
- **AND** it SHALL run after the Job's settlement and handback notice, its decisions SHALL be ignored, and handler failures (including an unavailable `fail_closed` handler) SHALL NOT change the Job status, the handback or settlement waiters
- **AND** a Job whose parent Session is being deleted, or that ends because the runtime is shutting down, SHALL NOT emit it

### Requirement: Command handlers
(P1) A `command` handler SHALL run its `command` string through the configured shell with the event JSON on stdin, working directory set to the Location directory, and environment variables `CYBER_PROJECT_DIR`, `CYBER_SESSION_ID`, `CYBER_HOOK_EVENT` and `CYBER_AGENT` added. Exit code 0 SHALL mean success, with stdout parsed as a JSON decision when it is a JSON object. Exit code 2 SHALL mean block, and stderr SHALL be fed back as the reason. Any other exit code SHALL be a non-blocking error that is logged and shown to the user.

#### Scenario: Exit 2 blocks a tool
- **WHEN** a `PreToolUse` command hook exits with code 2 and stderr `rm on prod paths is forbidden`
- **THEN** the tool call is not executed and the model receives a tool error containing that text

#### Scenario: Non-blocking failure
- **WHEN** a `PostToolUse` command hook exits with code 1
- **THEN** the turn continues and a warning with the hook id and stderr is shown

#### Scenario: Unavailable fail_closed post-tool handler
- **WHEN** a `fail_closed` `PostToolUse` or `PostToolUseFailure` handler times out, cannot launch or returns invalid output
- **THEN** the call SHALL settle as an error `Tool settled; post-hook failed: hook <id> unavailable` and launched siblings SHALL be cancelled
- **AND** without `fail_closed` the failure SHALL only produce a notice

#### Scenario: Simultaneous command streams
- **WHEN** a command writes stdout and stderr before consuming a large event from stdin
- **THEN** all three streams SHALL progress concurrently and the owned exit code SHALL remain available
- **AND** output beyond the capture limit SHALL be drained without unbounded memory growth

#### Scenario: Command stop acknowledgement
- **WHEN** command execution times out or its owner explicitly cancels it
- **THEN** the owned process tree SHALL be terminated and acknowledgement SHALL be awaited with a bound
- **AND** absent acknowledgement SHALL remain explicit rather than being reported as successful termination


#### Scenario: Captured command decision interpretation
- **WHEN** a command exits successfully with complete JSON object output
- **THEN** its applicable decision fields SHALL be validated against the event
- **AND** a leading UTF-8 byte-order mark and CRLF line endings, as Windows PowerShell may write them, SHALL NOT turn a decision into plain output
- **AND** non-JSON output SHALL mean success without a decision
- **AND** truncated output or malformed decision objects SHALL be errors rather than accepted allow decisions

#### Scenario: Command failure and timeout policies
- **WHEN** a command exits with a code other than zero or two
- **THEN** its stderr SHALL produce a transient nonblocking diagnostic even when fail_closed is enabled
- **AND** an acknowledged timeout SHALL block only when fail_closed is enabled

#### Scenario: Unknown command termination prevents admission
- **WHEN** command termination lacks acknowledgement or the owner cancels execution
- **THEN** the dispatcher SHALL stop subsequent effect admission
- **AND** missing acknowledgement SHALL remain explicit and SHALL NOT produce an allow decision
- **AND** raw diagnostics SHALL NOT be persisted in receipts by default


#### Scenario: Command launch binds trust to the checkout
- **WHEN** a resolved command handler is requested for an event
- **THEN** its current definition SHALL be selected from loaded indexed origins
- **AND** the event Location SHALL resolve to the same checkout as configuration trust
- **AND** project/local checkout and exact-handler approvals SHALL be rechecked before spawning
- **AND** revoked approvals or a changed effective handler digest SHALL prevent launch

#### Scenario: Required hook profile cannot opt out
- **WHEN** a project, local or sandbox-all command hook runs while ordinary tools use full access
- **THEN** the command SHALL still use the workspace-write hooks profile with configured network policy and credential masking
- **AND** an unavailable enforcing backend SHALL refuse execution before the user command starts

#### Scenario: Command scratch preserves unknown ownership
- **WHEN** the command owner is disposed or termination is not acknowledged
- **THEN** its private scratch SHALL remain available for recovery rather than being removed as though execution had settled


#### Scenario: Built-in command hook precedes permission evaluation
- **WHEN** a matching command PreToolUse handler returns a decision for a built-in call
- **THEN** denial SHALL prevent execution even in bypass Mode
- **AND** rewritten input SHALL feed later matching handlers and SHALL pass the tool schema and replay-approval validation before execution
- **AND** hook allow SHALL skip an ask only after permission deny rules and Mode ceilings have been evaluated

#### Scenario: Built-in command post-tool events
- **WHEN** a built-in invocation settles successfully or fails
- **THEN** matching command handlers SHALL receive PostToolUse or PostToolUseFailure with captured identity, call id, final input, output and elapsed duration
- **AND** their receipts SHALL remain durable without claiming the completed tool effect was undone

#### Scenario: Windows command event input retains native ownership
- **WHEN** an authorized Windows global command hook is eligible for full-access launch
- **THEN** the parent SHALL assign the trusted helper to its owned process tree before permitting user code
- **AND** the helper SHALL consume only the private permit prefix and preserve subsequent event bytes and EOF for the command
- **AND** ordinary shell launches SHALL retain their null-stdin behavior
- **AND** hooks requiring sandbox confinement SHALL refuse before effects until that enforcement is available

### Requirement: Hook observability
(P1) Every hook execution SHALL be recorded as a durable `hook.executed.1` event with hook id, event, scope, duration, outcome (`ok`, `blocked`, `error`, `timeout`, `skipped`) and decision. Execution SHALL NOT record stdin or stdout contents unless `telemetry.log_hook_io` is true. Outcomes SHALL be visible in the transcript when a hook blocks or modifies an action.

#### Scenario: Skipped handlers leave receipts
- **WHEN** a matching handler does not run because it is untrusted or a managed policy refuses it
- **THEN** a settled `hook.executed.1` receipt with outcome `skipped` and the reason in its decision SHALL be recorded under the Session's current admission fences
- **AND** a handler whose matcher, paths or `if` condition does not match SHALL leave no receipt

#### Scenario: Blocked action visible
- **WHEN** a hook blocks an `edit`
- **THEN** the transcript shows `blocked by hook <id>: <reason>` and a `hook.executed.1` event with outcome `blocked` is stored

#### Scenario: Durable admission precedes hook effects
- **WHEN** a hook execution is admitted for a captured Session and Location
- **THEN** its start record and projection SHALL commit atomically before the caller receives execution ownership
- **AND** closed or stale Session/ancestor admission fences SHALL refuse the start
- **AND** the execution owner SHALL remain subject to verification before launch or decision admission

#### Scenario: Unknown hook owner disposal
- **WHEN** an admitted execution owner is disposed without acknowledged settlement
- **THEN** its durable receipt SHALL record an explicit unknown error with missing acknowledgement
- **AND** unavailable storage SHALL leave the committed start as unresolved evidence rather than implying success or authorizing replay
- **AND** terminal recording SHALL remain possible after scope closure

#### Scenario: Receipt IO policy is pinned
- **WHEN** an execution starts with hook IO logging disabled
- **THEN** terminal recording SHALL omit supplied raw stdin, stdout and stderr
- **AND** an enabled execution MAY record bounded IO without changing its decision semantics
- **AND** receipt projection and settlement SHALL commit together exactly once

#### Scenario: Authenticated execution receipt pagination
- **WHEN** a client lists a known Session's hook execution receipts
- **THEN** the API SHALL require authentication and page in descending immutable admission-time/id order with Session-bound cursors
- **AND** malformed or foreign cursors SHALL be refused
- **AND** default receipts SHALL omit raw IO while explicitly enabled receipt IO remains visible to the authenticated client
- **AND** running or unknown observations SHALL NOT imply successful process reconciliation or authorize replay

### Requirement: Conditional, one-shot and annotated handlers
(P1) A handler MAY set `if` (`{ field, matches }`: a dotted payload field and a regex), `once: true` (run at most once per Session), `status_message` (shown in the client while the handler runs) and `system_message` (shown to the user, not the model, when the handler completes). A handler whose `if` does not match SHALL be skipped without logging an execution event.

#### Scenario: Run only for one branch
- **WHEN** a `Stop` hook has `if: { field: "git.branch", matches: "^release/" }` and the Session is on `main`
- **THEN** the handler is skipped

#### Scenario: Once command admission survives restart
- **WHEN** a once command has been durably admitted for a Session and its process outcome is unknown
- **THEN** another runtime SHALL skip the same effective handler digest without launching it again
- **AND** terminal settlement SHALL NOT change the admitted once claim
- **AND** a changed effective handler digest SHALL require its ordinary current trust checks before fresh admission

#### Scenario: Once command messages stay user visible
- **WHEN** a once command handler has status and completion messages and matches two built-in invocations in one Session
- **THEN** it SHALL execute only once and publish each configured message once as a transient user notice
- **AND** those messages SHALL NOT be admitted into the model context

### Requirement: Parallel execution within a group
(P1) Handlers matching the same event SHALL run concurrently up to `hooks.concurrency` (default 8). Decision merging SHALL still follow declared order, regardless of completion order. Identical `command` strings matched in multiple scopes SHALL run once per event.

#### Scenario: Duplicate command deduplicated
- **WHEN** the same `command` hook is defined in global and project scope
- **THEN** it executes once for each event

#### Scenario: Fixed-input pool reuses completed slots
- **WHEN** two post-tool command handlers occupy a pool of two and the second finishes while the first still waits
- **THEN** the next matching handler SHALL start in the freed slot without exceeding the configured limit
- **AND** identical supported commands SHALL still execute once per event

#### Scenario: Concurrent request decisions preserve declared order
- **WHEN** two PermissionRequest handlers deny and the later declared handler finishes first
- **THEN** the final merged denial SHALL retain the later declared handler's reason

#### Scenario: Rewrite chains stay sequential
- **WHEN** `PreToolUse` or `PermissionDenied` handlers match an event
- **THEN** they SHALL run one at a time in declared order, by design, because each handler's `updated_input` becomes the next handler's `tool_input`
- **AND** every other event SHALL use the concurrent pool

#### Scenario: Parallel interruption settles owned commands
- **WHEN** interruption or a fail-closed admission error stops a fixed-input hook event
- **THEN** launched sibling commands SHALL be cancelled and drained to their acknowledged or explicit unknown outcomes before dispatch returns
- **AND** queued handlers SHALL NOT launch after event cancellation
- **AND** a fail-closed error SHALL retain its original refusal rather than being replaced by a sibling's cancellation

### Requirement: HTTP handlers
(P1) An `http` handler SHALL `POST` the event JSON to `url` with `content-type: application/json`, configured `headers` (supporting `{env:NAME}` substitution), and the handler timeout. A 2xx response with a JSON object body SHALL be parsed as a decision. A non-2xx response or a transport error SHALL be treated as a non-blocking error, unless `fail_closed: true` is set, in which case it SHALL block.

#### Scenario: Remote policy server denies
- **WHEN** an `http` `PreToolUse` hook returns `200 {"decision":"deny","reason":"blocked by policy"}`
- **THEN** the tool call is denied with reason `blocked by policy`

#### Scenario: Fail-closed timeout
- **WHEN** an `http` hook with `fail_closed: true` times out
- **THEN** the guarded action is blocked with reason `hook <id> unavailable`

#### Scenario: HTTP transport respects explicit scope and authority
- **WHEN** a project/local/plugin or sandbox-all HTTP hook executes
- **THEN** network-off SHALL refuse before an upstream connection and proxy mode SHALL enforce the configured domain allowlist
- **AND** POST requests SHALL use an owned explicit proxy rather than ambient environment proxies
- **AND** redirect following and automatic retries SHALL be disabled
- **AND** configured transport authority/framing headers SHALL be refused and content-type SHALL remain application/json
- **AND** response bodies SHALL be bounded to 1 MiB and require a JSON object decision

#### Scenario: HTTP receipts and cancellation preserve transport ownership
- **WHEN** a recorded or synthetic HTTP hook completes, times out or is cancelled
- **THEN** every accepted local proxy/request transport SHALL be closed and joined before acknowledgement and receipt settlement
- **AND** Session subtree stop SHALL signal and await the owned HTTP execution
- **AND** disposal without settlement SHALL retain unknown receipt/activity evidence
- **AND** acknowledgement SHALL describe local transport disposal, not prove rollback of remote POST processing
- **AND** raw IO SHALL remain absent by default, explicitly opted-in event/response IO SHALL remain bounded and configured header credentials SHALL never be recorded as IO

### Requirement: Stop hooks and loop prevention
(P1) `Stop` hooks SHALL run when a Drain is about to go idle and MAY return `{"decision":"block","reason":...}` to admit the reason as a new user-visible instruction and continue. The event SHALL carry `stop_hook_active: true` when the current continuation was caused by a Stop hook. The system SHALL allow at most `hooks.max_stop_continuations` (default 5) consecutive Stop-hook continuations per Drain.

#### Scenario: Tests must pass before stopping
- **WHEN** a `Stop` hook runs the test suite, which fails, and returns `block` with the failure summary
- **THEN** the session continues with that summary as input

#### Scenario: Continuation cap
- **WHEN** Stop hooks have blocked 5 consecutive times
- **THEN** the session stops and a warning `stop hook continuation limit reached` is shown

#### Scenario: Stop runs only for a Drain that answered and would idle
- **WHEN** a Drain, including a child Session's Drain, finishes a model answer with no eligible input left
- **THEN** `Stop` SHALL run with the Session envelope, the agent name as matcher subject and `stop_hook_active`
- **AND** an interrupted or cancelled Drain, a failed Turn, the final step-limit Turn and a child governed by an output schema (which ends through `return_result`) SHALL NOT run `Stop`
- **AND** `SubagentStop` SHALL remain the parent-side handback event

#### Scenario: Blocked Stop admits tagged queued input
- **WHEN** the merged `Stop` decision is `block` and the Drain has used fewer than `hooks.max_stop_continuations` continuations
- **THEN** the reason, prefixed with the blocking hook's ID, SHALL be durably admitted as queued input with source `hook` and the same Drain SHALL continue with it
- **AND** a new Drain, any public user admission (whatever source it claims) or any internal admission other than a Stop continuation SHALL reset the consecutive count, so its next `Stop` carries `stop_hook_active: false`
- **AND** promoting a Stop continuation SHALL NOT reset the Session's step count, so continuations never extend `max_steps`

#### Scenario: Unavailable fail_closed Stop handler
- **WHEN** a `fail_closed` Stop handler times out, cannot launch or returns invalid output
- **THEN** the Drain SHALL end with a `hook` error rather than idle silently or treat the failure as a `block`
- **AND** without `fail_closed` the failure SHALL produce a transient notice and the Drain SHALL idle, even when the runner forces the failure closed (for example a failed launch re-authorization)

### Requirement: Hook context injection on lifecycle events
(P1) `SessionStart` and `UserPromptSubmit` hooks MAY return `additional_context`. The system SHALL admit it as a Mid-Conversation System Message tagged with the hook id. For `UserPromptSubmit`, a `deny` decision SHALL reject the prompt before admission and show the reason to the user.

#### Scenario: Inject ticket context
- **WHEN** a `UserPromptSubmit` hook returns `additional_context` with the linked Jira ticket text
- **THEN** the model sees that context before the user's prompt in the same Turn

#### Scenario: Block secrets in prompt
- **WHEN** a `UserPromptSubmit` hook denies a prompt containing an API key
- **THEN** the prompt is not admitted and the user sees the reason

#### Scenario: UserPromptSubmit guards public user input only
- **WHEN** input arrives through public user admission (HTTP prompt or command, TUI, exec), whatever source the client claims, an in-process admission with source `user`, or a public user delegation (`/delegations/{op}` and `/subtask`, behind TUI `@agent` and exec delegation)
- **THEN** `UserPromptSubmit` SHALL run before any durable effect of the admission, with the Session envelope (the parent Session for a delegation), an empty matcher subject and the prompt text (including text attachments) as `prompt` and its `source` (`user` for a delegation)
- **AND** a `deny` SHALL reject it with `blocked by hook <id>: <reason>` and commit no inbox row, history entry, staged revert, Delegation record, Job or child Session, and an unavailable `fail_closed` handler SHALL also reject it
- **AND** child prompts, handback notices, Stop continuations, forwarded forked-skill rows (already checked at admission) and other internal admissions SHALL NOT run it, and an exact retry of admitted input or of a recorded delegation SHALL NOT run it again
- **AND** editing the parts of an unpromoted row SHALL run it again on the edited text, and a `deny` SHALL leave the row unchanged

#### Scenario: Context is admitted with its prompt
- **WHEN** merged `UserPromptSubmit` decisions carry `additional_context`
- **THEN** each handler's context SHALL be tagged `[hook <id>]`, kept with the admitted row and committed as a system message with reason `hook_context` in the same commit that promotes the row, immediately before it
- **AND** a removed or refused row SHALL never admit its context, and edited parts SHALL replace it
- **AND** a forked skill row's context SHALL be committed in the parent in the same commit that forwards the row, so the forked child inherits it while the skill body stays out of the parent
- **AND** a delegation's context SHALL be admitted with the child's prompt and committed immediately before it when the child promotes it, never in the parent

#### Scenario: SessionStart runs once per root Session
- **WHEN** a root Session reaches the Safe Boundary of its first Turn
- **THEN** `SessionStart` SHALL run once, before input promotion or forked-skill forwarding (and therefore before that child's `SubagentStart`), with `source` `startup`, or `resume` when the Session begins with history it did not produce (a fork), as payload field and matcher subject
- **AND** a durable marker SHALL be committed together with its context, so it SHALL NOT run or admit its context again on later Turns or after the runtime is recreated, including a restart before the first Turn opened
- **AND** its context SHALL be committed as a tagged system message before the first prompt
- **AND** its decisions SHALL be ignored, an unavailable `fail_closed` handler SHALL end the Drain with a `hook` error before any Turn, and child Sessions SHALL report their launch through `SubagentStart` instead

### Requirement: Timeouts and async hooks
(P1) Each handler SHALL time out after `timeout` seconds (default 60, maximum 600). A timeout SHALL be treated as a non-blocking error unless `fail_closed` is set. A handler with `async: true` SHALL run in the background without blocking the event, and its decision SHALL be ignored except for `additional_context`, which SHALL be admitted at the next Safe Boundary.

#### Scenario: Async notification hook
- **WHEN** a `Notification` hook with `async: true` posts to a chat webhook taking 5 s
- **THEN** the session is not delayed

#### Scenario: Async admission precedes the event and settles once
- **WHEN** a trusted `async` handler matches an event
- **THEN** its durable receipt SHALL be admitted, with once claims and admission fences unchanged, before the event proceeds, and the handler SHALL then run in a runtime-owned task the event never awaits
- **AND** an `async` handler SHALL NOT deduplicate an identical synchronous command
- **AND** interrupt, subtree stop and shutdown SHALL cancel that task through its execution activity, and shutdown SHALL join it; work that does not settle within a bounded grace period after cancellation SHALL be disposed and its receipt recorded `unknown`, and otherwise the receipt SHALL settle exactly once

#### Scenario: Async decisions, context and failures
- **WHEN** an `async` handler completes
- **THEN** its `decision`, `updated_input`, `continue` and `stop_reason` SHALL be ignored with a debug log, and its `additional_context`, tagged `[hook <id>]`, SHALL be queued for its Session and committed as a `hook_context` system message at the next Safe Boundary, before that Turn's request
- **AND** `fail_closed` SHALL NOT block the event of an `async` handler: an admission failure, timeout or invalid output SHALL be recorded on its receipt and shown only as a notice
- **AND** queued context SHALL be process-local: context still queued when the runtime stops SHALL be lost while the receipt remains
