## MODIFIED Requirements

### Requirement: Built-in agents
(P0) The system SHALL ship the built-in agents `build` (primary, full tool access), `explore` (subagent, read-only: `read`, `glob`, `grep`, `webfetch`, `websearch` and read-only `bash`), `general` (subagent, full tool access except `todo`), and the hidden system agents `compaction`, `title`, `summary` and `evaluator`, which SHALL have every tool denied. Planning SHALL be the `plan` permission Mode (`permissions-modes`), not an agent; an agent definition MAY set `permission_mode: plan` to start Sessions in that Mode.

#### Scenario: Default agent set
- **WHEN** a user lists agents in a project with no agent configuration
- **THEN** `build`, `explore` and `general` are listed and the hidden system agents are omitted

#### Scenario: System agents cannot call tools
- **WHEN** the `title` agent's model emits a tool call
- **THEN** the call is settled as an error without executing and no permission request is created

### Requirement: Custom agents from config
(P0) The system SHALL read agents from the `agents` key of `cyber.jsonc`/`cyber.json`, keyed by name. An entry matching an existing agent SHALL patch only the fields it sets; a new name SHALL create an agent with `mode: "all"`; `disabled: true` SHALL remove the agent, including built-ins other than the hidden system agents.

#### Scenario: Override built-in model
- **WHEN** config sets `agents.explore.model` to `ollama/qwen3-coder`
- **THEN** `explore` subagents use that model and keep their built-in read-only rules

#### Scenario: Disable a built-in agent
- **WHEN** config sets `agents.explore.disabled` to `true`
- **THEN** `explore` is absent from listings, `@` autocomplete and the `agent` tool catalogue

### Requirement: Agent definition fields
(P0) An agent definition SHALL accept `description`, `system`, `model` (`provider/model[#variant]`), `variant`, `mode` (`primary`, `subagent` or `all`), `permission_mode` (one of the six Modes), `tools` (`allow` and `deny` lists of tool names or globs), `permissions` (ordered rules), `request` (a provider request overlay of `headers` and `body`, for example `temperature` or `top_p`, layered as defined by `provider-catalog`), `steps` (positive integer), `color`, `hidden`, `isolation` (`none` or `worktree`), `background` (boolean default for spawns), `memory` (`none`, `project` or `user`), `skills` (names preloaded into context) and `mcp` (subset of configured MCP server names). Unknown fields SHALL be rejected by schema validation, naming the agent and field.

#### Scenario: Agent request options reach runtime inference
- **WHEN** a Turn is prepared for a selected agent with configured request options
- **THEN** its body SHALL deep-merge after the resolved provider/model/variant template and its headers SHALL override earlier names case-insensitively
- **AND** API-key fields (`apiKey`, `api_key` and `apikey`, case-insensitively) SHALL be removed from nested objects and arrays before adapter dispatch
- **AND** changes or removal SHALL apply at a later safe boundary, preserving the current request during provider retries
- **AND** unavailable, invalid or hidden inference profiles SHALL refuse preparation before input promotion, retaining existing context and retryable input

#### Scenario: Profile starting Mode preserves caller authority and retryable creation
- **WHEN** a caller omits the Mode and the selected profile has `permission_mode`
- **THEN** the Session SHALL start in that Mode
- **AND** an explicit caller Mode SHALL override this starting default
- **AND** if a profile is temporarily unavailable at explicit-model creation, its default SHALL remain pending until a valid profile is resolved before inference
- **AND** explicit Mode switches SHALL settle that pending choice even when the visible Mode is unchanged
- **AND** creation retry and replay SHALL preserve the selected or pending state

#### Scenario: Agent model and variant defaults preserve explicit selection
- **WHEN** a Session omits its model
- **THEN** its selected agent's model SHALL take precedence over the Location default
- **AND** the agent's separate variant SHALL take precedence over variants embedded in inherited defaults
- **WHEN** a caller explicitly selects a model
- **THEN** that model SHALL remain authoritative and an explicit `#variant` SHALL override the agent variant
- **AND** an explicit bare model SHALL use the agent variant when configured
- **AND** profile changes and removal SHALL apply at later safe boundaries without turning automatic defaults into explicit choices
- **AND** replay and forks SHALL retain the original explicit or inherited selection
- **AND** unavailable selected models or variants SHALL refuse inference before Epoch creation or prompt promotion

#### Scenario: Selection changes during preparation
- **WHEN** an agent, model or Mode changes after safe-boundary resolution but before the Turn start event
- **THEN** preparation SHALL repeat for the latest selection before inference or tool dispatch
- **AND** input SHALL remain promoted exactly once, with no phantom Turn or post-Turn settlement for discarded preparation

#### Scenario: Agent identity remains pinned through tool settlement
- **WHEN** the user selects another agent after a Turn's request has been prepared
- **THEN** tool invocation identity SHALL retain the agent recorded at Turn start through every tool group
- **AND** ancestor profile deny ceilings SHALL use an active ancestor's Turn agent rather than its pending selection
- **AND** the next prepared Turn SHALL use the new selection
- **AND** replay SHALL preserve that identity, using historical start-time selection when legacy events omit it

#### Scenario: Unknown field rejected
- **WHEN** an agent definition contains `temprature: 0.2`
- **THEN** config validation fails with an error naming the agent and the field `temprature`

