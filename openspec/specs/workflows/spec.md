# workflows Specification

## Purpose
A Workflow is a JavaScript/TypeScript script that orchestrates many subagents: fan-out, pipelines, cross-verification and consolidation, executed in the background by a deterministic, resumable runtime. It brings Claude Code's dynamic workflows to any LLM and adds per-call model mixing, budgets and remote execution. The runtime builds on OpenCode's codemode ideas (a confined JS subset with structured diagnostics) and on Cyber Code's durable event store for resume. Codex has no equivalent; it orchestrates subagents turn by turn instead.

## Requirements

### Requirement: Workflow module format
(P2) The system SHALL accept a Workflow as an ES module written in JavaScript or TypeScript (TypeScript transpiled with swc, type errors not enforced). The module SHALL export `meta` as a pure literal object `{ name, description, phases?, args? }` (no variables, calls or interpolation) and a default async function, or top-level code, that forms the script body. A non-literal `meta` SHALL fail with `ParseError: meta must be a pure literal`.

#### Scenario: Valid module
- **WHEN** a module exports `meta = { name: "audit", description: "Audit auth", phases: [{ title: "Scan" }] }` and a default async function
- **THEN** the Workflow loads and its name, description and phases are shown before the run starts

#### Scenario: Computed meta rejected
- **WHEN** `meta.name` is built with a template literal containing a variable
- **THEN** loading fails with `ParseError: meta must be a pure literal` and no run is created

### Requirement: Sandboxed QuickJS runtime
(P2) The system SHALL execute Workflow scripts in an embedded QuickJS context per run, with no filesystem, network, process, module import (other than the host API), `eval`, `Function` constructor, timers or `WebAssembly`. Script memory SHALL be limited by `workflows.max_memory_mb` (default 256) and per-run script CPU time (excluding time awaiting host calls) by `workflows.script_cpu_seconds` (default 60).

#### Scenario: Filesystem access unavailable
- **WHEN** a script calls `require("fs")` or `import("node:fs")`
- **THEN** the run fails with `UnsupportedSyntax` naming the forbidden import

#### Scenario: Busy loop stopped
- **WHEN** a script runs `while(true){}`
- **THEN** after 60 seconds of script CPU the run fails with `Timeout` and in-flight agents are cancelled

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
(P2) `phase(title)` SHALL set the current phase for subsequent `agent()` calls and the run monitor; titles not declared in `meta.phases` SHALL be appended. `log(...values)` SHALL append JSON-safe values to the run log (max 1 MiB, then truncated with a marker). `checkpoint(name, value)` SHALL durably record a JSON-safe value that is returned unchanged by the same call on resume.

#### Scenario: Monitor shows phase
- **WHEN** a script calls `phase("Verify")` before spawning agents
- **THEN** those agents appear under `Verify` in the run monitor

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
(P2) A Workflow Run (`run_` ID) SHALL have status `queued`, `running`, `waiting_input`, `paused`, `completed`, `failed`, `budget_exceeded` or `stopped`. Pause SHALL stop scheduling new agents while letting in-flight agents finish; stop SHALL cancel in-flight agents and mark them `cancelled`; resume SHALL continue a `paused`, `failed` or `stopped` run through the resume mechanism. Transitions SHALL be durable events (`workflow.run.started.1`, `workflow.run.paused.1`, `workflow.run.resumed.1`, `workflow.run.completed.1`, `workflow.run.failed.1`, `workflow.run.stopped.1`).

#### Scenario: Pause lets agents finish
- **WHEN** the user pauses a run with 4 agents in flight
- **THEN** those 4 complete and are stored, and no new agent starts until resume

#### Scenario: Resume refused while agents alive
- **WHEN** the user relaunches a stopped run whose cancelled agents have not exited yet
- **THEN** the relaunch is refused with `Run has agents still exiting; retry shortly`

### Requirement: Background execution on the server
(P2) Runs SHALL execute inside the `cyber` server, independent of any client connection, and the starting Session SHALL remain usable while the run proceeds. Closing the TUI SHALL NOT stop a run; stopping the server SHALL mark running runs `paused` for later resume.

