# workflows Specification

## Purpose
A Workflow is a JavaScript/TypeScript script that orchestrates many subagents: fan-out, pipelines, cross-verification and consolidation, executed in the background by a deterministic, resumable runtime. It brings Claude Code's dynamic workflows to any LLM and adds per-call model mixing, budgets and remote execution. The runtime builds on OpenCode's codemode ideas (a confined JS subset with structured diagnostics) and on Cyber Code's durable event store for resume. Codex has no equivalent; it orchestrates subagents turn by turn instead.

## Requirements

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

### Requirement: Host API surface
(P2) The runtime SHALL expose exactly these globals to scripts: `agent`, `parallel`, `pipeline`, `phase`, `log`, `ask`, `checkpoint`, `args`, `ctx` and, when allowlisted, `tools`. Any other global access to host capabilities SHALL be undefined.

#### Scenario: Unknown global
- **WHEN** a script calls `fetch("https://example.com")`
- **THEN** the run fails with `ExecutionFailure: fetch is not defined`

### Requirement: agent() call
(P2) `agent(prompt, opts?)` SHALL spawn a subagent child Session and resolve to its result, with options `agent` (default `general`), `model` (model ref or tier name), `schema` (JSON Schema; resolves to the validated object), `isolation` (`none` or `worktree`), `label`, `phase`, `tools` (allow list narrowing the agent's tools), `budget` (`{ tokens?, cost_usd?, turns? }`) and `goal` (a goal condition, see goals). Without `schema` it SHALL resolve to `{ text, session_id, cost, tokens }`. Each call SHALL get an `agt_` ID.

#### Scenario: Structured agent result
- **WHEN** a script awaits `agent("List risky endpoints", { schema: { type: "object", properties: { endpoints: { type: "array" } } } })`
- **THEN** the promise resolves to the validated object and the call is recorded with its `agt_` ID

#### Scenario: Agent failure rejects
- **WHEN** an agent's child Session ends with a provider error after retries
- **THEN** the promise rejects with an error whose `kind` is `AgentFailed` and that the script can catch

### Requirement: Parallel and pipeline helpers
(P2) `parallel(thunks, { concurrency? })` SHALL run an array of zero-argument async functions concurrently and resolve to their results in input order, rejecting only if `{ settle: true }` is not set and a thunk rejects. `pipeline(items, ...stages)` SHALL pass each item through the stages in order, starting stage N+1 for an item as soon as its stage N completes, independent of other items, and resolve to the final stage results in input order. Both SHALL respect the global concurrency limit.

#### Scenario: Pipeline does not wait for slowest item
- **WHEN** a pipeline has a review stage and a verify stage over 10 items
- **THEN** verification of item 1 starts as soon as item 1's review completes, even while other reviews run

#### Scenario: Settle mode collects failures
- **WHEN** `parallel(thunks, { settle: true })` runs and two thunks reject
- **THEN** it resolves to an array of `{ status: "fulfilled", value }` or `{ status: "rejected", reason }` entries

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

### Requirement: ask() user interaction
(P2) `ask(question)` SHALL accept a string or a structured question (`{ question, options?, multiple? }`), create a pending question routed to the run's owner through the question flow (including remote devices), pause the script until answered, and resolve to the answer. In non-interactive runs without a pre-declared answer in `args`, it SHALL reject with `NoInteractiveUser`.

#### Scenario: Script waits for answer
- **WHEN** a script calls `ask("Proceed with the migration of 120 files?")`
- **THEN** the run status becomes `waiting_input` and resumes when the user answers

### Requirement: Direct read-only tools
(P2) When the Workflow's meta or invocation allowlists them, `tools.read`, `tools.glob` and `tools.grep` SHALL be callable directly from the script without spawning an agent, with the same permission checks and output limits as the built-in tools, returning structured data. Mutating tools SHALL NOT be exposed to scripts.

#### Scenario: Script lists files to fan out
- **WHEN** a script calls `await tools.glob({ pattern: "src/**/*.rs" })`
- **THEN** it receives the list of matching paths and can spawn one agent per file

### Requirement: Determinism rules
(P2) To make relaunches replay the same calls, `Date.now()`, `Math.random()`, `performance.now()` and argument-less `new Date()` SHALL throw `ExecutionFailure: non-deterministic API; pass values through args`. `new Date(<value>)` with an argument SHALL remain available.

#### Scenario: Random rejected
- **WHEN** a script calls `Math.random()`
- **THEN** the run fails with the non-deterministic API diagnostic

### Requirement: Durable agent results and resume
(P2) The system SHALL key every `agent()` call by a hash of its prompt, normalized options and call ordinal for that (prompt, options) pair, and SHALL persist each settled result as a durable `workflow.agent.settled.1` event before resolving the promise. Relaunching a run (after pause, crash, server restart or script edit) SHALL re-execute the script from the start, resolving calls whose key already has a settled result immediately from the store, and resuming unsettled existing child Sessions before spawning calls with new or changed keys. Script edits and other host calls SHALL follow Replay of workflow host calls.

#### Scenario: Resume after crash
- **WHEN** the server restarts while a run has 30 of 50 agents settled
- **THEN** resuming the run returns the 30 cached results instantly and resumes the existing child Sessions for the remaining 20, reconciling any unknown tool outcomes

#### Scenario: Edited prompt reruns
- **WHEN** the user edits one agent prompt in the script and relaunches
- **THEN** only that call (and calls depending on its output) spawn new agents

### Requirement: Run lifecycle states
(P2) A Workflow Run (`run_` ID) SHALL have status `queued`, `running`, `waiting_input`, `paused`, `completed`, `failed`, `budget_exceeded` or `stopped`. `queued` SHALL mean the run waits for a run slot ("Run queue"); dispatch SHALL move it to `running`.

Pause SHALL stop scheduling new agents while letting in-flight agents finish. An `agent()` call still waiting for a slot of the global limit when the run pauses SHALL NOT start, and stays unrecorded so a resume serves it. Pausing a `queued` run SHALL remove it from the run queue.

Stop SHALL cancel in-flight agents and mark them `cancelled`, and a script that ends (completed or failed) while agents are in flight SHALL cancel them and mark them `cancelled` too. After a crash, the next server process to hold the per-user server lock SHALL release child execution ownership the crashed server held, so the run's interrupted agents can resume their child Sessions.

Resume SHALL continue a `paused`, `failed`, `stopped` or `budget_exceeded` run through the resume mechanism, and SHALL queue it when every run slot is taken.

Transitions SHALL be durable events (`workflow.run.started.1`, `workflow.run.dispatched.1`, `workflow.run.paused.1`, `workflow.run.resumed.1`, `workflow.run.completed.1`, `workflow.run.failed.1`, `workflow.run.stopped.1`), serialized per run with the script's own end: a run has at most one terminal event per attempt, a stop recorded first wins, and a script that ends on its own after a pause records its end and result. Serialization SHALL be per run, as "Concurrent runs" requires.

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

#### Scenario: Pause a queued run
- **WHEN** the user pauses a run with status `queued`
- **THEN** it becomes `paused` without any agent having started, and a later resume queues it again

### Requirement: Background execution on the server
(P2) Runs SHALL execute inside the `cyber` server, independent of any client connection, and the starting Session SHALL remain usable while the run proceeds. Closing the TUI SHALL NOT stop a run; stopping the server SHALL mark running runs `paused` for later resume.

#### Scenario: Client disconnect
- **WHEN** the user closes the terminal while a run is executing
- **THEN** the run continues and is visible on reconnect in `/workflows`

### Requirement: Concurrency limit
(P2) The system SHALL run at most `workflows.max_concurrent` (default 16, range 1–256, reduced to the number of CPU cores when lower) workflow agents at once across all runs of a server, queueing further `agent()` calls and sharing the slots between runs as "Fair agent scheduling" defines. `parallel({ concurrency })` SHALL further limit a group but SHALL NOT exceed the global limit or the run's share. A `concurrency` that is not a positive integer SHALL reject with `ExecutionFailure`.

#### Scenario: Large fan-out queued
- **WHEN** a script launches 100 agents in parallel with the default limit on a 32-core machine
- **THEN** 16 run at a time and the rest wait

#### Scenario: Limit shared by runs
- **WHEN** two runs each launch 50 agents with a limit of 16
- **THEN** at most 16 of their agents run at once in total

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

### Requirement: Model tiers and mixing
(P2) The `model` option of `agent()` SHALL accept a model ref or a tier name defined in `workflows.tiers` (defaults: `fast` → `model_roles.small`, `smart` → `model_roles.default`, `verify` → `model_roles.advisor` or the default model). Tier names SHALL match `^[a-z][a-z0-9_-]{0,31}$`, and each tier value SHALL be a model ref `provider/model[#variant]` or `role:<role>` naming a model role (`default`, `small`, `advisor`, `evaluator`, `compaction`, `title`); other values SHALL be configuration errors. Different calls in one run MAY use different providers. An unknown tier SHALL reject the call with `ModelUnavailable`.

#### Scenario: Cheap fan-out, strong verification
- **WHEN** a script runs reviewers with `model: "fast"` and verifiers with `model: "verify"`
- **THEN** reviewer sessions use the small model and verifier sessions use the advisor model, and run cost is reported per model

#### Scenario: Invalid tier rejected at load
- **WHEN** `workflows.tiers` maps `fast` to `role:huge`
- **THEN** configuration loading fails naming `workflows.tiers.fast`

### Requirement: Prompt-cache warm-up for fan-out
(P2) When a `parallel` group launches two or more agents whose requests share an identical cacheable prefix, the runtime SHALL start the first agent alone and release the others when its first provider response begins, so that the others read the shared cached prefix.

#### Scenario: Shared prefix
- **WHEN** 10 agents with the same agent, model and preamble are launched together
- **THEN** one starts immediately and nine start after its first response chunk arrives

### Requirement: Write isolation default
(P2) When more than one concurrently running workflow agent has edit permission in the same Location, whether the agents belong to one run or to several, the runtime SHALL default those agents to `isolation: "worktree"` unless the call sets `isolation: "none"` explicitly, and SHALL report each agent's branch in its result. An agent has edit permission when its agent profile's tool set offers an edit tool (`write`, `edit`, `apply_patch` or `notebook_edit`) and the run Mode is not `plan`. Agent calls issued together (one `parallel()` fan-out) SHALL count as concurrent before the first of them starts, and edit-capable calls issued and not yet settled by any run in the Location SHALL count. The default SHALL apply only in a git checkout, where a worktree can isolate the agent; outside one, agents stay in the Location. Creating a run's worktree SHALL obey deny rules for `worktree` but SHALL NOT ask, because the user approved the run.

#### Scenario: Concurrent writers isolated
- **WHEN** a script launches 5 `general` agents in parallel without `isolation`
- **THEN** each runs in its own worktree and its result includes `branch`

#### Scenario: Single writer in place
- **WHEN** a script awaits one `general` agent at a time and no other run has an edit-capable agent in the Location
- **THEN** each agent works in the owner's checkout and its result has no `branch`

#### Scenario: Explicit opt-out
- **WHEN** a script launches 2 agents in parallel with `isolation: "none"`
- **THEN** both work in the owner's checkout

#### Scenario: Writers of two runs isolated
- **WHEN** runs A and B in the same Location each await one `general` agent without `isolation`, and the two agents overlap in time
- **THEN** the agent that starts second runs in its own worktree and reports its `branch`

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

### Requirement: Saved and bundled workflows
(P2) The system SHALL discover saved Workflows from `.cyber/workflows/**/*.{ts,js}` (project), `~/.config/cyber/workflows/` (global) and plugins, and SHALL bundle `review` (multi-angle code review with verification), `audit` (codebase-wide bug or security sweep), `migrate` (file-by-file migration with build and test checks), `research` (cross-checked research with sources) `plan` (plans drafted from several independent angles, then merged), `solve` (independent candidate implementations with host-run checks and an independent judge, as defined by `verified-workflows`), and `integrate` (complementary commits, host checks on combined source and read-only review, as defined by `verified-integrations`). Project workflows SHALL override global and bundled ones with the same name.

Bundled Workflows SHALL be embedded in the `cyber` binary, carry the origin `bundled` with their name, and have the lowest precedence: global roots override them and project roots override both. A listing SHALL mark every entry's origin (`bundled`, `global` or `project`) and name the definitions it shadows. `workflows.bundled: false` or `CYBER_DISABLE_BUNDLED_WORKFLOWS=1` SHALL hide the bundled Workflows from listings, the `workflow` tool and runs by name. `cyber workflows show <name>` SHALL print a library entry's meta, args schema, default budgets and origin (and with `--source` its source), and `cyber workflows eject <name> [--global] [--force]` SHALL copy a bundled Workflow's source to `.cyber/workflows/<name>.js` (or the global root), refusing to overwrite an existing file without `--force`. A run of a bundled Workflow SHALL record its origin and source digest, and a release that changes a bundled source SHALL be treated on resume like an edited saved module.

#### Scenario: Run a bundled workflow
- **WHEN** the user runs `cyber workflows run review --args '{"target":"main..HEAD"}'`
- **THEN** the bundled review workflow starts against the diff between the resolved commits of `main` and `HEAD`

#### Scenario: Bundled listed on a fresh install
- **WHEN** a user with no saved Workflows runs `cyber workflows list`
- **THEN** `review`, `audit`, `migrate`, `research`, `plan`, `solve` and `integrate` are listed with origin `bundled`

#### Scenario: Ejected copy overrides the bundled one
- **WHEN** the user runs `cyber workflows eject audit`, edits `.cyber/workflows/audit.js`, and runs `cyber workflows run audit`
- **THEN** the edited project module runs, and `cyber workflows list` shows `audit` with origin `project` shadowing `bundled:audit`

#### Scenario: Eject refuses to overwrite
- **WHEN** `.cyber/workflows/review.js` exists and the user runs `cyber workflows eject review`
- **THEN** the command exits 1 naming the file and leaves it unchanged

#### Scenario: First verified workflow slice
- **WHEN** a release ships the shared library foundation and `solve` before the five other planned modules
- **THEN** only implemented modules are listed as runnable, and `solve` is available with its real args schema, origin and budgets

#### Scenario: Start a verified solve
- **WHEN** the user runs `cyber workflows run solve --args` with a valid task and required checks
- **THEN** the server starts the verified candidate/check/judge protocol under the effective run ceilings

#### Scenario: Customized integration script lacks native authority
- **WHEN** a user ejects or overrides `integrate` and calls its native host verification bridge
- **THEN** the host rejects that call unless the run has the exact embedded origin and source digest

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
(P2) A completed run SHALL produce its script return value (JSON-safe, max 1 MiB, otherwise stored as `<state>/workflows/<run_id>.result.json` with a preview), per-agent child Sessions with transcripts, and a summary message admitted to the starting Session with `delivery: queue`, source `workflow` and message ID `msg_workflow_<run_id>` (`msg_workflow_<run_id>_<n>` for the attempt after the `n`th resume). The summary SHALL contain the return value preview, agent counts, cost and duration.

Failed, budget-exceeded and stopped runs SHALL be summarized the same way. The starting Session SHALL receive exactly one summary per ended attempt, including across server restarts. A resume SHALL clear the previous attempt's result, result file and preview, so a resumed run that ends again sends a new summary with its own result.

A run started by the Session's `workflow` tool SHALL make the Session continue if idle. A run started over the API SHALL leave the summary queued without waking the Session.

When several `workflow` summaries are consecutive at the head of a Session's queue at a promotion boundary, up to 8 of them SHALL be promoted together into one Turn, in admission order.

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

#### Scenario: Two runs end together
- **WHEN** two runs started by Session S's `workflow` tool end while S is busy with a Turn
- **THEN** when that Turn ends, S runs one Turn whose input holds both summaries, in the order they were admitted

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

Each listed run SHALL carry:
- `agents: { total, completed, running, queued, failed, cancelled, interrupted }`, where `queued` counts issued calls waiting for a slot of the global limit;
- `tokens` and `cost_usd` of the run's agents;
- `elapsed_ms` of active time (time spent `queued` or `paused` excluded);
- `share`;
- `queue_position` while `queued`;
- `interrupted: true` while the run is paused by a server restart (its latest lifecycle event is a pause with `interrupted: true`), so a client that reconnects after a restart can tell that pause from a user's.

The list SHALL accept `status` (comma-separated), `session_id`, `directory` (default the request Location, `all` for every Location), `limit` (default 50, max 200) and `cursor`. It SHALL return active runs first, then the newest. `GET /api/v1/workflows/runs/{id}` SHALL return the same summary with the run's agents (`agents_detail`), each with `started_ms` from its first start and, once settled, `ended_ms`, so a monitor can show its duration.

Besides the durable `workflow.*` events, the server SHALL emit the non-durable live event `workflow.run.progress` (`{ run_id, status, phase, agents, tokens, cost_usd }`) on the instance stream of the run's Location. It SHALL be emitted only when one of these values changed, at most every 250 ms per run. It SHALL carry nothing that the list does not return, so a client that reconnects or lags resynchronizes with the list.

#### Scenario: Inspect an agent
- **WHEN** the user selects a running agent in the monitor
- **THEN** its child Session transcript opens with live updates

#### Scenario: Stop key
- **WHEN** the user presses `x` on a running run in `/workflows`
- **THEN** the run stops and its in-flight agents are cancelled

#### Scenario: Queued agents counted
- **WHEN** a run has 16 agents running and 84 calls waiting for a slot
- **THEN** its listed `agents` reads `running: 16, queued: 84`, and a `workflow.run.progress` event follows within 250 ms of each change

#### Scenario: Filter active runs
- **WHEN** a client calls `GET /api/v1/workflows/runs?status=running,queued&directory=all`
- **THEN** it receives only running and queued runs, of every Location, with their summaries

### Requirement: Workflow CLI
(P2) The CLI SHALL provide these commands:
- `cyber workflows list` (saved and bundled);
- `run <name|file> [--args JSON] [--max-cost N] [--max-turns N] [--max-tokens N] [--timeout D] [--max-agents N] [--max-concurrent N] [--runner <id>] [--wait]`;
- `runs [--all] [--status S[,S…]] [--watch]` (recent runs of the Location, or of every Location with `--all`);
- `show <run_id>`, `pause <run_id>`, `resume <run_id> [--revision]`, `resume --interrupted`, `stop <run_id>` and `logs <run_id>`.

The run commands SHALL talk to the shared server, which owns the run. `run` without `--wait` SHALL print the run ID.

`runs` SHALL show ID, name, status, phase, agents `done/running/queued/failed` (`#N in queue` for a queued run), tokens, cost and elapsed time. `runs --watch` SHALL keep the list current from live events until interrupted, redrawing in place on a terminal and printing one line (or, with `--format json`, one JSON object) per change otherwise, and SHALL exit 0 on Ctrl-C without changing any run.

`run --wait` SHALL stream progress and exit 0 on `completed`, 1 on `failed` (and on `stopped` or `paused` by another client), 4 on `BudgetExceeded`, and 130 when interrupted, stopping the run. An interrupt SHALL be honoured whether it arrives during a poll or between polls, and `queued` SHALL count as not yet ended. `--runner` SHALL be rejected as unavailable until remote runners ship (P3).

#### Scenario: CI usage
- **WHEN** CI runs `cyber workflows run audit --wait --max-cost 10`
- **THEN** the command blocks until the run ends and exits with the mapped code

#### Scenario: Runner unavailable
- **WHEN** a user runs `cyber workflows run audit --runner r1`
- **THEN** the command exits 2 saying `--runner` is not available in this build yet, and no run starts

#### Scenario: Watch several runs
- **WHEN** a user runs `cyber workflows runs --watch` while two runs execute and a third is queued
- **THEN** the table shows all three with live agent counts and the queue position, and Ctrl-C exits 0 with all three still active

### Requirement: Remote execution on runners
(P3) A run SHALL be startable on a Runner (`--runner` or `runner` option) as defined by runners-cloud, with the same semantics, durable results and monitor visibility through the Relay, and SHALL continue when the starting machine goes offline.

#### Scenario: Run continues on runner
- **WHEN** a user starts `migrate` on a cloud runner and closes their laptop
- **THEN** the run completes on the runner and the result is available from any Device

### Requirement: Workflow sharing
(P4) The system SHALL let Workflows be packaged in plugins and installed from a marketplace, verifying that a shared Workflow declares its tool allowlist, default budgets and required agents in `meta`, and showing them before the first run.

#### Scenario: Install shared workflow
- **WHEN** a user installs a plugin containing `security-sweep.ts`
- **THEN** `cyber workflows list` shows it with its declared budgets and requested tools

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

### Requirement: Remote agents
(P3) `agent()` SHALL accept `runner` (a pool name, an `rnr_` Runner ID, or a configured peer name). The child Session SHALL be created on that Runner or peer through the Orchestrator or peer API, with the repository materialized as for `cyber --cloud` and `isolation` defaulting to `worktree` there. Results SHALL be persisted as `workflow.agent.settled.1` exactly as for local agents, counted against the run's Budget, shown in the run monitor with the Runner name, and resumable after a restart of either side. A `runner` that is unavailable SHALL reject the call with `RunnerUnavailable` naming it.

#### Scenario: Fan out across machines
- **WHEN** a script launches 20 agents with `runner: "gpu-pool"` and 4 with `runner: "desk"`
- **THEN** the pool executes 20 child Sessions and the peer `desk` executes 4, and the monitor shows each agent's Runner

### Requirement: Automatic orchestration
(P2) `workflows.auto` SHALL accept `off`, `suggest` (default) and `on`. When not `off`, the system prompt SHALL instruct the model to draft a Workflow (inline script through the `workflow` tool) for tasks it judges substantive: several independent parts, more than `workflows.auto_threshold_files` (default 10) files, or a review or research task with several angles. In `suggest`, the draft, agent count and cost estimate SHALL be shown and require approval through the `workflow.run` permission; in `on` the run starts under that permission's rules. `workflows.size_guideline` (default 10 agents) SHALL be given to the model, and a draft above it SHALL carry a warning in the approval prompt. `/orchestrate <task>` SHALL force a draft for one task.

#### Scenario: Large migration suggested as a workflow
- **WHEN** `workflows.auto` is `suggest` and the user asks to migrate 300 files to a new API
- **THEN** the model proposes a `migrate` workflow with its agent count and estimated cost, and nothing runs until the user approves

### Requirement: Concurrent runs
(P2) A server SHALL let any number of Workflow Runs be active at once, started from the same or different Sessions and through the HTTP API, the CLI, the SDK or the `workflow` tool, subject only to "Run queue" and "Fair agent scheduling". A Session MAY own several active runs and SHALL remain usable while they run. Every lifecycle operation (pause, resume, stop, revision, dispatch, and the record of a script's own end) SHALL act on exactly one run. It SHALL NOT wait for, or change, another run's lifecycle operation, engine, agents or queued calls. In particular, stopping a run whose agents are still exiting SHALL NOT delay pausing, stopping, resuming or ending another run.

#### Scenario: Three runs from three surfaces
- **WHEN** `cyber workflows run audit`, `POST /api/v1/workflows/runs` with an inline script, and the model's `workflow` tool in Session S each start a run within one second
- **THEN** all three are listed by `GET /api/v1/workflows/runs` as active at the same time, each with its own agents, and the tool's run is owned by S

#### Scenario: Two runs from one Session
- **WHEN** Session S starts `review` and then `plan` through its `workflow` tool while `review` is still running
- **THEN** both runs execute, S can still be prompted, and stopping `review` leaves `plan` running

#### Scenario: Stop does not block another run
- **WHEN** run A is being stopped while its 16 agents take 5 seconds to exit, and the user pauses run B during that time
- **THEN** B is recorded `paused` without waiting for A's agents, and B's script end is recorded as soon as it happens

### Requirement: Run queue
(P2) At most `workflows.max_running_runs` (default 8, range 1–64) runs SHALL hold an execution attempt at once on a server. A run SHALL hold a run slot from dispatch until its attempt's engine and every agent task of that attempt have ended, so a paused run whose agents are still settling keeps its slot.

A run started or resumed while every run slot is taken SHALL be recorded with status `queued`, and the start or resume SHALL return at once with that status and the run's `queue_position` (1 for the next to be dispatched). Queued runs SHALL be dispatched FIFO by the time they entered the queue (ties by run ID), each with a durable `workflow.run.dispatched.1` that moves it to `running`.

A queued run SHALL start no engine, spawn no agent, and accrue no `max_wall_seconds` time. A queued run MAY be paused, which leaves the queue and moves it to `paused`, or stopped, which moves it to `stopped` and hands its summary back. A resume puts a run back at the end of the queue. A run SHALL be queued at most once, with what its latest start or resume validated: a pause, stop or resume of a run whose start is still being recorded (its `WorkflowRunStart` hooks included) SHALL wait for that start to launch or queue it.

#### Scenario: Ninth run queued
- **WHEN** 8 runs are running and a ninth is started with the default limit
- **THEN** the ninth is recorded `queued` with `queue_position: 1` and has no agents until one of the 8 ends

#### Scenario: Dispatch order
- **WHEN** runs Q1 and then Q2 are queued and one running run ends
- **THEN** Q1 is dispatched with `workflow.run.dispatched.1`, and Q2 moves to `queue_position: 1`

#### Scenario: Shutdown with a queued run
- **WHEN** every run slot is taken, run Q is queued, and the server stops while a running run's slot is freed
- **THEN** Q is still `queued` with no `workflow.run.dispatched.1`, the running runs are `paused` with `interrupted: true`, and after a restart Q is dispatched

#### Scenario: Stop a queued run
- **WHEN** the user stops a queued run
- **THEN** it becomes `stopped` without starting any agent, the other queued runs keep their order, and its owner Session receives one summary

#### Scenario: Wall budget waits in the queue
- **WHEN** a run with `max_wall_seconds: 60` waits 5 minutes in the queue and then runs for 30 seconds
- **THEN** `ctx.budget.remaining.max_wall_seconds` is about 30 and the run is not halted for wall time

### Requirement: Fair agent scheduling
(P2) The `workflows.max_concurrent` agent slots SHALL be shared between runs by a work-conserving fair scheduler. An issued `agent()` call that needs a slot, to spawn a new agent or resume an unsettled one, SHALL wait in its run's queue, ordered by issue sequence. Calls held back by a `parallel({ concurrency })` group are not issued and SHALL NOT be queued.

A run SHALL be eligible while it is `running`, its queue is not empty, and its running agents are fewer than its share. Whenever a slot is free and some run is eligible, the scheduler SHALL grant the slot to the eligible run that least recently received a grant. A run that never received one SHALL be first, and ties SHALL be broken by the time the run became eligible, then by run ID. That run's oldest queued call SHALL start. A run's last-grant time SHALL be kept while its queue is empty.

With R eligible runs, each run's oldest queued call SHALL therefore be granted within R grants. A slot SHALL never be idle while some run is eligible, and a running agent SHALL never be cancelled to rebalance slots.

A run's share SHALL be its `max_concurrent`: the invocation option (API `max_concurrent`, `cyber workflows run --max-concurrent`, the `workflow` tool's `max_concurrent`), else `meta.max_concurrent`, else `workflows.max_concurrent_per_run` (default unset). Each source SHALL be a positive integer, and the share SHALL be capped at the global limit. A share may be changed by a resume that gives `max_concurrent`; a resume that gives none, including resuming interrupted runs, SHALL keep the invocation value the run was last given, else resolve `meta` and configuration again. It SHALL NOT be part of the configuration digest, so changing it SHALL NOT require a revision.

Pausing, stopping or shutting down a run SHALL remove its queued calls from the scheduler at once, and they stay unrecorded so that a resume serves them.

#### Scenario: Small run is not starved
- **WHEN** the limit is 16, run A has 16 agents running and 84 queued, and run B then issues 2 `agent()` calls
- **THEN** B's first agent starts on the first slot A releases, A's next agent on the second, and B's second agent on the third

#### Scenario: Equal split under contention
- **WHEN** the limit is 4, run A starts 4 agents of equal duration with 10 more queued, and run B then queues 10
- **THEN** after A's first 4 agents end, A and B each have 2 agents running, and they keep 2 each while both have queued calls

#### Scenario: Idle slots are used
- **WHEN** the limit is 16 and only run A has queued calls
- **THEN** A runs 16 agents at once

#### Scenario: Per-run share
- **WHEN** the limit is 16 and run A is started with `max_concurrent: 4` and 20 queued calls, and run B queues 20
- **THEN** A never has more than 4 agents running, and B uses the remaining 12 slots

#### Scenario: Plain resume keeps the share
- **WHEN** a run started with `max_concurrent: 2` is paused and resumed with no `max_concurrent`
- **THEN** its share is still 2

#### Scenario: FIFO within a run
- **WHEN** a run issues calls c1, c2 and c3, in that order, that all wait for a slot
- **THEN** they start in the order c1, c2, c3

### Requirement: Run isolation
(P2) Each run SHALL keep its own:
- budget and spend (only Sessions whose `workflow_run_id` is the run, including their descendants), with `ctx.budget` and the `max_agents` count;
- replay records, call keys and ordinals (scoped by run ID and revision);
- log, phase list and current phase;
- worktree names (`run-<id>-<n>` on `cyber/run-<id>/<n>`);
- durable event aggregate (the run ID).

Two runs with the same script and arguments SHALL NOT reuse each other's recorded results, SHALL NOT share ordinals, and SHALL NOT collide on a worktree or branch. A budget halt, failure, pause or stop of one run SHALL NOT change another run's status or reject its calls. The owner Session's own Budget SHALL see the sum of every run it owns, as its other descendants count under `observability-costs`.

#### Scenario: Budget halt stays in its run
- **WHEN** run A, started with `max_cost_usd: 1`, reaches $1.02 while run B of the same Session has no cost limit
- **THEN** A's next `agent()` call rejects with `BudgetExceeded: max_cost_usd`, and B's calls keep starting

#### Scenario: Same script, separate replay
- **WHEN** the same inline script is started twice, the first run completes, and the second is paused and resumed
- **THEN** the second run's resume replays only its own recorded results and spawns its own agents for the rest

#### Scenario: Worktrees of two runs
- **WHEN** two runs in one Location each start 3 isolated agents
- **THEN** six distinct worktrees and branches are created, each named with its own run ID

### Requirement: Recovery of several runs
(P2) On startup, recovery SHALL process every active run in `started_ms` order. A `running` or `waiting_input` run SHALL become `paused` with `interrupted: true`, and its unsettled agents SHALL be marked `interrupted`. A `queued` run SHALL stay queued in its order and SHALL be dispatched as run slots allow; its script SHALL be read again as a resume reads it, and a run whose script or configuration changed SHALL instead become `paused` (`interrupted: true`) with a `WorkflowReplayMismatch` reason. A server that stops SHALL pause its running runs with `interrupted: true` too; once it starts doing so it SHALL launch and dispatch no run, so a queued run stays `queued` (never dispatched) for the next start's recovery, and an attempt that could no longer launch SHALL be paused with `interrupted: true` rather than failed. Then the summary of every ended run that was not handed back SHALL be admitted.

When `workflows.resume_after_restart` is `true` (default `false`), the interrupted runs (paused by this recovery or by the previous server's shutdown) SHALL then be resumed in `started_ms` order through the run queue, as if the user had resumed each one. A run whose saved module changed SHALL stay `paused` with a `WorkflowReplayMismatch` reason. `POST /api/v1/workflows/runs/resume_interrupted` (for the request Location, or `directory=all`) and `cyber workflows resume --interrupted` SHALL resume the same set on demand: runs whose latest lifecycle event is a pause with `interrupted: true`. Each resumed run SHALL continue its own unsettled agents' child Sessions.

#### Scenario: Crash with two runs mid-flight
- **WHEN** the server is killed while runs A and B each have agents in flight and run C is queued
- **THEN** after restart A and B are `paused` with their in-flight agents `interrupted`, C is dispatched, and resuming A and B completes both, each resuming only its own interrupted child Sessions

#### Scenario: Automatic resume after restart
- **WHEN** `workflows.resume_after_restart` is `true` and the server restarts with two interrupted runs, one of whose saved scripts was edited
- **THEN** the unedited run is resumed and the edited one stays `paused` with reason `WorkflowReplayMismatch`

#### Scenario: Resume every interrupted run
- **WHEN** the user runs `cyber workflows resume --interrupted` after a restart left three runs paused with `interrupted: true` and one paused by the user
- **THEN** the three interrupted runs are resumed, and the user-paused run stays paused

### Requirement: Integration queue workflow surfaces
(P2) `cyber workflows integrations submit|list|show|run|cancel|report|publish` and `/api/v1/workflows/integrations` SHALL expose the verified integration queue through existing workflow ownership, Location, permission, idempotency and budget boundaries. SDK and browser clients SHALL show manifests, progress, conflicts, host evidence, independent review, target staleness and explicit publication outcomes. Local owner-selected batches SHALL work without an account; authenticated hub-linked actions SHALL preserve current membership requirements.

#### Scenario: Owner runs a local integration batch
- **WHEN** an owner submits local commit contributions and admitted checks without a Cyber Account
- **THEN** the local queue can verify them and present evidence without claiming hub membership or publishing automatically

#### Scenario: Browser retries a cancellation
- **WHEN** a browser loses acknowledgement of cancellation
- **THEN** it retries the same revision-bound operation key without cancelling a newer attempt

### Requirement: Bundled workflow contracts
(P2) Each bundled Workflow SHALL declare its args as a JSON Schema in `meta.args`, its default budgets in `meta.budget` and an agent-count description in `meta.estimate`, and the host SHALL reject invalid args before creating a run with `ExecutionFailure` naming the args path. Effective budgets SHALL follow `workflows` → Run budgets: module/model inputs may only lower inherited ceilings, while explicit authenticated API/CLI overrides follow that shared contract, and the `workflow.run` approval SHALL show them with the estimate. Every bundled Workflow SHALL cap each fan-out, record dropped work in its result's `limitations`, return its partial result with `partial: true` when a budget stops new agents, record an agent failure for an item in `limitations` rather than omit it, pass file, diff and web content to agents only as delimited data, and return `{summary, limitations, partial, stats}` plus its own fields:

- `review`: args `target` (the `vcs-integration` review target grammar, default uncommitted changes), `lenses` (subset of `correctness`, `security`, `tests`, `maintainability`, default all), `max_findings` (default 30, at most 60) and `verify` (default true). Before the run starts, the host SHALL resolve `target` with the review target resolver under the reviewer's permissions and pass the filtered diff as `args.diff`; resolution errors SHALL be reported as `ReviewTargetError` with no run created. Read-only reviewers per lens and file group SHALL raise findings in the `vcs-integration` findings schema; findings SHALL be re-validated, deduplicated and, when `verify` is true, each checked by an independent `verify`-tier agent that may refute it. The result SHALL contain `findings` ordered by severity, file, line and summary, `refuted` and per-lens counts. Default budgets: 24 agents, 2 USD, 30 minutes.
- `audit`: args `scope` (glob, default all files), `exclude`, `focus` (`bugs`, `security` or `both`), `max_files` (default 400, at most 2000), `chunk_files` (default 12) and `votes` (default 3). Read-only finders per chunk SHALL raise findings in the findings schema; critical and high findings SHALL survive only if a majority of `votes` independent refuters fail to refute them; medium and low findings SHALL get one verifier. The result SHALL contain `findings`, `refuted` and `coverage` with the files scanned and every skipped file with its reason. Default budgets: 120 agents, 10 USD, 2 hours.
- `migrate`: args `instructions` (required), `files` or `pattern` (one required) with optional `contains`, `batch_size` (default 5, at most 25), `check` (commands), `confirm_over` (default 20) and `yes`. When more than `confirm_over` files are selected and `yes` is not true, the run SHALL `ask()` for confirmation before any agent starts. Each batch SHALL run in its own worktree with an edit-capable agent that runs the `check` commands there and commits on its branch, followed by a verifier of the branch's diff and check output. The result SHALL list every batch with files, branch, status, check exit codes and verification, and SHALL NOT merge any branch. Before starting agents, the run SHALL check that the owner's rules allow the edits and commands it needs in a non-interactive run and fail with the missing rules otherwise. Default budgets: 80 agents, 15 USD, 4 hours.
- `research`: args `question` (required), `sources` (subset of `web`, `repo`, `docs`), `angles` (default 3, at most 6) and `max_sources` (default 12). Searchers SHALL run per source and angle, readers SHALL extract claims with locations, and each claim used in the answer SHALL be cross-checked by an independent agent; a claim supported by one source SHALL be marked `verified: false`. When `web` is requested but the web tools are not permitted, the result SHALL say so in `limitations` and use the other sources. The result SHALL contain `answer`, `claims` with sources and `verified`, and `sources`. Default budgets: 40 agents, 5 USD, 1 hour.
- `plan`: args `task` (required), `angles` (2–6, default `mvp-first`, `risk-first`, `user-first`), `judges` (default 3) and `context` (paths). Read-only planners SHALL draft one plan per angle, judges SHALL score every draft on correctness, completeness, risk, effort and testability, and a final agent SHALL merge the highest-scoring draft with named grafts from the others. The result SHALL contain `plan_markdown`, `chosen`, `scores` and `grafts`, and no file SHALL be edited. Default budgets: 14 agents, 3 USD, 30 minutes.

`/review` and `cyber review` SHALL run the `review` Workflow of the Location's library when no discovered `review` skill or command overrides the bundled one, and SHALL report its findings as the `vcs-integration` "Local review command" requires; `cyber review` SHALL exit 0 when the run completes, 4 when a budget stopped it (printing the partial findings) and 1 when it fails. A `review` run in which every reviewer failed SHALL fail rather than report no findings. When the bundled Workflows are disabled and no saved `review` Workflow exists, `/review` and `cyber review` SHALL run the bundled review skill as before.

#### Scenario: Review verifies findings
- **WHEN** `/review` runs on a diff where the correctness lens raises two findings and the verifier refutes one
- **THEN** the TUI shows one finding, and the run result has `refuted: 1`

#### Scenario: Review without the bundled Workflows
- **WHEN** `CYBER_DISABLE_BUNDLED_WORKFLOWS=1` is set and the user runs `cyber review`
- **THEN** the bundled review skill's reviewer Job reviews the target and no Workflow Run is created

#### Scenario: Invalid review target creates no run
- **WHEN** the user runs `cyber workflows run review --args '{"target":"main.."}'`
- **THEN** the command fails with `ReviewTargetError` code `invalid_target` and `GET /api/v1/workflows/runs` lists no new run

#### Scenario: Audit reports skipped files
- **WHEN** `audit` runs with `max_files: 100` over a scope of 140 files
- **THEN** the result's `coverage` lists 100 files scanned and 40 skipped with reason `max_files`, and `limitations` repeats the cap

#### Scenario: Majority refutes an audit finding
- **WHEN** an audit finder raises a high finding and two of three refuters refute it
- **THEN** the finding is absent from `findings` and counted in `refuted`

#### Scenario: Migrate asks before a large run
- **WHEN** `migrate` selects 60 files with the default `confirm_over` in an interactive Session
- **THEN** the run waits in `waiting_input` with the question naming 60 files and 12 batches, and no agent has started

#### Scenario: Migrate batches stay on branches
- **WHEN** `migrate` completes 3 batches with `check: ["cargo check"]`
- **THEN** the result lists 3 branches with the check exit codes, and the owner's checkout is unchanged

#### Scenario: Migrate in CI without allow rules
- **WHEN** `cyber workflows run migrate --wait` runs non-interactively and no rule allows `edit`
- **THEN** the run fails before starting any agent, naming the missing `edit` rule

#### Scenario: Research without web access
- **WHEN** `research` is started with `sources: ["web", "repo"]` and the web tools are denied
- **THEN** the answer is built from repository sources and `limitations` says web sources were not permitted

#### Scenario: Plan merges the winner
- **WHEN** `plan` drafts three plans and the risk-first draft scores highest
- **THEN** the result's `chosen` is `risk-first` and every graft names the draft it came from

#### Scenario: Budget stops a fan-out
- **WHEN** `audit` runs with `--max-cost 1` and the budget is reached after 6 of 20 chunks
- **THEN** the run ends `budget_exceeded`, its result has `partial: true` with the findings of the finished chunks, and `cyber workflows run --wait` exits 4

### Requirement: Verified workflow library entry
(P2) The library SHALL expose the bundled `solve` module with the arguments, soft budgets, isolated candidate protocol, host verification and report defined by `verified-workflows`. Library consumers SHALL distinguish check-passing evidence from a guarantee of correctness.

#### Scenario: Inspect solve before running
- **WHEN** a user inspects the shipped `solve` entry through the CLI or library API
- **THEN** its schema shows the required checks and candidate/judge models and its budget metadata states the soft-limit semantics

### Requirement: Workflow library over HTTP
(P2) `GET /api/v1/workflows` SHALL return the Workflow library of a Location (`directory` query): each saved and bundled entry with `name`, `description`, `phases`, `args`, `budgets`, `estimate`, `origin`, `digest` and `shadowed`, and the modules that failed to load with their diagnostics, applying the same precedence and the same `workflows.bundled` setting as the CLI. The `workflow` tool description SHALL name the library entries visible in the Location (at most 20, bundled first) and SHALL tell the model to prefer a library Workflow over an inline script when one fits.

#### Scenario: Library over HTTP
- **WHEN** a client calls `GET /api/v1/workflows?directory=/repo` in a project with `.cyber/workflows/release.js`
- **THEN** the response lists `release` with origin `project` and the five bundled Workflows with origin `bundled`

#### Scenario: Model starts a bundled workflow by name
- **WHEN** the user asks the model to audit the auth module for security bugs
- **THEN** the model calls the `workflow` tool with `name: "audit"` and args `{scope: "src/auth/**", focus: "security"}`, and the approval shows origin `bundled:audit` with its budgets and estimate