#### Scenario: Tool deny list hides tools
- **WHEN** an agent sets `tools.deny: ["bash", "web*"]`
- **THEN** `bash`, `webfetch` and `websearch` are omitted from that agent's tool definitions for every Turn

#### Scenario: Tool profile restrictions cover direct and client-registered dispatch
- **GIVEN** an agent's tools deny list matches a tool name, or its allow list excludes that name
- **WHEN** built-in or client-registered tool definitions are materialized or a call is dispatched
- **THEN** the tool SHALL be omitted from definitions and rejected before execution
- **AND** bypass Mode SHALL NOT override profile visibility restrictions
- **AND** hidden, disabled or unknown profiles SHALL NOT dispatch tools

#### Scenario: Agent prompt uses the durable Context Epoch
- **WHEN** a selected agent has system text at Context Epoch initialization
- **THEN** its system text SHALL start model requests and be persisted as that Epoch's prefix
- **AND** changes or removal SHALL arrive through safe-boundary context updates without rewriting the Epoch prefix
- **AND** historical Epoch events without an explicit prefix SHALL retain provider-base fallback behavior

#### Scenario: Request overlay applied
- **WHEN** an agent sets `request.body.temperature: 0.2`
- **THEN** its Turns send `temperature: 0.2` after provider, model and variant defaults

#### Scenario: Malformed agent profile values are rejected before materialization
- **WHEN** a resolved agent definition contains an invalid field type, profile mode, permission mode, model reference, nonpositive step limit or malformed tool/request overlay
- **THEN** configuration loading SHALL fail with an error naming the agent and field
- **AND** arbitrary provider body members and valid permission shorthand SHALL remain accepted

#### Scenario: Agent rules reach built-in permission admission
- **WHEN** a selected profile provides permission rules
- **THEN** built-in tool admission SHALL evaluate them after config and before Session rules
- **AND** a final effective deny SHALL refuse dispatch even in bypass Mode without side effects
- **AND** user/global deny ceilings SHALL remain final despite profile or Session allows

#### Scenario: Step-limit requests cannot restore tools through overlays
- **WHEN** an agent reaches its final allowed Turn
- **THEN** native and compatible adapters SHALL force no-tools selection after all request overlays
- **AND** injected tool definitions or forced tool choices SHALL NOT override that limit
- **AND** stricter runtime/Session budgets SHALL remain enforced and new user input SHALL reset the count

### Requirement: Agent tool spawns subagents
(P1) The system SHALL provide an `agent` tool with inputs `prompt` (required), `agent` (default `general`), `description` (3–8 words), `output_schema` (JSON Schema), `model`, `isolation` (`none`, `worktree` or, from P3, `remote`), `runner` (with `isolation: remote`: a pool, `rnr_` ID or peer name), `background` (boolean), `fork` (boolean) and `resume` (subagent name or `ses_` ID). Each spawn SHALL create a child Session whose `parent_id` is the caller, titled `<description> (@<agent>)`, and SHALL request the `agent` permission with the target agent name as resource.

#### Scenario: Foreground subagent
- **WHEN** the model calls `agent` with `agent: "explore"` and `background: false`
- **THEN** a child Session runs to completion and the tool result contains the subagent's final text and its `ses_` ID

#### Scenario: Foreground ownership survives cancellation and disposal
- **WHEN** a foreground caller is interrupted or its tool future is dropped
- **THEN** its child SHALL be interrupted and joined before its concurrency permit is released
- **AND** canceled queued calls SHALL NOT create child Sessions
- **AND** uncertain spawn settlement SHALL NOT automatically create another child during recovery

#### Scenario: Configuration changes while a spawn waits
- **WHEN** a queued or approved spawn resumes after its target profile is disabled, hidden or unavailable, or its effective permission becomes denied
- **THEN** the spawn SHALL fail before creating its child Session

#### Scenario: Unknown agent
- **WHEN** the model calls `agent` with `agent: "nonexistent"`
- **THEN** the call fails with `Unknown agent "nonexistent". Available: <names>` without creating a Session

### Requirement: Structured subagent output
(P1) When `output_schema` is supplied, the system SHALL give the subagent a `return_result` tool whose input is that schema, SHALL end the child Session when it is called with valid input, and SHALL return the validated value as the tool result's structured output. If the subagent finishes without a valid result, the system SHALL re-prompt it once with the validation errors and then fail the call with `SchemaMismatch` including the last errors.

#### Scenario: Valid structured result
- **WHEN** a subagent spawned with schema `{findings: array}` calls `return_result` with a matching object
- **THEN** the parent receives that object as structured output and the child Session ends

#### Scenario: Retry once on mismatch
- **WHEN** a subagent ends with text only and the single re-prompt also yields no valid result
- **THEN** the `agent` call fails with `SchemaMismatch` and the validation errors

#### Scenario: Typed results survive replay and output limits
- **WHEN** a foreground child returns a valid value, including JSON null or a value larger than the ordinary tool-output preview limit
- **THEN** durable call settlement, replay and HTTP history SHALL retain the complete typed value
- **AND** historical calls without structured output SHALL preserve their serialized shape
- **AND** rewind removing the result SHALL retain the child schema for a new attempt

