## Why

The user asked for parallel workflow support: several Workflow Runs active at once, started from the same or different Sessions, from the CLI, the HTTP API, the SDK or the model's `workflow` tool, all visible and controllable from one place, as in Claude Code, where several workflows run in the background and `/workflows` lists them all.

M2.1 (`fc61f9f`) already runs every run in the background on its own engine thread, with a per-run `Control`, per-run replay, budgets and worktrees. Acceptance, however, tested one run at a time, and a close reading of the runtime finds these gaps once N runs share a server:

- **Starvation.** One server-wide FIFO `tokio::sync::Semaphore` (`runtime/workflows.rs:519-537`, acquired in `workflow_host.rs:507-528`) serves every `agent()` call. A 100-agent fan-out queues 84 calls, and a 2-agent run started a second later waits behind all of them. The test `the_global_limit_is_fifo_across_runs` (`crates/cyber-server/tests/workflow_runs.rs:532`) asserts exactly this behavior.
- **No run limit.** `WorkflowRunStatus::Queued` exists (`workflows.rs:79`), but no code ever sets it. `start_workflow_run` records `running` (`workflows.rs:648`) and spawns one OS thread per run, each with a QuickJS heap of up to 256 MiB (`workflows.rs:781-797`), with no upper bound.
- **Cross-run lock coupling.** One `workflow_lifecycle` mutex serializes pause, stop, resume and recovery for all runs (`workflow_control.rs:192, 214, 262, 340, 357`). `stop_workflow_run` holds it while it awaits `stop_subtree` for every child of the run (`workflow_control.rs:213-243`), and every engine's `finish` takes it with `blocking_lock` (`workflows.rs:851-854`). Stopping one large run delays the pause, stop and end of every other run.
- **Write isolation per run only.** The "another edit-capable agent is in flight" counter is a field of each `RunHost` (`workflow_host.rs:92-93, 220, 376-384`). Two runs in one Location, each with one writer, both edit the owner's checkout in place at the same time.
- **Queued agents are invisible.** A call waits for a permit before anything is recorded (`workflow_host.rs:465-478`), so neither the API nor a monitor can report `queued` agents, even though "Run monitor" already requires `done/running/queued/failed`.
- **Thin listing.** `GET /workflows/runs` returns raw `WorkflowRun` rows, with no agent counts, tokens, cost or elapsed time, no status filter and no pagination (`http/workflows.rs:169-180`). `cyber workflows runs` prints only ID, name, status and phase (`commands/workflows.rs:348-373`), and has no `--watch`. The SDK can start one run and follow it, but cannot list runs or attach to one (`sdk/typescript/src/client.ts:79-85`).
- **Recovery.** `recover_workflow_runs` pauses every active run (`workflow_control.rs:339-353`). That treats a never-started `queued` run like a running one. There is also no way to resume every run a crash interrupted.
- **Handback.** Each run's summary is a separate queued message (`workflow_results.rs:147-167`), and `promote` takes one queued message per boundary (`drain.rs:416-421`). Two runs of one Session that end together therefore cost two consecutive model Turns.
- **No TUI surface.** `crates/cyber-tui` has no workflow code: no `/workflows` monitor (M2.1 task 5.6 deferred), no running-run indicator, and no end notification.

What already works for N runs is listed in design.md ("What already holds"). This change keeps those parts and closes the gaps.

## What Changes