#### Scenario: Client disconnect
- **WHEN** the user closes the terminal while a run is executing
- **THEN** the run continues and is visible on reconnect in `/workflows`

### Requirement: Concurrency limit
(P2) The system SHALL run at most `workflows.max_concurrent` (default 16, reduced to the number of CPU cores when lower) workflow agents at once across all runs of a server, queueing further `agent()` calls FIFO. `parallel({ concurrency })` SHALL further limit a group but SHALL NOT exceed the global limit.

#### Scenario: Large fan-out queued
- **WHEN** a script launches 100 agents in parallel with the default limit on a 32-core machine
- **THEN** 16 run at a time and the rest wait

### Requirement: Run budgets
(P2) A run SHALL accept budgets `max_agents` (default 200), `max_tokens`, `max_cost_usd` and `max_wall_minutes` (default 240) from `meta`, invocation options or `workflows.default_budget`. When a budget would be exceeded, new `agent()` calls SHALL reject with `BudgetExceeded` naming the budget, in-flight agents SHALL finish, and `ctx.budget` SHALL expose `{ spent_tokens, spent_cost_usd, agents_started, remaining }` to the script at any time. Cost and token dispatch SHALL obey the soft/reserved enforcement and uncertain usage rules in observability-costs, including checks before each child provider Turn. A halted budgeted run SHALL enter `budget_exceeded`.

#### Scenario: Cost budget reached
- **WHEN** a run with `max_cost_usd: 5` has spent $5.02
- **THEN** the next `agent()` call rejects with `BudgetExceeded: max_cost_usd` and the script may return a partial result

### Requirement: Model tiers and mixing
(P2) The `model` option of `agent()` SHALL accept a model ref or a tier name defined in `workflows.tiers` (defaults: `fast` → `model_roles.small`, `smart` → `model_roles.default`, `verify` → `model_roles.advisor` or the default model). Different calls in one run MAY use different providers. An unknown tier SHALL reject the call with `ModelUnavailable`.

#### Scenario: Cheap fan-out, strong verification
- **WHEN** a script runs reviewers with `model: "fast"` and verifiers with `model: "verify"`
- **THEN** reviewer sessions use the small model and verifier sessions use the advisor model, and run cost is reported per model

### Requirement: Prompt-cache warm-up for fan-out
(P2) When a `parallel` group launches two or more agents whose requests share an identical cacheable prefix, the runtime SHALL start the first agent alone and release the others when its first provider response begins, so that the others read the shared cached prefix.

#### Scenario: Shared prefix
- **WHEN** 10 agents with the same agent, model and preamble are launched together
- **THEN** one starts immediately and nine start after its first response chunk arrives

### Requirement: Write isolation default
(P2) When more than one concurrently running workflow agent has edit permission in the same Location, the runtime SHALL default those agents to `isolation: "worktree"` unless the call sets `isolation: "none"` explicitly, and SHALL report each agent's branch in its result.

#### Scenario: Concurrent writers isolated
- **WHEN** a script launches 5 `general` agents in parallel without `isolation`
- **THEN** each runs in its own worktree and its result includes `branch`

### Requirement: Permissions for runs
(P2) Starting a run SHALL require the `workflow.run` permission with the workflow name as resource (`ask` by default in `default` Mode, `allow` in `auto` and `bypass`). Agents of the run SHALL run under the run's Mode as a ceiling. A run started non-interactively SHALL only use permissions pre-approved by rules, because any `ask` from an agent SHALL be auto-rejected with feedback `No interactive approver for workflow run`.

#### Scenario: Approval before start
- **WHEN** the model calls the `workflow` tool in `default` Mode
- **THEN** the user sees the workflow name, phases and budgets and approves or rejects before any agent starts

