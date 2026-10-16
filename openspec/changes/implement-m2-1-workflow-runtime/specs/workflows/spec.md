## MODIFIED Requirements

### Requirement: Workflow module format
(P2) The system SHALL accept a Workflow as an ES module written in JavaScript (`.js`). TypeScript (`.ts`) modules SHALL be listed by discovery with `UnsupportedSyntax: TypeScript Workflows are not supported yet; save the script as .js` until a type-strip step is specified. The module SHALL export `meta` as `export const meta = <literal>`, where the literal is an object `{ name, description, phases?, args? }` built only from objects, arrays, strings, template literals without substitutions, numbers, booleans and `null` (no variables, calls, spreads, computed keys or interpolation). The module SHALL also export a default async function, or contain top-level code (top-level `await` allowed), that forms the script body; a script without a default function returns `null`. `meta.name` SHALL match `^[a-z0-9][a-z0-9._-]{0,63}$`, `meta.description` SHALL be non-empty, and each phase SHALL be an object `{ title }`. Scripts SHALL be at most 1 MiB. A non-literal `meta` SHALL fail with `ParseError: meta must be a pure literal` at the line and column of the offending token. Objects and arrays in `meta` SHALL nest at most 64 levels deep; deeper nesting SHALL fail with `ParseError: meta nests objects and arrays more than 64 levels deep`, wherever the module comes from (inline, the API or discovery), without exhausting the server's stack.

#### Scenario: Valid module
- **WHEN** a module exports `meta = { name: "audit", description: "Audit auth", phases: [{ title: "Scan" }] }` and a default async function
- **THEN** the Workflow loads and its name, description and phases are shown before the run starts

#### Scenario: Computed meta rejected
- **WHEN** `meta.name` is built with a template literal containing a variable
- **THEN** loading fails with `ParseError: meta must be a pure literal` and no run is created

#### Scenario: Deeply nested meta rejected
- **WHEN** a saved module declares `meta` with an array nested 400,000 levels deep
- **THEN** discovery reports it as a `ParseError` issue and lists the other Workflows, and the server keeps running

#### Scenario: TypeScript module listed as unsupported
- **WHEN** `.cyber/workflows/audit.ts` exists
- **THEN** `cyber workflows list` reports it as an issue with `UnsupportedSyntax` and lists the other Workflows

### Requirement: Sandboxed QuickJS runtime
(P2) The system SHALL execute Workflow scripts in an embedded QuickJS context per run, with no filesystem, network, process, module import (other than the host API), `eval`, `Function` constructor (including the constructors reachable through function prototypes), timers, `WebAssembly`, `WeakRef`, `FinalizationRegistry`, `SharedArrayBuffer` or `Atomics`. `import`, `export … from` and `require(…)` SHALL be rejected before execution with `UnsupportedSyntax` naming the specifier. Script memory SHALL be limited by `workflows.max_memory_mb` (default 256, range 16–4096), and exhaustion SHALL fail with `ExecutionFailure: memory limit of <N> MiB exceeded`. Per-run script CPU time SHALL be limited by `workflows.script_cpu_seconds` (default 60, range 1–3600), measured as the elapsed time of synchronous script execution (module evaluation, promise jobs and settling host results), excluding time spent awaiting host calls. Describing an uncaught error runs script code (`toString`, getters, proxies) and SHALL be charged to the same limit. What the script queues for the host outside the QuickJS heap SHALL be bounded as the script queues it: at most 10,000 host calls pending, their payloads and queued phase titles within the memory limit, each queued entry charged its text plus a fixed per-entry overhead so many tiny entries are bounded like a few large ones (a refused call throws `ExecutionFailure` synchronously), and log text past 1 MiB dropped after one `[log truncated at 1 MiB]` marker.

#### Scenario: Hostile error description stopped
- **WHEN** a script throws `{ toString() { for (;;) {} } }`
- **THEN** the run fails with `Timeout` once the script CPU limit is spent

#### Scenario: Synchronous call loop bounded
- **WHEN** a script calls `agent(s)` with a 1 MiB `s` in a synchronous loop under a 64 MiB limit
- **THEN** the call past the limit throws `ExecutionFailure` and server memory stays bounded

