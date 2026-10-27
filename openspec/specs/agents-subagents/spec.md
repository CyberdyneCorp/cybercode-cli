# agents-subagents Specification

## Purpose
Agents are named profiles (system prompt, model, mode, tools, permissions, step limit) that drive a Session; subagents are agents run in child Sessions spawned by the `agent` tool so side work does not flood the parent's context. Cyber Code combines OpenCode's built-in agent set and Markdown agent files, Codex's per-agent model configuration and thread concurrency cap, and Claude Code's subagent features (structured results, background runs, forked context, worktree isolation, resume by name).

## Requirements

### Requirement: Built-in agents
(P0) The system SHALL ship the built-in agents `build` (primary, full tool access), `explore` (subagent, read-only: `read`, `glob`, `grep`, `webfetch`, `websearch` and read-only `bash`), `general` (subagent, full tool access except `todo`), and the hidden system agents `compaction`, `title`, `summary` and `evaluator`, which SHALL have every tool denied. Planning SHALL be the `plan` permission Mode (`permissions-modes`), not an agent; an agent definition MAY set `permission_mode: plan` to start Sessions in that Mode.

#### Scenario: Default agent set
- **WHEN** a user lists agents in a project with no agent configuration
- **THEN** `build`, `explore` and `general` are listed and the hidden system agents are omitted

#### Scenario: System agents cannot call tools
- **WHEN** the `title` agent's model emits a tool call
- **THEN** the call is settled as an error without executing and no permission request is created

