## MODIFIED Requirements

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
- **WHEN** event-specific data contains event, session_id, location, project_id, agent, mode or timestamp
- **THEN** envelope construction SHALL refuse that payload rather than overwriting the captured identity

#### Scenario: Chained rewrite preserves envelope
- **WHEN** a PreToolUse or PermissionDenied decision rewrites tool input
- **THEN** the next handler SHALL receive that input with unchanged Session, Location, agent, Mode and timestamp
- **AND** the original event SHALL remain unchanged
- **AND** tool-schema validation and permission evaluation SHALL still be required before execution

### Requirement: Command handlers
(P1) A `command` handler SHALL run its `command` string through the configured shell with the event JSON on stdin, working directory set to the Location directory, and environment variables `CYBER_PROJECT_DIR`, `CYBER_SESSION_ID`, `CYBER_HOOK_EVENT` and `CYBER_AGENT` added. Exit code 0 SHALL mean success, with stdout parsed as a JSON decision when it is a JSON object. Exit code 2 SHALL mean block, and stderr SHALL be fed back as the reason. Any other exit code SHALL be a non-blocking error that is logged and shown to the user.

#### Scenario: Exit 2 blocks a tool
- **WHEN** a `PreToolUse` command hook exits with code 2 and stderr `rm on prod paths is forbidden`
- **THEN** the tool call is not executed and the model receives a tool error containing that text

#### Scenario: Non-blocking failure
- **WHEN** a `PostToolUse` command hook exits with code 1
- **THEN** the turn continues and a warning with the hook id and stderr is shown

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