#### Scenario: Synchronous phase loop bounded
- **WHEN** a script calls `phase(i % 2 ? 'a' : 'b')` in a synchronous loop
- **THEN** the phase past the memory limit throws `ExecutionFailure` and server memory stays bounded

#### Scenario: Filesystem access unavailable
- **WHEN** a script calls `require("fs")` or `import("node:fs")`
- **THEN** the run fails with `UnsupportedSyntax` naming the forbidden import

#### Scenario: Busy loop stopped
- **WHEN** a script runs `while(true){}`
- **THEN** after 60 seconds of script CPU the run fails with `Timeout` and in-flight agents are cancelled

#### Scenario: Awaiting agents is not charged
- **WHEN** a script with `script_cpu_seconds: 1` awaits agents for ten minutes while running little JavaScript
- **THEN** the run is not stopped by the script CPU limit

#### Scenario: Large allocation stopped
- **WHEN** a script keeps allocating strings past `workflows.max_memory_mb`
- **THEN** the run fails with `ExecutionFailure: memory limit of 256 MiB exceeded`

### Requirement: Phases, logs and checkpoints
(P2) `phase(title)` SHALL set the current phase for subsequent `agent()` calls and the run monitor; titles not declared in `meta.phases` SHALL be appended. A title SHALL be at most 200 characters (a longer one throws `ExecutionFailure`), a run SHALL have at most 256 distinct titles, declared ones included (a new title past it fails the run with `ExecutionFailure`), and calling `phase()` with the current title SHALL NOT be recorded again. A recorded phase change SHALL carry the phase list only when it adds a title. `log(...values)` SHALL append JSON-safe values to the run log (max 1 MiB, then truncated with a marker). `checkpoint(name, value)` SHALL durably record a JSON-safe value of at most 1 MiB that is returned unchanged by the same call on resume; a larger value fails the run closed with `ExecutionFailure`. A run SHALL write at most 100,000 durable records (host calls, agent starts and settlements, phase changes and logs) and at most 256 MiB of record data; past either bound it fails closed with `ExecutionFailure`, so a script cannot fill the store.

#### Scenario: Monitor shows phase
- **WHEN** a script calls `phase("Verify")` before spawning agents
- **THEN** those agents appear under `Verify` in the run monitor

#### Scenario: Phase loop
- **WHEN** a script calls `phase('p' + i)` and awaits a checkpoint in an endless loop
- **THEN** 255 phase changes beyond the declared title are recorded and the run fails with `ExecutionFailure` at the 257th distinct title

#### Scenario: Oversized checkpoint
- **WHEN** a script awaits `checkpoint('big', s)` where `s` serializes to more than 1 MiB
- **THEN** the run fails closed and nothing is recorded for the call

### Requirement: Concurrency limit
(P2) The system SHALL run at most `workflows.max_concurrent` (default 16, range 1–256, reduced to the number of CPU cores when lower) workflow agents at once across all runs of a server, queueing further `agent()` calls FIFO. `parallel({ concurrency })` SHALL further limit a group but SHALL NOT exceed the global limit. A `concurrency` that is not a positive integer SHALL reject with `ExecutionFailure`.

#### Scenario: Large fan-out queued
- **WHEN** a script launches 100 agents in parallel with the default limit on a 32-core machine
- **THEN** 16 run at a time and the rest wait

### Requirement: Model tiers and mixing
(P2) The `model` option of `agent()` SHALL accept a model ref or a tier name defined in `workflows.tiers` (defaults: `fast` → `model_roles.small`, `smart` → `model_roles.default`, `verify` → `model_roles.advisor` or the default model). Tier names SHALL match `^[a-z][a-z0-9_-]{0,31}$`, and each tier value SHALL be a model ref `provider/model[#variant]` or `role:<role>` naming a model role (`default`, `small`, `advisor`, `evaluator`, `compaction`, `title`); other values SHALL be configuration errors. Different calls in one run MAY use different providers. An unknown tier SHALL reject the call with `ModelUnavailable`.

#### Scenario: Cheap fan-out, strong verification
- **WHEN** a script runs reviewers with `model: "fast"` and verifiers with `model: "verify"`
- **THEN** reviewer sessions use the small model and verifier sessions use the advisor model, and run cost is reported per model