#### Scenario: Result completion cannot dispatch later side effects
- **WHEN** a provider response includes a valid return_result followed by another tool call
- **THEN** later calls SHALL settle without dispatch
- **AND** duplicate call IDs within a response SHALL stop that response before tool dispatch

#### Scenario: Result correction preserves cancellation and refusal
- **WHEN** a structured child is interrupted or its permission request is rejected without feedback
- **THEN** the parent SHALL fail that attempt without admitting a result correction
- **AND** interruption during the single correction SHALL stop the child through the foreground owner

### Requirement: Background subagents and handback
(P1) A spawn with `background: true` (or an agent with `background: true` by default) SHALL return immediately with `{ id, name, state: "running" }`. On completion or failure, the system SHALL admit a handback message into the parent Session with `delivery: queue` containing the subagent name, status, final text or structured result, and cost. Background subagents SHALL be listed in `/tasks` and stoppable.

#### Scenario: Handback when parent idle
- **WHEN** a background subagent completes while its parent Session is idle
- **THEN** the handback is admitted and the parent starts a new Turn to process it

#### Scenario: Handback while parent busy
- **WHEN** a background subagent completes while the parent is mid-Drain
- **THEN** the handback waits in the inbox and is promoted when the parent would otherwise go idle

#### Scenario: Parent interruption preserves a background child
- **WHEN** the parent Drain is interrupted after a background call hands off ownership
- **THEN** the child SHALL remain running and continue under its existing permission and Mode ceilings
- **AND** explicit task cancellation or Session deletion SHALL stop it

### Requirement: Forked subagents
(P1) A spawn with `fork: true` SHALL create a child Session that inherits the parent's full projected history, Context Epoch baseline, agent and model (unless overridden), instead of starting with only the prompt. The `/subtask <prompt>` command SHALL spawn a forked background subagent; `/fork` SHALL copy the whole Session into a new independent top-level Session.

#### Scenario: Fork inherits context
- **WHEN** the user runs `/subtask try the alternative parser approach`
- **THEN** the child Session's first Turn includes the parent's history and the prompt, and the parent continues independently

#### Scenario: Explicit user subtask admission
- **WHEN** the user submits `/subtask <prompt>` or POSTs its prompt to `/api/v1/sessions/{sessionID}/subtask`
- **THEN** the system SHALL start a forked background child and return its durable Job identity without switching the parent or adding a parent Turn
- **AND** explicit user initiation SHALL not require another agent-spawn approval, in every Mode, while profile restrictions, deny rules and child permission ceilings remain enforced
- **AND** blank prompts SHALL fail before child creation

#### Scenario: Fork identity and durable context
- **WHEN** a Session is copied or an agent forks its caller
- **THEN** the copy SHALL preserve the projected history, Context Epoch and durable task sources with fresh local message/call identities
- **AND** copied source calls SHALL never be redispatched by the copy or cancel their source owner
- **AND** /fork SHALL clear parent/name bindings while an agent fork SHALL retain its caller as parent
- **AND** historical copied usage SHALL not be billed again

### Requirement: Resume subagents by name
(P1) Each subagent SHALL get a unique name within its parent (the agent name, suffixed `-2`, `-3`… on collision, or a caller-supplied `name`). A spawn with `resume` SHALL admit the new prompt into the existing child Session instead of creating one, failing with `Subagent not found` for unknown names and `Subagent busy` when it is running in the foreground of another caller.

#### Scenario: Continue a previous subagent
- **WHEN** the model calls `agent` with `resume: "explore"` and a follow-up prompt
- **THEN** the prompt is admitted to the existing `explore` child Session, which keeps its earlier context

#### Scenario: Foreground and background names share one parent namespace
- **WHEN** a parent creates foreground and background children with default or explicit names
- **THEN** each new child SHALL have a unique durable parent-scoped name regardless of execution mode
- **AND** a duplicate caller name SHALL fail before child creation or dispatch
- **AND** the name SHALL survive restart and appear on tool results and routed approvals without changing the Session title

#### Scenario: Resume ownership and structured attempts
- **WHEN** an agent call resumes a named child or a direct child Session ID
- **THEN** it SHALL admit into that same child after target permission checks, preserving history and its name
- **AND** an active execution owner SHALL cause Subagent busy before admission, including the interval after Drain idleness and before result settlement
- **AND** a resumed structured attempt SHALL durably clear its earlier terminal result and validate the new result against the retained or explicitly replaced schema
- **AND** a background resume SHALL create a new Job for the same child/name, with one distinct handback for each attempt

#### Scenario: Failed follow-up preparation never returns a previous answer
- **WHEN** a resumed prompt cannot be promoted because preparation fails
- **THEN** foreground result collection SHALL fail and a background Job SHALL settle as error
- **AND** neither result SHALL reuse an assistant answer from before that prompt
- **AND** the pending prompt SHALL remain available for recovery

### Requirement: Worktree isolation for subagents
(P1) A spawn with `isolation: "worktree"` SHALL create a managed git worktree (see worktrees) on a branch `cyber/<parent-short-id>/<subagent-name>` and bind the child Session's Location to it. On completion the tool result SHALL report the branch, changed files and diff stats; the worktree SHALL be kept when it has changes and removed when clean, unless `worktrees.keep` is `always`.

