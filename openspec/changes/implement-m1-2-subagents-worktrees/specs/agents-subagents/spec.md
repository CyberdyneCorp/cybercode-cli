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

### Requirement: Forked subagents
(P1) A spawn with `fork: true` SHALL create a child Session that inherits the parent's full projected history, Context Epoch baseline, agent and model (unless overridden), instead of starting with only the prompt. The `/subtask <prompt>` command SHALL spawn a forked background subagent; `/fork` SHALL copy the whole Session into a new independent top-level Session.

#### Scenario: Fork inherits context
- **WHEN** the user runs `/subtask try the alternative parser approach`
- **THEN** the child Session's first Turn includes the parent's history and the prompt, and the parent continues independently

### Requirement: Resume subagents by name
(P1) Each subagent SHALL get a unique name within its parent (the agent name, suffixed `-2`, `-3`… on collision, or a caller-supplied `name`). A spawn with `resume` SHALL admit the new prompt into the existing child Session instead of creating one, failing with `Subagent not found` for unknown names and `Subagent busy` when it is running in the foreground of another caller.

#### Scenario: Continue a previous subagent
- **WHEN** the model calls `agent` with `resume: "explore"` and a follow-up prompt
- **THEN** the prompt is admitted to the existing `explore` child Session, which keeps its earlier context

### Requirement: Worktree isolation for subagents
(P1) A spawn with `isolation: "worktree"` SHALL create a managed git worktree (see worktrees) on a branch `cyber/<parent-short-id>/<subagent-name>` and bind the child Session's Location to it. On completion the tool result SHALL report the branch, changed files and diff stats; the worktree SHALL be kept when it has changes and removed when clean, unless `worktrees.keep` is `always`.

#### Scenario: Isolated edits
- **WHEN** two subagents with `isolation: "worktree"` edit the same file concurrently
- **THEN** each edits its own checkout and both results report their branch names

#### Scenario: Clean worktree removed
- **WHEN** an isolated subagent finishes without changing files
- **THEN** its worktree and branch are deleted

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

### Requirement: Manual invocation by mention
(P1) An `@<agent>` mention of a subagent-capable agent in a user prompt SHALL spawn that agent through the `agent` tool flow with the rest of the prompt, without requiring the `agent` permission. In the TUI, `@` autocomplete SHALL offer visible agents with `mode` `subagent` or `all`.

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