#### Scenario: Invalid tier rejected at load
- **WHEN** `workflows.tiers` maps `fast` to `role:huge`
- **THEN** configuration loading fails naming `workflows.tiers.fast`

### Requirement: Diagnostics
(P2) Failures SHALL be reported with exactly one diagnostic kind from `ParseError`, `UnsupportedSyntax`, `ExecutionFailure`, `AgentFailed`, `SchemaMismatch`, `BudgetExceeded`, `ModelUnavailable`, `NoInteractiveUser`, `Timeout`, `PermissionDenied` and `WorkflowReplayMismatch`, including a message and, for script errors, the source line and column. Errors raised by host calls SHALL expose their kind to the script as `error.kind`; a kind set by the script on its own error SHALL be reported as `ExecutionFailure`.

#### Scenario: Syntax error location
- **WHEN** a script has a missing closing brace on line 12
- **THEN** the run fails with `ParseError` and location `12:1`

#### Scenario: Forged kind ignored
- **WHEN** a script throws an `Error` whose `kind` property it set to `BudgetExceeded`
- **THEN** the run fails with `ExecutionFailure`

### Requirement: Run monitor
(P2) The `/workflows` view and `GET /api/v1/workflows/runs` SHALL list runs with name, status, current phase, agents `done/running/queued/failed`, tokens, cost and elapsed time, and SHALL let the user open any agent's transcript, pause (`p`), resume, stop (`x`, as in the `tui` Workflow monitor) and open the script. Live updates SHALL stream over the event stream.

#### Scenario: Inspect an agent
- **WHEN** the user selects a running agent in the monitor
- **THEN** its child Session transcript opens with live updates

#### Scenario: Stop key
- **WHEN** the user presses `x` on a running run in `/workflows`
- **THEN** the run stops and its in-flight agents are cancelled

### Requirement: Write isolation default
(P2) When more than one concurrently running workflow agent has edit permission in the same Location, the runtime SHALL default those agents to `isolation: "worktree"` unless the call sets `isolation: "none"` explicitly, and SHALL report each agent's branch in its result. An agent has edit permission when its agent profile's tool set offers an edit tool (`write`, `edit`, `apply_patch` or `notebook_edit`) and the run Mode is not `plan`. Agent calls issued together (one `parallel()` fan-out) SHALL count as concurrent before the first of them starts. The default SHALL apply only in a git checkout, where a worktree can isolate the agent; outside one, agents stay in the Location. Creating a run's worktree SHALL obey deny rules for `worktree` but SHALL NOT ask, because the user approved the run.

#### Scenario: Concurrent writers isolated
- **WHEN** a script launches 5 `general` agents in parallel without `isolation`
- **THEN** each runs in its own worktree and its result includes `branch`

#### Scenario: Single writer in place
- **WHEN** a script awaits one `general` agent at a time
- **THEN** each agent works in the owner's checkout and its result has no `branch`

#### Scenario: Explicit opt-out
- **WHEN** a script launches 2 agents in parallel with `isolation: "none"`
- **THEN** both work in the owner's checkout

### Requirement: Permissions for runs
(P2) Starting a run SHALL require the `workflow.run` permission (`ask` by default in `default` Mode, `allow` in `auto` and `bypass`) on the workflow's origin-qualified resource: its name for a global or bundled saved Workflow, `project:<name>` for a project one and `inline:<name>@<digest>` for an inline script. A script declares its own name, so a rule or an "always" approval for one origin SHALL NOT cover a script of another origin, or another inline script, declaring the same name. A deny rule for the bare name SHALL refuse every origin. Starting or resuming a run over the API or the CLI SHALL honour the owner Session's deny rules for both resources and answer `403` with reason `PermissionDenied`; the authenticated client approves the run by asking for it, so `ask` lets it start. The default SHALL be an explicit rule whose source inspection reports as `default`; a configured `workflow.run` rule replaces it, and in `auto` Mode only the default rule allows without the classifier. The approval request SHALL carry the workflow name, phases and budgets. Agents of the run SHALL run under the run's Mode as a ceiling: a resume SHALL use the stricter of the Mode recorded at start and the owner Session's current Mode. A run started non-interactively (over the API or with `cyber workflows run`) SHALL only use permissions pre-approved by rules, because any `ask` from an agent (or its subagents) SHALL be auto-rejected with feedback `No interactive approver for workflow run`. A workflow agent SHALL NOT start a Workflow Run: the server SHALL refuse a run whose owner Session, or any of its ancestors, belongs to a run (`409`, `Workflow agents cannot start Workflow Runs`), whether through the `workflow` tool, the API or the CLI, and SHALL refuse it when that ancestry cannot be read.