#### Scenario: Isolated edits
- **WHEN** two subagents with `isolation: "worktree"` edit the same file concurrently
- **THEN** each edits its own checkout and both results report their branch names

#### Scenario: Clean worktree removed
- **WHEN** an isolated subagent finishes without changing files
- **THEN** its worktree and branch are deleted

#### Scenario: Explicit user isolated profile
- **GIVEN** the user's current profile selects worktree isolation
- **WHEN** the user starts `/subtask` in any permission Mode
- **THEN** checkout creation and trusted source setup use explicit user lifecycle authority, with no additional model-tool spawn approval
- **AND** child tools retain the inherited effective Mode, ordinary approval routing and deny ceilings
- **AND** source/ancestor denies and read-only sandbox policy reject creation

#### Scenario: Empty setup for a Plan child
- **WHEN** a model-selected isolated child starts in Plan Mode with no setup commands
- **THEN** setup verifies ready ownership without requiring write authority
- **AND** child model tools remain subject to Plan restrictions

#### Scenario: Child names are independent of storage names
- **WHEN** an isolated child has a Git-valid name within the Session naming limit
- **THEN** its branch uses that name independently of its generated managed storage identity
- **AND** invalid Git branch refs fail before child Session or model admission

#### Scenario: Child cleanup confirmation
- **GIVEN** a clean isolated child and `worktrees.cleanup: ask`
- **WHEN** the child finishes
- **THEN** cleanup requests individual child-owned approval through its ancestor routes without holding the repository lifecycle lock
- **AND** rejection, unattended operation or cancellation preserves the checkout
- **AND** approval rechecks current deny/keep rules and native dirty/activity removal admission
- **AND** cancellation removes the idle child's pending cleanup request without abandoning unrelated routed requests

#### Scenario: Clean-removed isolated child resume
- **GIVEN** a completed non-force clean removal matching the child's original incarnation and base
- **WHEN** its parent resumes that child
- **THEN** native recreation retains the original branch/base and assigns a new creation identity
- **AND** a durable child-owner-checked rebound updates Location before trusted setup and prompt admission
- **AND** existing history, Epoch and result-schema continuity are preserved
- **AND** missing, force, incomplete, foreign or replaced evidence requires recovery rather than silent recreation

#### Scenario: Source-authorized initialization of a Plan child
- **GIVEN** the source invocation authorizes a trusted setup recipe
- **WHEN** it creates or recreates a child whose inference Mode is Plan
- **THEN** setup uses source invocation authority within the child checkout
- **AND** child model tools retain Plan restrictions

#### Scenario: Setup readiness survives interruption
- **WHEN** a newly created or rebound isolated child has not acknowledged completed setup
- **THEN** prompt admission, wake/resume and ordinary Location operations are rejected
- **AND** failed setup remains gated after restart without blind command redispatch

#### Scenario: Existing primary-profile child resume
- **WHEN** the parent resumes its existing primary-profile fork or explicit user subtask
- **THEN** the child retains its assigned identity and ordinary permissions
- **AND** resumption does not authorize a fresh primary-profile subagent spawn

#### Scenario: Retained isolated child restart
- **GIVEN** an isolated child's checkout was retained
- **WHEN** the application restarts and resumes that child
- **THEN** it verifies the durable creation identity, branch and path before admitting the new prompt
- **AND** changed or missing ownership cannot silently redirect execution

#### Scenario: Setup failure prevents isolated inference
- **WHEN** an isolated child's trusted setup command fails
- **THEN** no child prompt is admitted and the owned checkout remains available for recovery

#### Scenario: Approval cannot override a newly added deny
- **WHEN** a worktree deny is added while isolated creation awaits approval
- **THEN** replying once cannot create the checkout or child Session

### Requirement: Concurrency cap
(P1) The system SHALL cap concurrently running subagents per top-level Session at `agents.max_concurrent` (default 8). Spawns beyond the cap SHALL wait in FIFO order, and a waiting foreground spawn SHALL report `queued` progress. Workflow agents SHALL be governed by `workflows.max_concurrent` instead.

#### Scenario: Ninth spawn queued
- **WHEN** 8 subagents are running and the model spawns a 9th
- **THEN** the 9th starts only after one of the 8 settles

### Requirement: Nesting depth
(P1) The system SHALL refuse a spawn when the calling Session is already nested `agents.max_depth` (default 2) levels below a top-level Session, failing with `Subagent depth limit reached (2). Increase agents.max_depth to allow deeper nesting.`

#### Scenario: Depth exceeded
- **WHEN** a subagent at depth 2 calls `agent`
- **THEN** the call fails with the depth limit message and no Session is created

### Requirement: Subagent permission inheritance
(P1) A child Session SHALL evaluate permissions as: the subagent's own rules, then every `deny` rule of the parent Session, capped by the parent's Mode (a child SHALL NOT run in a more permissive Mode than its parent). Permission requests from a child SHALL surface in the parent's client with the child's name, and a reject without feedback SHALL stop only the child.

#### Scenario: Parent deny propagates
- **WHEN** the parent denies `bash` resource `rm *` and a subagent allows `bash`
- **THEN** the subagent's `rm -rf build` is denied