### Requirement: Agent definition fields
(P0) An agent definition SHALL accept `description`, `system`, `model` (`provider/model[#variant]`), `variant`, `mode` (`primary`, `subagent` or `all`), `permission_mode` (one of the six Modes), `tools` (`allow` and `deny` lists of tool names or globs), `permissions` (ordered rules), `request` (a provider request overlay of `headers` and `body`, for example `temperature` or `top_p`, layered as defined by `provider-catalog`), `steps` (positive integer), `color`, `hidden`, `isolation` (`none` or `worktree`), `background` (boolean default for spawns), `memory` (`none`, `project` or `user`), `skills` (names preloaded into context: each named skill that is discovered, model-invocable and not denied for the agent SHALL be added as the `core/agent-skills` Context Source of that agent's sessions, framed as the `skill` tool returns it, and any other name SHALL be listed in `preload_issues` of `GET /api/v1/skills?agent=<name>` instead of being preloaded) and `mcp` (subset of configured MCP server names). Unknown fields SHALL be rejected by schema validation, naming the agent and field.

#### Scenario: Unknown field rejected
- **WHEN** an agent definition contains `temprature: 0.2`
- **THEN** config validation fails with an error naming the agent and the field `temprature`

#### Scenario: Tool deny list hides tools
- **WHEN** an agent sets `tools.deny: ["bash", "web*"]`
- **THEN** `bash`, `webfetch` and `websearch` are omitted from that agent's tool definitions for every Turn

#### Scenario: Request overlay applied
- **WHEN** an agent sets `request.body.temperature: 0.2`
- **THEN** its Turns send `temperature: 0.2` after provider, model and variant defaults

#### Scenario: Skills preloaded for the agent
- **WHEN** agent `scribe` sets `skills: ["notes", "missing"]` and only `notes` is discovered
- **THEN** a `scribe` session's provider request contains the `notes` skill body, a `build` session's does not, and `GET /api/v1/skills?agent=scribe` lists `missing` in `preload_issues`

### Requirement: Custom agents from config
(P0) The system SHALL read agents from the `agents` key of `cyber.jsonc`/`cyber.json`, keyed by name. An entry matching an existing agent SHALL patch only the fields it sets; a new name SHALL create an agent with `mode: "all"`; `disabled: true` SHALL remove the agent, including built-ins other than the hidden system agents.

#### Scenario: Override built-in model
- **WHEN** config sets `agents.explore.model` to `ollama/qwen3-coder`
- **THEN** `explore` subagents use that model and keep their built-in read-only rules

#### Scenario: Disable a built-in agent
- **WHEN** config sets `agents.explore.disabled` to `true`
- **THEN** `explore` is absent from listings, `@` autocomplete and the `agent` tool catalogue

### Requirement: Markdown agent files
(P0) The system SHALL load agents from `{agent,agents}/**/*.md` in every config directory (global `~/.config/cyber`, each `.cyber/` directory from the Location up to the project root, nearest wins). YAML frontmatter SHALL supply definition fields and the trimmed body SHALL become `system`; the name SHALL be the path relative to the `agents/` folder without extension unless frontmatter sets `name`. Agents imported from `.claude/agents/*.md` and Codex agent TOML files SHALL be loaded through the compat-import capability with the same precedence as global agents.

#### Scenario: Nested agent name
- **WHEN** `.cyber/agents/review/security.md` exists without a `name` field
- **THEN** an agent named `review/security` is registered with the file body as its system prompt

#### Scenario: Project file overrides global
- **WHEN** both `~/.config/cyber/agents/tester.md` and `.cyber/agents/tester.md` define `tester`
- **THEN** the project definition wins

### Requirement: Default agent selection
(P0) The system SHALL use the `default_agent` config value for prompts that name no agent, and SHALL fail configuration loading with a clear error when that agent does not exist, is hidden, or has `mode: "subagent"`. Without `default_agent`, `build` SHALL be used, else the first visible primary agent by name.

#### Scenario: Subagent as default rejected
- **WHEN** `default_agent` is set to `explore`
- **THEN** loading fails with `default_agent "explore" is a subagent and cannot run a primary session`

### Requirement: Agent step limit
(P0) When an agent sets `steps`, the system SHALL, on the Turn that reaches the limit, send the request with no tools and `tool_choice: none` plus an instruction to reply with a text summary of completed and remaining work; tool calls still emitted SHALL fail with `Tools are disabled after the maximum agent steps`. Promoting newly admitted user input SHALL reset the step count. Without `steps`, primary agents SHALL be unbounded except for goal and workflow budgets, and subagents SHALL default to 50 steps.

#### Scenario: Last step disables tools
- **WHEN** an agent with `steps: 5` reaches its 5th Turn in one Drain
- **THEN** that Turn is sent without tool definitions and the Drain ends after the model's text reply

#### Scenario: Subagent default limit
- **WHEN** a `general` subagent without `steps` completes its 50th Turn with tool calls pending
- **THEN** its next Turn has no tools and returns a summary to the parent

### Requirement: Agent tool spawns subagents
(P1) The system SHALL provide an `agent` tool with inputs `prompt` (required), `agent` (default `general`), `description` (3–8 words), `output_schema` (JSON Schema), `model`, `isolation` (`none`, `worktree` or, from P3, `remote`), `runner` (with `isolation: remote`: a pool, `rnr_` ID or peer name), `background` (boolean), `fork` (boolean) and `resume` (subagent name or `ses_` ID). Each spawn SHALL create a child Session whose `parent_id` is the caller, titled `<description> (@<agent>)`, and SHALL request the `agent` permission with the target agent name as resource.

#### Scenario: Foreground subagent
- **WHEN** the model calls `agent` with `agent: "explore"` and `background: false`
- **THEN** a child Session runs to completion and the tool result contains the subagent's final text and its `ses_` ID

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

### Requirement: Remote isolation
(P3) A spawn with `isolation: "remote"` SHALL create the child Session on the named `runner` (or `runners.default`) as defined by `runners-cloud`: the repository is materialized there, the child runs in its own worktree, and the handback carries the branch and diff summary plus a `runner` field. Permission requests from a remote child SHALL surface in the parent's client like local ones. The parent's Mode ceiling and deny rules SHALL apply on the Runner. Without a Cyber Account or peer able to host the Session, the spawn SHALL fail with `RunnerUnavailable`.

#### Scenario: Offload a long test run
- **WHEN** the model spawns `general` with `isolation: "remote"`, `runner: "ci-pool"` and `background: true`
- **THEN** the child runs on the pool, and when it finishes the parent receives a queued handback with its result, branch and `runner: "ci-pool"`

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

### Requirement: Durable child result ownership and target authority
(P1) Child result ownership SHALL be exclusive across runtimes sharing the database, and acquisition SHALL verify the target belongs to the declared parent. Acquisition SHALL persist original source authority and, for existing children, the original target identity and cancellation boundaries before callbacks or native preparation. The writer SHALL refuse a closed target independently of supplied bindings. Callback work and resumed admission SHALL retain this original authority through awaits and writer settlement. Existing-child continuation SHALL be sourced by the child so ordinary parent interruption does not revoke independent child work; an explicit subtree fence SHALL revoke it. Session setup SHALL capture source/target authority before Location admission and recheck it before each new native command, including after asynchronous sandbox preparation. Owner release SHALL revoke callback authority while preserving terminal release evidence after closure. A held or orphaned durable owner SHALL prevent subtree acknowledgement; releasing result ownership SHALL NOT itself prove native effects acknowledged.

#### Scenario: Another runtime holds the result owner
- **WHEN** a child owner belongs to another runtime or has not released before restart
- **THEN** another claim is refused and subtree stop records Unknown until ownership is acknowledged or recovered

#### Scenario: Target fence during parent callback
- **WHEN** the parent remains open but its originally claimed child is fenced while a callback waits
- **THEN** native preparation and child admission refuse the original target authority instead of capturing a fresh boundary

#### Scenario: Owner disposal after closure
- **WHEN** a child result owner is released after its source or target closes
- **THEN** its release receipt is durable and retained callback authority cannot launch additional work


#### Scenario: Setup admission waits across a target boundary
- **WHEN** target authority changes while a setup Location claim waits or between setup commands
- **THEN** the next effect is refused with original captured authority and a not-dispatched command receipt when journaled

#### Scenario: Independent child continuation
- **WHEN** the parent Drain is interrupted during child-sourced continuation
- **THEN** the child retains its authority until its own interruption or an explicit ancestor subtree fence

### Requirement: Retained native settlement proof
(P1) Verified admission reopening SHALL hold native Location ownership through the database commit that consumes its settlement evidence. A managed Location host SHALL explicitly support retained settlement; consuming settlement that releases ownership SHALL NOT be accepted as retained proof. A retained proof SHALL record acknowledged settlement while keeping the original native lock until disposal. Unsupported hosts and failed settlement SHALL refuse proof. This primitive SHALL NOT reopen admission or replace actor/effect audits.

#### Scenario: Native lock held after settlement
- **WHEN** a checkout lease records settlement for a retained proof
- **THEN** another native claimant cannot acquire its activity record until the proof is dropped
- **AND** the settled record remains durable after release

#### Scenario: Host without retained settlement
- **WHEN** a managed host implements only consuming settlement
- **THEN** retained proof is refused without silently invoking consuming settlement

#### Scenario: Retained host guard disposal
- **WHEN** a host supplies retained acknowledged ownership
- **THEN** the runtime preserves that guard until proof disposal
- **AND** unmanaged Locations require no managed checkout lock

### Requirement: Matched verified subtree reopening
(P1) Explicit reopening SHALL require the current close scope and latest persisted Acknowledged stop receipt. It SHALL replay fresh durable Session state, refuse open Steps, unsettled tool outcomes, pending preparation, running Jobs and unresolved delegation, idle or result ownership. Native Location identity and retained settlement proof SHALL remain held through the writer commit. The writer SHALL recheck the descendant identities, Session revisions and scoped ownership evidence atomically; changed evidence SHALL refuse reopening. Only the matching scope SHALL reopen. Existing captured authority SHALL remain revoked, independently closed descendants SHALL remain closed and reopening SHALL NOT dispatch preserved input automatically. Unknown receipts require settlement or reviewed recovery, not reopening.

#### Scenario: Matched acknowledged reopening
- **WHEN** the reviewed close scope and latest acknowledged receipt still match and fresh actor/native evidence is settled
- **THEN** the writer records reopening, retains input and allows fresh admission
- **AND** captured authority from before the stop remains invalid across restart

#### Scenario: Stale review or changed evidence
- **WHEN** the receipt is Unknown, belongs to another scope, is superseded or actor evidence changes during native verification
- **THEN** reopening is refused and admission stays closed

#### Scenario: Nested scopes and repeated stops
- **WHEN** an acknowledged ancestor scope reopens while a descendant has its own closed scope
- **THEN** the descendant remains closed
- **AND** a later stop creates a new close scope that an earlier reopening request cannot clear

#### Scenario: Owned sweep still pending
- **WHEN** another stop sweep has a durable start without its own terminal receipt, including after process loss
- **THEN** an earlier acknowledged observation SHALL NOT authorize reopening
- **AND** sweep settlement SHALL bind the original sweep and active close owner

#### Scenario: Callback crosses to an unrelated Session
- **WHEN** a callback captures authority for an unrelated Session before its original source hands off ownership
- **THEN** the callback and its writer submissions retain the original source boundaries
- **AND** stop and reopening audits retain the original operation scope after terminal records remove admission bindings