#### Scenario: Approval before start
- **WHEN** the model calls the `workflow` tool in `default` Mode
- **THEN** the user sees the workflow name, phases and budgets and approves or rejects before any agent starts

#### Scenario: Inline script named after an allowed Workflow
- **WHEN** `workflow.run` allows `audit` and the model starts an inline script declaring `meta.name: 'audit'`
- **THEN** the user is asked before any agent starts

#### Scenario: Deny rule over the API
- **WHEN** `workflow.run` denies `migrate` and a client runs `cyber workflows run migrate`
- **THEN** the start is refused with `403 PermissionDenied` and no run is created

#### Scenario: Mode widened before a resume
- **WHEN** a run started from a `plan` Session is stopped, the Session switches to `bypass` and the run is resumed
- **THEN** its agents still run in `plan`

#### Scenario: Nested run over the API
- **WHEN** a client posts `/workflows/runs` with the child Session of a workflow agent, or a Session below it, as owner
- **THEN** the start is refused with `409` and no run is created

#### Scenario: Non-interactive ask rejected
- **WHEN** an agent of a run started with `cyber workflows run` calls `bash` in `default` Mode
- **THEN** the call is rejected at once with `No interactive approver for workflow run` and the agent continues

### Requirement: Model-authored workflows
(P2) The model SHALL be able to start a Workflow through a `workflow` tool that takes either `name` + `args` or inline `script` + `args` (and optional `budget` fields). Inline scripts SHALL be persisted to `<state>/workflows/<run_id>.js` (JavaScript, see Workflow module format) and listed with the run as `script_file`, so that the user can read, edit and relaunch them, or save them into `.cyber/workflows/`. A resume SHALL read `script_file` again when it exists, like a saved Workflow, so an edited script needs `revision: true` and then runs as a new revision; a run whose file is gone resumes its stored source. The tool SHALL return the run handle (`run_id`, name, status, phases and budget) without waiting for the run.

#### Scenario: Save an inline workflow
- **WHEN** a run started from an inline script completes and the user chooses Save
- **THEN** the script is written to `.cyber/workflows/<meta.name>.js`

#### Scenario: Inline script kept with the run
- **WHEN** the model starts an inline script through the `workflow` tool
- **THEN** the run lists `script_file` at `<state>/workflows/<run_id>.js` with the script's source

#### Scenario: Edited inline script relaunched
- **WHEN** the user edits a stopped run's `script_file` and resumes it with `revision: true`
- **THEN** the run continues as revision 2 with the edited script's digest and behaviour

### Requirement: Run results
(P2) A completed run SHALL produce its script return value (JSON-safe, max 1 MiB, otherwise stored as `<state>/workflows/<run_id>.result.json` with a preview), per-agent child Sessions with transcripts, and a summary message admitted to the starting Session with `delivery: queue`, source `workflow` and message ID `msg_workflow_<run_id>` (`msg_workflow_<run_id>_<n>` for the attempt after the `n`th resume), containing the return value preview, agent counts, cost and duration. Failed, budget-exceeded and stopped runs SHALL be summarized the same way. The starting Session SHALL receive exactly one summary per ended attempt, including across server restarts; a resume SHALL clear the previous attempt's result, result file and preview, so a resumed run that ends again sends a new summary with its own result. A run started by the Session's `workflow` tool SHALL make the Session continue if idle; a run started over the API SHALL leave the summary queued without waking the Session.

#### Scenario: Result handback
- **WHEN** a run started from a Session completes
- **THEN** the Session receives a queued message with the run summary and continues if idle

#### Scenario: Large result
- **WHEN** a script returns 2 MiB of JSON
- **THEN** the run reports `result_file` and `result_preview` and the summary carries the preview

#### Scenario: Resumed run summarized again
- **WHEN** a run started by the `workflow` tool ends `budget_exceeded` with a partial result over 1 MiB, is resumed with a raised budget and completes with a small result
- **THEN** the Session receives a second summary, and the run reports the small result without the earlier `result_file`