#### Scenario: Mode ceiling
- **WHEN** the parent runs in `default` Mode and spawns an agent configured with `permission_mode: bypass`
- **THEN** the child runs in `default` Mode

#### Scenario: Durable ancestry cannot silently lose restrictions
- **WHEN** a child dispatches a built-in tool with a durable parent chain
- **THEN** every ancestor deny from config, profile and Session rules SHALL remain an execution ceiling
- **AND** parent allows SHALL NOT preapprove that child
- **AND** fresh Session creation SHALL reject missing parents and ancestry cycles before recording the Session

#### Scenario: Parent Mode is pinned and composed with child restrictions
- **WHEN** a built-in child request is evaluated while an ancestor has an active Turn
- **THEN** permission decisions SHALL intersect the child Mode with every ancestor’s effective Turn Mode, preserving deny over ask over allow
- **AND** a pending ancestor Mode selection SHALL NOT replace the active Turn’s Mode
- **AND** a plan-file allowance or an earlier ask SHALL NOT mask a later ancestor denial
- **AND** an unknown ancestor Mode SHALL refuse dispatch

#### Scenario: Ancestor request listing preserves child ownership
- **WHEN** a child has a pending permission or question request
- **THEN** its ancestors SHALL be able to list and reply to that request through their Session endpoints, while unrelated Sessions SHALL NOT gain reply authority
- **AND** the request and reply SHALL remain recorded on the child, without adding permission events to ancestor histories
- **AND** reject and always-approval cascades SHALL affect only requests owned by the replied-to request's Session

#### Scenario: Child request notifications survive client reconnects
- **WHEN** a child in another Location requests permission or asks a question
- **THEN** ancestor Location and Session streams SHALL forward live ask/reply notifications with server-derived child identity and validated ancestor routes
- **AND** the TUI SHALL show the child name and only requests owned by or routed to its viewed Session
- **AND** pending child requests SHALL be recovered on parent Session stream and SDK reconnects, without replacing the parent Session's durable replay cursor or adding child events to its history
- **AND** a newly published request SHALL already have a registered reply waiter
- **AND** historical request payloads without routing fields SHALL preserve their serialized shape

### Requirement: Subagent result summarization
(P1) The tool result returned to the parent SHALL contain at most `agents.result_max_bytes` (default 16384) of the subagent's final text; longer text SHALL be written to a Managed Tool Output File and its path included. Intermediate tool output of the child SHALL never be copied into the parent's history.

#### Scenario: Oversized final answer
- **WHEN** a subagent's final text is 40 KB
- **THEN** the parent receives a 16 KB preview plus the path of the full text file

### Requirement: Cost attribution
(P1) Token usage and cost of child Sessions SHALL be recorded on the child and rolled up into the parent Session's `children_cost` and `children_tokens`, and SHALL count against any goal or workflow budget that covers the parent.

#### Scenario: Parent shows rolled-up cost
- **WHEN** a parent with own cost $0.10 spawned subagents costing $0.25 in total
- **THEN** the Session info reports `cost: 0.10` and `children_cost: 0.25`

#### Scenario: Durable descendant billing attribution
- **WHEN** a child or nested descendant commits visible or hidden model usage
- **THEN** each ancestor's children cost, total tokens and unknown-price count SHALL update in the same database transaction as that usage
- **AND** own usage SHALL remain separate and copied fork history SHALL not create new charges
- **AND** deleting a child conversation SHALL retain its already-attributed charges on surviving ancestors
- **AND** reopening the database SHALL preserve the rollup without redispatch or duplicate charges
- **AND** an older database SHALL reconstruct surviving usage and disclose incomplete historical child accounting when previously deleted usage cannot be proven

#### Scenario: Delegated exec descendant budget accounting
- **WHEN** a named delegated exec child creates nested children and their combined recorded cost or tokens reach its exec limit
- **THEN** exec SHALL request cancellation of its owned Job and report budget_exceeded with own and descendant billing separately disclosed
- **AND** budgeted delegation SHALL refuse a server lacking descendant billing capability before submission, or reject an incomplete/malformed billing snapshot with explicit owned cancellation
- **AND** repeated polling SHALL replace cumulative billing snapshots without double-counting, and final catch-up SHALL retain descendant billing
- **AND** client polling SHALL disclose possible in-flight overshoot; complete server-side subtree budget and cancellation enforcement remains required

### Requirement: Manual invocation by mention
(P1) An `@<agent>` mention of a subagent-capable agent in a user prompt SHALL spawn that agent through the `agent` tool flow with the rest of the prompt, without requiring the `agent` permission. In the TUI, `@` autocomplete SHALL offer visible agents with `mode` `subagent` or `all`.

#### Scenario: Explicit named client delegation
- **WHEN** a client submits the optional `agent` field on the Session subtask endpoint
- **THEN** the host SHALL start a fresh background child with that visible subagent-capable profile, without copying the parent history
- **AND** user initiation SHALL authorize spawning without an ask, while current/ancestor denies, child Mode ceilings, profile tool restrictions and ordinary child approvals remain enforced
- **AND** a hidden, unknown or primary-only explicit target SHALL fail before child creation