- **Concurrent runs.** Explicit multi-run guarantees: any number of runs per Session and per server, from any surface. Each lifecycle operation acts on one run and never waits on another run's lifecycle, engine end or agents. Per-run lifecycle locks replace the global one.
- **Run queue.** `workflows.max_running_runs` (default 8, range 1–64) bounds the runs that hold an execution attempt at once. Further starts and resumes are recorded `queued` and dispatched FIFO (`workflow.run.dispatched.1`). A queued run starts no engine, spawns no agent and accrues no wall-clock budget. It can be paused or stopped, and it keeps its place across a restart.
- **Fair agent scheduling.** The global FIFO semaphore is replaced by a work-conserving scheduler. Each run has a FIFO queue of calls. A free slot goes to the eligible run that least recently received one. An optional per-run share (`max_concurrent` from the invocation, `meta` or `workflows.max_concurrent_per_run`) caps a run below the global limit. Running agents are never preempted.
- **Run isolation.** Budgets, `ctx.budget`, `max_agents`, replay records and ordinals, logs, phases, worktree names and event aggregates are stated per run. The write-isolation default counts edit-capable agents of every run in a Location.
- **Recovery of several runs.** Every interrupted run is paused with its own interrupted agents. Queued runs keep their order and are dispatched. A new `workflows.resume_after_restart` setting (default `false`) and `cyber workflows resume --interrupted` resume them all.
- **Coalesced handback.** Run summaries pending together in one Session's queue are promoted as one Turn.
- **Progress summary.** `GET /workflows/runs` gains per-run agent counts (including `queued`), tokens, cost, active elapsed time, `queue_position`, filters and cursor pagination. A coalesced live event `workflow.run.progress` reports changes that leave no durable event (agents queued or dequeued).
- **CLI.** `cyber workflows runs` shows counts, tokens, cost and elapsed time, with `--all`, `--status` and `--watch`. New flags: `run --max-concurrent N` and `resume --interrupted`.
- **SDK.** `client.workflows.list()` and `client.workflows.get(id)` return run handles.
- **TUI.** The `/workflows` monitor lists every run of the Location with name, status, phase, agent counts, tokens, cost, elapsed time and a per-run nyan bar. Enter drills into a run's agent tree by phase and opens a child transcript. `p`/`r`/`x` pause, resume and stop. It updates live. Also added: a sidebar Workflows section, a `workflows` status-line field (`⚙ 2 workflows`), aggregate known progress for the nyan progress line, and notifications when a run ends.

## Capabilities

### Modified Capabilities
- `workflows`: ADDED "Concurrent runs", "Run queue", "Fair agent scheduling", "Run isolation" and "Recovery of several runs" (P2). MODIFIED "Concurrency limit" (fair scheduling instead of FIFO), "Run lifecycle states" (`queued`, dispatch), "Write isolation default" (counted across runs), "Run results" (coalesced handback), "Run monitor" (summary fields, filters, live progress) and "Workflow CLI" (`runs` columns and `--watch`, `--max-concurrent`, `resume --interrupted`).
- `tui`: MODIFIED "Workflow monitor". ADDED "Workflow run indicators" and "Workflow run notifications" (P2).
- `client-sdk`: MODIFIED "Workflow helpers" (`list`, `get`).

`server-api` and `cli-commands` need no change: the routes stay in the `workflow` group, and `cyber workflows runs` is already registered. `configuration` needs no change either: the new keys live in the `workflows` section, and the workflows spec defines them.

## Impact

- **Code.**
  - `crates/cyber-server/src/runtime`: a new `workflow_scheduler.rs` replaces the semaphore, and a new `workflow_queue.rs` handles run dispatch. Per-run locks go in `workflow_control.rs`, the Location writer registry in `workflow_host.rs`, recovery in `workflow_control.rs`, and the handback batch in `drain.rs`.
  - `http/workflows.rs` gains the summary, filters and progress event.
  - `crates/cyber-core/src/config/workflows.rs` gains three keys.
  - `crates/cyber-cli/src/commands/workflows.rs`.
  - `sdk/openapi.json` and `sdk/typescript/src/workflows.ts`.
  - In `crates/cyber-tui`: a new `workflows.rs` and `view/workflows.rs`, plus the sidebar section, footer field and progress feed.
- **Data model.** One new durable event, `workflow.run.dispatched.1`. A `queued` run's budget row is created with `activated_ms = NULL` and activated at dispatch. No table migration: `workflow_run.status` already holds any status, and agent counts are aggregated from `workflow_agent`. One new non-durable live event, `workflow.run.progress`.
- **Behavior change.** Calls are no longer started FIFO across runs. `the_global_limit_is_fifo_across_runs` is rewritten into a fairness test. `Runtime::workflow_slots()` (used by `crates/cyber-app/tests/windows_service.rs:226`) is replaced by a scheduler handle.
- **Sequencing.**
  - Archive after `implement-m2-1-workflow-runtime`. Its deltas MODIFY the same `workflows` requirements, and the MODIFIED text here restates the M2.1 wording plus this change's additions.
  - The TUI indicator tasks build on `improve-tui-experience` increments 4 (sidebar) and 8.8 (nyan progress indicator) and do not edit that change. Its "Status line" field list gains `workflows` when both are archived.
- **Canonical inventory after archive.** +7 requirements (`workflows` +5, `tui` +2), all P2.