#### Scenario: Restart before the summary was recorded
- **WHEN** the server stops after admitting a summary but before recording it
- **THEN** the next start does not admit a second summary

### Requirement: Workflow CLI
(P2) The CLI SHALL provide `cyber workflows list` (saved and bundled), `run <name|file> [--args JSON] [--max-cost N] [--max-turns N] [--max-tokens N] [--timeout D] [--max-agents N] [--runner <id>] [--wait]`, `runs` (recent runs of the Location), `show <run_id>`, `pause <run_id>`, `resume <run_id> [--revision]`, `stop <run_id>` and `logs <run_id>`. The run commands SHALL talk to the shared server, which owns the run; `run` without `--wait` prints the run ID. `run --wait` SHALL stream progress and exit 0 on `completed`, 1 on `failed` (and on `stopped` or `paused` by another client), 4 on `BudgetExceeded`, and 130 when interrupted, stopping the run; an interrupt SHALL be honoured whether it arrives during a poll or between polls. `--runner` SHALL be rejected as unavailable until remote runners ship (P3).

#### Scenario: CI usage
- **WHEN** CI runs `cyber workflows run audit --wait --max-cost 10`
- **THEN** the command blocks until the run ends and exits with the mapped code

#### Scenario: Runner unavailable
- **WHEN** a user runs `cyber workflows run audit --runner r1`
- **THEN** the command exits 2 saying `--runner` is not available in this build yet, and no run starts

### Requirement: Run lifecycle states
(P2) A Workflow Run (`run_` ID) SHALL have status `queued`, `running`, `waiting_input`, `paused`, `completed`, `failed`, `budget_exceeded` or `stopped`. Pause SHALL stop scheduling new agents while letting in-flight agents finish; an `agent()` call still waiting for a slot of the global limit when the run pauses SHALL NOT start, and stays unrecorded so a resume serves it. Stop SHALL cancel in-flight agents and mark them `cancelled`, and a script that ends (completed or failed) while agents are in flight SHALL cancel them and mark them `cancelled` too; after a crash, the next server process to hold the per-user server lock SHALL release child execution ownership the crashed server held, so the run's interrupted agents can resume their child Sessions; resume SHALL continue a `paused`, `failed`, `stopped` or `budget_exceeded` run through the resume mechanism. Transitions SHALL be durable events (`workflow.run.started.1`, `workflow.run.paused.1`, `workflow.run.resumed.1`, `workflow.run.completed.1`, `workflow.run.failed.1`, `workflow.run.stopped.1`), serialized with the script's own end: a run has at most one terminal event per attempt, a stop recorded first wins, and a script that ends on its own after a pause records its end and result.

#### Scenario: Pause lets agents finish
- **WHEN** the user pauses a run with 4 agents in flight
- **THEN** those 4 complete and are stored, and no new agent starts until resume

#### Scenario: Queued agents do not start after a pause
- **WHEN** a run with a global limit of 1 pauses while one agent runs and two wait for a slot
- **THEN** only the running agent finishes, and a resume starts the two others

#### Scenario: Paused while another run holds every slot
- **WHEN** run A holds every slot of the global limit and run B, whose only agent waits for a slot, is paused
- **THEN** B leaves the queue at once and can be resumed while A still holds the slots

#### Scenario: Stop racing the script's end
- **WHEN** the user stops a run whose script returns at the same moment
- **THEN** the run records exactly one of `stopped` or `completed`

#### Scenario: Resume refused while agents alive
- **WHEN** the user relaunches a stopped run whose cancelled agents have not exited yet
- **THEN** the relaunch is refused with `Run has agents still exiting; retry shortly`