#### Scenario: TUI agent mentions and files coexist
- **WHEN** the user enters a leading `@` token
- **THEN** autocomplete SHALL offer visible subagent-capable profiles along with files
- **AND** submitting a leading known agent mention SHALL send the remaining prompt through explicit named delegation
- **AND** names containing whitespace SHALL complete with a JSON-quoted name after `@` and resolve to the exact configured profile
- **AND** ordinary file mentions and agent-like text quoted before the `@` marker or embedded in prose SHALL retain ordinary prompt admission

#### Scenario: Durable client admission identity
- **WHEN** a client starts delegation with a client-selected `op_` request ID scoped to the source Session
- **THEN** the runtime SHALL persist the request identity and exact-input digest before queue admission and return a queryable admission record
- **AND** repeating identical input SHALL return the same admission or recorded Job without redispatch
- **AND** different input under the same identity SHALL fail with a conflict
- **AND** scoped lookup and stop SHALL not grant authority over another Session's admission or Job
- **AND** cancellation received before a delayed submission SHALL persist a tombstone that prevents dispatch
- **AND** a durable launch marker SHALL precede child creation or setup effects
- **AND** failure or lost ownership after that marker SHALL remain unknown unless Job settlement is recorded
- **AND** restart or actor failure SHALL not automatically redispatch uncertain work

#### Scenario: Explicit cancellation recovers an abandoned reservation
- **WHEN** an admission actor is missing or finished and the durable request remains reserved without a Job
- **THEN** explicit scoped cancellation SHALL atomically persist a cancelled tombstone, without dispatching work
- **AND** a competing or delayed launch-marker write SHALL be refused if cancellation wins
- **AND** a launch marker winning the race SHALL preserve unknown outcome until effect settlement is proven
- **AND** late host errors SHALL not replace an acknowledged reserved cancellation
- **AND** request input binding and conflict detection SHALL remain intact after recovery

#### Scenario: Caller-owned delegation cancellation
- **WHEN** an owned runtime delegation request is cancelled while waiting for child admission
- **THEN** it SHALL settle that request without creating another child or stopping an existing child
- **AND** its queued admission SHALL release capacity for later requests
- **AND** cancellation racing with successful Job handoff SHALL settle only the recorded Job belonging to the source Session and return its terminal identity
- **AND** a returned Job SHALL transfer cancellation ownership to its Job controls

#### Scenario: Exec follows an explicitly mentioned child
- **WHEN** `cyber exec` submits a leading eligible agent mention
- **THEN** it SHALL start explicit named delegation with attached Content and an optional positive requested turn ceiling
- **AND** it SHALL print the child's durable result and usage, including events committed before the delegation response
- **AND** it SHALL wait for terminal Job settlement rather than an early child idle notification
- **AND** it SHALL retain a client-generated admission ID before submission and recover a lost response by scoped lookup without redispatch
- **AND** timeout and interruption SHALL cover submission and queued admission, cancelling only that request or its validated Job without interrupting the parent or another child
- **AND** local budget exhaustion after Job admission SHALL cancel only that owned Job
- **AND** uncertain cancellation SHALL report the admission ID and preserve any validated Job identity for recovery
- **AND** unknown/file mentions and `--command` SHALL preserve ordinary admission

#### Scenario: TUI retains queued delegation ownership
- **WHEN** the TUI submits a named agent mention or explicit `/subtask`
- **THEN** it SHALL save source Session, Location and client-generated request identity before submission, refusing submission if persistence fails
- **AND** it SHALL expose pending admissions separately from child Jobs and cancel only the selected source/request, with bounded acknowledgement and explicit uncertainty
- **AND** reconnect and lost-response recovery SHALL look up the saved request without resubmitting its prompt
- **AND** an acknowledged cancellation SHALL remain terminal when a delayed submission response arrives
- **AND** cancellation of a recorded Job SHALL verify its ownership and terminal acknowledgement

#### Scenario: Mention runs subagent
- **WHEN** the user submits `@explore where is retry logic implemented?`
- **THEN** an `explore` child Session is spawned with that question and its result is shown in the parent

### Requirement: Agent thread switching
(P1) Clients SHALL let the user view and continue any child Session: the `/agent` command (alias `/subagents`) SHALL list children with status (`running`, `waiting`, `completed`, `failed`) and switch the view to the selected thread; prompts typed there SHALL be admitted to that child.

#### Scenario: Steer a running subagent
- **WHEN** the user switches to a running `explore` thread and types a hint
- **THEN** the hint is admitted to the child with `delivery: steer`

### Requirement: Agent selection on the command line
(P0) `cyber --agent <name>` and `cyber exec --agent <name>` SHALL select the agent for the new Session, failing with exit code 2 when the agent is unknown, hidden or `mode: subagent`.

#### Scenario: Invalid CLI agent
- **WHEN** the user runs `cyber exec --agent explore "fix it"`
- **THEN** the command exits with code 2 and an error that `explore` cannot run a primary session

### Requirement: Agent management commands
(P0) `cyber agents list` SHALL print each agent as `<name> (<mode>) <model or "inherit">` with built-ins first, and `--json` SHALL print full resolved definitions. `cyber agents create` SHALL generate a Markdown agent from a description using the default model, accepting `--name`, `--description`, `--mode`, `--tools` and `--scope project|global`, prompting for missing values, and writing to `.cyber/agents/` or `~/.config/cyber/agents/`.