### Requirement: Saved and bundled workflows
(P2) The system SHALL discover saved Workflows from `.cyber/workflows/**/*.{ts,js}` (project), `~/.config/cyber/workflows/` (global) and plugins, and SHALL bundle `review` (multi-angle code review with verification), `audit` (codebase-wide bug or security sweep), `migrate` (file-by-file migration with build and test checks), `research` (cross-checked research with sources) and `plan` (plans drafted from several independent angles, then merged). Project workflows SHALL override global and bundled ones with the same name.

#### Scenario: Run a bundled workflow
- **WHEN** the user runs `cyber workflows run review --args '{"base":"main"}'`
- **THEN** the bundled review workflow starts against the diff from `main`

### Requirement: Model-authored workflows
(P2) The model SHALL be able to start a Workflow through a `workflow` tool that takes either `name` + `args` or inline `script` + `args`. Inline scripts SHALL be persisted to `<state>/workflows/<run_id>.ts` and listed with the run, so that the user can read, edit and relaunch them, or save them into `.cyber/workflows/`.

#### Scenario: Save an inline workflow
- **WHEN** a run started from an inline script completes and the user chooses Save
- **THEN** the script is written to `.cyber/workflows/<meta.name>.ts`

### Requirement: Run results
(P2) A completed run SHALL produce its script return value (JSON-safe, max 1 MiB, otherwise stored as a file with a preview), per-agent child Sessions with transcripts, and a summary message admitted to the starting Session with `delivery: queue` containing the return value preview, agent counts, cost and duration.

#### Scenario: Result handback
- **WHEN** a run started from a Session completes
- **THEN** the Session receives a queued message with the run summary and continues if idle

### Requirement: Diagnostics
(P2) Failures SHALL be reported with exactly one diagnostic kind from `ParseError`, `UnsupportedSyntax`, `ExecutionFailure`, `AgentFailed`, `SchemaMismatch`, `BudgetExceeded`, `ModelUnavailable`, `NoInteractiveUser`, `Timeout` and `PermissionDenied`, including a message and, for script errors, the source line and column.

#### Scenario: Syntax error location
- **WHEN** a script has a missing closing brace on line 12
- **THEN** the run fails with `ParseError` and location `12:1`

### Requirement: Run monitor
(P2) The `/workflows` view and `GET /api/v1/workflows/runs` SHALL list runs with name, status, current phase, agents `done/running/queued/failed`, tokens, cost and elapsed time, and SHALL let the user open any agent's transcript, pause (`p`), resume, stop (`s`) and open the script. Live updates SHALL stream over the event stream.

#### Scenario: Inspect an agent
- **WHEN** the user selects a running agent in the monitor
- **THEN** its child Session transcript opens with live updates

### Requirement: Workflow CLI
(P2) The CLI SHALL provide `cyber workflows list` (saved and bundled), `run <name|file> [--args JSON] [--budget-cost N] [--runner <id>] [--wait]`, `runs` (recent runs), `show <run_id>`, `pause <run_id>`, `resume <run_id>` and `stop <run_id>`. `run --wait` SHALL stream progress and exit 0 on `completed`, 1 on `failed`, 4 on `BudgetExceeded`, and 130 when interrupted.

#### Scenario: CI usage
- **WHEN** CI runs `cyber workflows run audit --wait --budget-cost 10`
- **THEN** the command blocks until the run ends and exits with the mapped code

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
(P2) Resumable runs SHALL persist results of all observable host calls, including ask, read-only tools, phase checkpoints and agent results, keyed by stable call identity and invocation sequence. Replay SHALL return recorded results instead of rereading changed files or asking the user again. A run SHALL record script and configuration digests. An edited script SHALL create an explicit revision with reuse limited to compatible recorded calls; divergent host-call sequences SHALL fail with `WorkflowReplayMismatch` rather than silently reuse results. Unsettled agents SHALL resume their existing child Session and tool recovery records before any replacement is spawned.

#### Scenario: Files change while run is stopped
- **WHEN** a workflow resumes after its earlier glob result would now differ
- **THEN** replay uses the recorded glob result for that revision and resumes existing child Sessions