### Requirement: Run budgets
(P2) A run SHALL accept the Budget object defined by `observability-costs` (`max_turns`, `max_tokens`, `max_cost_usd`, `max_wall_seconds` default 14400, `enforcement`) plus the workflow-specific `max_agents` (default 200), from `meta`, invocation options or `budgets.run`. `budgets.run` and the defaults SHALL be a ceiling: `meta.budget` and the `workflow` tool's `budget` MAY only lower a limit (a larger value keeps the current one, and `null` for a limit already set is refused), so neither a script nor the model can widen what the user configured. Only the authenticated client SHALL override a limit upward, explicitly, through the `budget` of an API (or CLI) start or resume. Past `max_wall_seconds`, the first host call other than `agent()` SHALL reject with `BudgetExceeded: max_wall_seconds` and any later one SHALL end the run, so a script without agents also ends at its wall budget. When a budget would be exceeded, new `agent()` calls SHALL reject with `BudgetExceeded` naming the budget, in-flight agents SHALL finish, and `ctx.budget` SHALL expose `{ spent_tokens, spent_cost_usd, agents_started, remaining }` to the script. `ctx.budget` SHALL be the snapshot taken when the revision started and refreshed each time a host call result reaches the script; it SHALL be recorded with the result and replayed as recorded, so a script that branches on it repeats the recorded attempt. Cost and token dispatch SHALL obey the soft/reserved enforcement and uncertain usage rules in observability-costs, including checks before each child provider Turn. A halted budgeted run SHALL enter `budget_exceeded`.

#### Scenario: Cost budget reached
- **WHEN** a run with `max_cost_usd: 5` has spent $5.02
- **THEN** the next `agent()` call rejects with `BudgetExceeded: max_cost_usd` and the script may return a partial result

#### Scenario: Model cannot lift a configured cap
- **WHEN** `budgets.run.max_cost_usd` is 1 and the model calls the `workflow` tool with `budget: { max_cost_usd: null }` or `{ max_cost_usd: 50 }`
- **THEN** the first call is refused and the second starts a run whose `max_cost_usd` is 1

#### Scenario: Checkpoint loop past the wall budget
- **WHEN** a script with `max_wall_seconds: 0.3` and no agents loops on caught `checkpoint()` calls
- **THEN** the first late call rejects with `BudgetExceeded: max_wall_seconds`, the next ends the run and it enters `budget_exceeded`

#### Scenario: Budget read during replay
- **WHEN** a script loops `while (ctx.budget.agents_started < 2)` and the run is paused and resumed after three agents started
- **THEN** the replay shows the recorded snapshots, issues the same calls and completes without `WorkflowReplayMismatch`

### Requirement: Replay of workflow host calls
(P2) Resumable runs SHALL persist results of all observable host calls, including ask, read-only tools, phase checkpoints and agent results, keyed by stable call identity and invocation sequence. A call SHALL get its sequence number when the script issues it, so every call of one batch (for example `Promise.all`) is sequenced before any is served, and a call the host rejects (invalid options, an unknown tier, `ask()` without an approver) SHALL be sequenced and recorded like the others. Replay SHALL deliver recorded results in their recorded order, whatever order the batch is served in, and SHALL return recorded results instead of rereading changed files or asking the user again. A run SHALL record script and configuration digests. An edited script SHALL create an explicit revision with reuse limited to compatible recorded calls; divergent host-call sequences SHALL fail with `WorkflowReplayMismatch` rather than silently reuse results. Unsettled agents SHALL resume their existing child Session and tool recovery records before any replacement is spawned. An agent counts as unsettled only while no later record settled its call; a call whose resumed agent failed SHALL be retried by a new agent, counted against `max_agents`, never by resuming the failed child again.

#### Scenario: Resumed agent failed
- **WHEN** revision 1 left an agent unsettled, revision 2 resumed it and it failed, and revision 2 is relaunched
- **THEN** the call runs as a new `agt_` agent charged against `max_agents`, without the interrupted-attempt prompt

#### Scenario: Files change while run is stopped
- **WHEN** a workflow resumes after its earlier glob result would now differ
- **THEN** replay uses the recorded glob result for that revision and resumes existing child Sessions

#### Scenario: Parallel results settled out of order
- **WHEN** `await Promise.all([agent("a"), agent("c")])` saw `c` settle before `a`, and the run is paused and resumed
- **THEN** replay delivers `c` then `a` from the store and the run continues without `WorkflowReplayMismatch`

#### Scenario: Caught rejection beside a parallel agent
- **WHEN** a script catches `ModelUnavailable` from one call and falls back to another while a parallel agent runs, and the run is resumed
- **THEN** the rejection is replayed in its place and the run completes without `WorkflowReplayMismatch`