#### Scenario: Non-interactive creation
- **WHEN** the user runs `cyber agents create --name docs --description "writes docs" --mode subagent --tools read,edit --scope project`
- **THEN** `.cyber/agents/docs.md` is written with frontmatter and a generated system prompt and its path is printed

### Requirement: Agent tool catalogue
(P1) The `agent` tool description SHALL list every visible agent whose mode is `subagent` or `all` and whose `agent` permission is not denied for the caller, sorted by name, formatted `- <name>: <description>`, and SHALL be regenerated per Turn so newly loaded agents appear without restart.

#### Scenario: Denied agent omitted
- **WHEN** the caller's rules deny `agent` for resource `general`
- **THEN** `general` does not appear in the `agent` tool description

## ADDED Requirements

### Requirement: Requested child inbox handoff
(P1) Requested queued child input SHALL continue in FIFO order through fresh schema-preserving attempts after the preceding result owner finishes collection. Foreground tool output and background Job result and billing SHALL preserve their original attempt. Handoff SHALL use exclusive child ownership and verified checkout preparation, retain original inbox identity and edited content, and recheck pending wake intent before committing the next attempt. `resume: false` and historical input without wake intent SHALL remain deferred. Interruption SHALL durably clear automatic intent without deleting pending input, permitting a later explicit wake. Runtime shutdown SHALL join owned handoff preparation tasks.

#### Scenario: Foreground structured result followed by queued prompts
- **WHEN** two requested prompts are queued while a foreground structured child runs
- **THEN** the foreground tool receives its original result before the queued prompts run as separate attempts in admission order

#### Scenario: Background result billing precedes handoff
- **WHEN** a background structured child has requested queued input when its Job completes
- **THEN** the Job persists its original result and usage before a new attempt adds usage to the child

#### Scenario: Interrupted queued input remains deferred
- **WHEN** a child is interrupted with requested pending input and the server restarts
- **THEN** the input remains pending without automatic inference until an explicit wake

### Requirement: Child continuation preparation ownership
(P1) Existing-child preparation for public prompts, wake, held release and automatic handoff SHALL retain exclusive execution ownership through native acknowledgement and admission. Caller disposal SHALL cancel preparation without releasing ownership early. Child interruption SHALL record a durable pause boundary, cancel registered preparation and await bounded acknowledgement; preparation registration and admission SHALL reject an older boundary. A late successful result after cancellation SHALL NOT admit input, release held input or start inference. Unacknowledged or panicked preparation SHALL retain unknown recovery evidence and refuse execution after restart. Shutdown SHALL cancel and join owned native preparation.

#### Scenario: Interrupt native setup before new input
- **WHEN** interruption occurs while existing-child checkout setup is running
- **THEN** setup is cancelled and acknowledged, the prior inbox is preserved and neither late setup effects nor new inference occur

#### Scenario: Disposed caller with delayed acknowledgement
- **WHEN** the caller disappears while preparation is running and the host delays its cancellation acknowledgement
- **THEN** execution ownership remains held until the owned preparation settles

#### Scenario: Unacknowledged preparation after restart
- **WHEN** preparation fails to acknowledge cancellation or panics and the server restarts
- **THEN** durable unknown evidence refuses prompt, wake and resume pending recovery

### Requirement: Captured child launch authority
(P1) Child launches SHALL capture source and ancestor authority before approval, concurrency waiting and native preparation. Ordinary interruption SHALL invalidate outstanding launches sourced by the interrupted Session without invalidating independent background-child launches. A durable subtree admission fence SHALL invalidate outstanding launches from every descendant. Child creation SHALL recheck the captured identities and boundaries in its writer transaction and refuse stale authority without creating a Session. Missing or cyclic ancestry and foreign runtime authority SHALL be refused.

#### Scenario: Cancel between preflight and writer commit
- **WHEN** a source interruption commits after a child creation preflight but before its writer transaction
- **THEN** the writer refuses stale authority and creates neither child history nor a Session projection

#### Scenario: Background independence and subtree fencing
- **WHEN** a parent Drain is interrupted while an independent background child holds launch authority
- **THEN** the child authority remains valid until an explicit subtree admission fence commits for an ancestor

### Requirement: Delegation callback authority
(P1) Explicit user delegation and owned child-preparation callbacks SHALL retain their original source/ancestor authority and caller cancellation through host awaits. A delayed callback SHALL NOT recapture fresh authority after cancellation. Nested callback work SHALL retain the original source boundary until execution handoff; independent child Drains SHALL NOT inherit that callback scope. Durable reservation/launch markers, callback-owned input and resumed attempts, and background Job registration SHALL recheck captured boundaries in the single writer transaction. A refused pre-launch request SHALL remain Reserved and settle Cancelled; terminal cancellation evidence SHALL remain writable after authority is fenced.

#### Scenario: Delayed user host callback
- **WHEN** the source is interrupted or an ancestor subtree boundary commits before the host dispatches a delegated child
- **THEN** neither a child, input nor a Job is created using newly captured authority

#### Scenario: Cancelled callback owner
- **WHEN** a caller cancels while its host delays acknowledgement
- **THEN** callback-local admission refuses further child effects and a durable pre-launch reservation settles Cancelled

#### Scenario: Nested callback and independent child
- **WHEN** a callback creates a child before source interruption and attempts nested work afterward
- **THEN** nested callback admission refuses the old source boundary while subsequent independent child work retains ordinary background semantics

#### Scenario: Ancestor fence during child preparation
- **WHEN** an ancestor admission fence commits while public child preparation is awaiting its host
- **THEN** late success cannot reset the result, release held input, admit a prompt or infer using the older authority

#### Scenario: Job registered before delayed reply
- **WHEN** a background Job registers before ordinary parent interruption while its delegation reply remains delayed
- **THEN** ordinary parent interruption does not cancel the registered Job and its terminal admission receipt remains writable

#### Scenario: Staged conversation changes before refused input
- **WHEN** a fenced callback attempts admission into a Session with a staged conversation revert
- **THEN** admission refuses before committing that revert and preserves the prior history and staged state

#### Scenario: Revoked request during Location admission
- **WHEN** a durable request is cancelled after child creation preflight while its Location claim waits
- **THEN** the writer checks the request's current pending authority and rolls back child creation without registering a Job

#### Scenario: Missing admission owner at launch marker
- **WHEN** a Reserved admission has lost its live actor through restart or failure
- **THEN** a launch-marker call refuses before advancing the record and preserves Unknown lookup and reviewed cancellation semantics

### Requirement: Owned legacy subtask callback
(P1) Direct user subtask requests SHALL share the tracked durable callback owner with explicit delegation. Caller disposal SHALL cancel the request without disposing the host before native acknowledgement. Caller cancellation SHALL invalidate durable request authority. Callback-owned Job delivery SHALL require caller acceptance; disposed or cancelled delivery SHALL stop only the verified owned Job and await settlement. Accepted delivery SHALL transfer ownership to the independent Job. Hosts without the launch-marker contract SHALL retain conservative post-launch uncertainty.

#### Scenario: Disposed direct request before native acknowledgement
- **WHEN** a direct subtask caller disappears while its host delays cancellation acknowledgement
- **THEN** the host remains tracked until acknowledgement and cannot create new child work under revoked authority

#### Scenario: Accepted Job remains independent
- **WHEN** a direct subtask returns an accepted Job and its former caller token is later cancelled
- **THEN** the registered Job keeps running

#### Scenario: Rejected handoff without cancellation acknowledgement
- **WHEN** a rejected or cancelled direct handoff returns a recorded Running Job without a live cancellation owner
- **THEN** the request records Unknown with the verified Job identity and a cancelled caller receives a cancellation acknowledgement error

### Requirement: Durable subtree admission closure
(P1) An owned subtree cancellation SHALL close admission durably while its actors settle. Fresh capture and unbound writer admission SHALL check every ancestor closure. Admission SHALL check the target child as well as the source parent. Generation-only fences SHALL NOT reopen a closed scope. New input, release/reset, child or Job registration, Step/tool dispatch, compaction and new idle native work SHALL refuse closed ancestry. Terminal effect/cancellation receipts SHALL remain writable. Restart SHALL preserve closed admission until verified sweep settlement or reviewed recovery. The durable close primitive SHALL NOT imply completed actor cancellation.

#### Scenario: Fresh request during closed sweep
- **WHEN** a root or descendant receives fresh work after the scope is durably closed
- **THEN** capture and the writer refuse it without projecting input, children or Jobs

#### Scenario: Parent outside closed child scope
- **WHEN** an open parent tries to register a Job targeting a closed child
- **THEN** the writer refuses the target handoff and rolls back Job registration

#### Scenario: Closure survives restart and another generation fence
- **WHEN** the runtime restarts or a generation-only fence is added after scope closure
- **THEN** new descendant work remains refused

#### Scenario: Shell effect already started before closure
- **WHEN** an already-owned native shell finishes after admission closes
- **THEN** its internal output receipt remains durable without waking inference or permitting new prompt admission

### Requirement: Owned bounded subtree stop
(P1) Explicit subtree stop SHALL close admission before taking its descendant snapshot and SHALL retain the sweep after caller disposal. It SHALL signal all known scoped Drains, preparations, callback admissions, idle native operations and Jobs before waiting for any actor; Job target ownership SHALL count independently of its parent. Inbox pause intent SHALL remain durable and preserve rows. Acknowledgement SHALL be bounded and SHALL depend on terminal actor/effect evidence, not caller disposal. Missing owners, native uncertainty and held child-result ownership SHALL retain closed admission and a durable Unknown report. A local acknowledgement report SHALL NOT imply verified reopening or complete cross-process ownership.

#### Scenario: Concurrent scoped actors
- **WHEN** several descendant actors and parent-owned Jobs are active during subtree stop
- **THEN** all receive cancellation before the sweep waits and unrelated actors remain active

#### Scenario: Disposed sweep caller
- **WHEN** the caller disappears after admission closes
- **THEN** the tracked sweep continues to write its bounded settlement report

#### Scenario: Idle native operation lacks acknowledgement
- **WHEN** a scoped shell or maintenance operation is disposed or fails to acknowledge cancellation
- **THEN** its durable pending/unknown receipt prevents successful acknowledgement and admission stays closed
