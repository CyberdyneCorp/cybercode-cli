## Context

M2.1 runs each Workflow Run in the `cyber` server, on its own OS thread with its own QuickJS context. A process-local `Control` per attempt is kept in `Inner::workflow_controls`, keyed by run ID. One FIFO semaphore caps all workflow agents. Durable state is the `run_` aggregate (`workflow.run.*`, `workflow.agent.*`, `workflow.call.recorded.1`, `workflow.phase.1`, `workflow.log.1`), projected into `workflow_run`, `workflow_agent`, `workflow_call`, `workflow_run_revision` and `workflow_run_budget`.

This change turns "several runs happen to work" into a specified, fair and observable multi-run runtime with a TUI monitor.

### What already holds for N runs

Kept as is, and covered by the new multi-run tests:

| Concern | Why it already works | Where |
|---|---|---|
| Engine isolation | One thread and one QuickJS context per attempt | `workflows.rs:733-797` |
| Control map | Keyed by run ID; a second attempt of the same run is refused while the first is busy | `workflow_control.rs:131-150` |
| Several runs per Session | `start_workflow_run` refuses only owners that belong to a run (ancestry), not owners with an active run | `workflows.rs:632-635` |
| Replay identity | `Replay::load(store, run_id, revision)`; call keys and ordinals live in each `RunHost`; operation key `<run_id>:<call_key>` | `workflow_control.rs:314`, `workflow_host.rs:404-422`, `cyber-tools/src/host/workflow_agents.rs:73` |
| Agent identity | `agt_` IDs are unique and resolved by subagent name under the owner | `workflow_host.rs:471-473` |
| Budgets | `workflow_run_budget` per run; spend sums Sessions whose `workflow_run_id` is the run | `budget.rs:99-123` |
| Worktree names | `run-<ulid>-<n>` on `cyber/run-<ulid>/<n>` include the run ID | `host.rs:410-426` |
| Handback IDs | `msg_workflow_<run_id>[_<n>]`, so summaries of different runs never collide | `workflow_results.rs:28-39` |
| Event routing | Each run is its own aggregate; the instance stream locates it by `workflow.run.started.1`'s directory | `http/event_location.rs:14,37` |
| Pause while queued for a slot | The call leaves the semaphore queue at once | `workflow_host.rs:507-528`, test `a_run_paused_while_queued_for_a_slot_leaves_the_queue_at_once` |
| Shutdown | Pauses every busy control and waits for all of them | `workflow_control.rs:356-417` |

### Gaps

These are listed with file and line references in proposal.md:

1. The FIFO semaphore lets one run starve another.
2. No run limit, and the `queued` status is unused.
3. The global lifecycle lock couples runs.
4. Writer counting is per run.
5. Queued agents cannot be observed.
6. The listing is thin, with no CLI watch and no SDK list.
7. Recovery treats `queued` as running and has no batch resume.
8. One Turn per handback.
9. No TUI.

## Goals / Non-goals

Goals:
- Several runs from any surface, each controllable independently.
- A bounded wait for every run that has queued agents.
- An exact `queued` count.
- Restart with several runs mid-flight.
- One `/workflows` view for all of them.

Non-goals:
- Priorities or weights between runs, beyond the per-run share.
- Preempting running agents.
- Runs spread over several servers (P3 runners).
- Changing replay semantics.
- Per-agent budgets.
- `ask()` (still M2.1 task 4.8).

## Decisions

### Fair scheduler instead of the semaphore

`WorkflowScheduler` (`runtime/workflow_scheduler.rs`) owns `capacity = effective workflows.max_concurrent` and a `BTreeMap<run_id, RunQueue>`, all behind one `std::sync::Mutex`. Each `RunQueue` holds:
- `waiting: VecDeque<Waiter>`, FIFO by issue sequence;
- `running: usize` and `share: usize`;
- `last_grant: Option<u64>`, a scheduler tick, not wall time;
- `eligible_since: u64`.

A `Waiter` is a `oneshot::Sender<Slot>`. A `Slot` is an RAII permit: on drop it decrements its run's `running` and calls `dispatch()`.

`dispatch()` loops while `total_running < capacity`:
1. Pick the eligible run with the smallest `(last_grant or −∞, eligible_since, run_id)`. A run is eligible when it has waiters, is not suspended (paused, stopped or closing) and `running < share`.
2. Pop that run's oldest waiter.
3. Increment `running` and `total_running`, set `last_grant = ++tick`, and send the slot.
4. If the receiver is gone (the call was dropped), undo step 3 and continue.

`last_grant` persists while a run's queue is empty. A run that briefly drains and refills therefore does not jump the line. A run seen for the first time sorts first, which is what lets a 2-agent run start on the next free slot.

This is a least-recently-served round robin. With R eligible runs, a run's oldest waiter is granted within R grants, whatever the other runs' queue lengths. When agent durations are equal, slots converge to an equal split, ±1. The scheduler is work-conserving: a slot is never idle while some run is eligible.

`parallel({ concurrency })` stays in the engine. Calls held back by a group are not issued, so they never reach the scheduler and are not counted as `queued`.

Why not weighted fair queuing or deficit round robin: agent cost is unknown at start, so slot-count fairness is the only fairness we can honestly promise. The scheduler state is also small enough to unit-test exhaustively with a fake clock.

`RunHost::slot()` keeps its current shape: `admit()`, then the scheduler wait raced against the run's pause, stop or cancel signal, then `admit()` again. Pausing calls `scheduler.suspend(run_id)`, which drops that run's waiters at once; it already leaves the semaphore queue today. Resume calls `unsuspend`. A run's `RunQueue` is removed when its attempt ends.

The scheduler also exposes `snapshot(run_id) -> {queued, running}` for the monitor. `queued` is the length of `waiting`: issued calls that wait for a slot. Every change to `waiting` or `running` marks the run dirty for the progress event.

### Run queue

`WorkflowQueue` (`runtime/workflow_queue.rs`) holds `max_running_runs` run slots. A run holds one from dispatch until its `Control` stops being busy: the engine thread and every agent task have returned. A paused run whose agents are still settling therefore still occupies its slot.

- **Start.** `start_workflow_run` records `workflow.run.started.1` with status `queued` when every slot is taken. Its budget row starts with `activated_ms = NULL`.
- **Resume.** `resume_workflow_run` does the same with `workflow.run.resumed.1` and status `queued`.
- **Dispatch.** When a slot is free, the oldest queued run (by the time it entered the queue, then ID) gets `workflow.run.dispatched.1` (status `running`, activates the budget clock) and its engine launches, as `launch` does today.
- **Order.** The queue order is durable: the time of the last `started` or `resumed` event that queued the run, so a restart rebuilds it from `workflow_run`.
- **Pause and stop.** A queued run can be paused (`paused`, leaves the queue) or stopped (`stopped`, summary handed back). Dispatch re-reads the run under its lock, so a pause racing dispatch is serialized.

Default 8: each attempt is an OS thread plus up to `max_memory_mb` (256) of QuickJS heap, so 8 runs cap script memory at 2 GiB by default. The range is 1–64.

### Per-run lifecycle locks

`workflow_lifecycle: Mutex<()>` becomes `workflow_locks: StdMutex<HashMap<run_id, Arc<tokio::sync::Mutex<()>>>>`. Pause, stop, resume, dispatch and `finish` take only their run's lock.

- **Recovery and shutdown** iterate the runs and take each run's lock in turn. They never hold two at once, so there is no lock-ordering cycle.
- **Stop** still awaits its children's `stop_subtree` under its own run's lock. That lock is what serializes the stop with that run's script end.
- **The queue** has its own short `std::sync::Mutex`. It is never held across an await or while a run lock is taken.

### Write isolation across runs

`RunHost::writers` becomes a handle into `Inner::workflow_writers: StdMutex<HashMap<canonical Location, Arc<AtomicUsize>>>`. `WriterSlot` already increments it on issue and decrements it on drop. Sharing it per Location makes "more than one concurrently running edit-capable workflow agent in the same Location" true across runs.

The existing semantics are kept: issued calls of one fan-out count before the first starts, the count applies only in a git checkout, and `isolation: "none"` opts out. Worktree names already include the run ID, so two runs never collide on a name or branch.

### Recovery of several runs

`recover_workflow_runs`, for each run in `started_ms` order:
- **Running or waiting for input.** The run is durably `paused` with `interrupted: true`; its running agents are projected `interrupted`, as today.
- **Queued.** The run stays queued and is dispatched as slots allow. Its engine is rebuilt through `WorkflowRelaunch`, which the HTTP layer provides (the saved module or script file read again, the Location's limits and tiers, the owner's `workflow.run` rules); a changed script or configuration pauses it with `WorkflowReplayMismatch` instead. Queued runs are put back into the queue without dispatching; dispatch starts once, after the loop, so they leave it in `queued_ms` order (a resumed run's `queued_ms` is its resume time), not in the `started_ms` order they were visited in.

Next, every unnotified ended run is handed back, as today. Then, when `workflows.resume_after_restart` is true, the interrupted runs (this recovery's, and those the previous server paused while shutting down) are resumed in `started_ms` order through `resume_workflow_run`. That goes through the run queue, the saved module re-read and the replay check. A run whose saved script changed stays `paused`, with reason `WorkflowReplayMismatch: …`.

`POST /api/v1/workflows/runs/resume_interrupted` (`?directory=` defaults to the request Location) resumes the same set on demand. `cyber workflows resume --interrupted` and the monitor's `R` call it. A run is "interrupted" when its latest lifecycle event is `workflow.run.paused.1` with `interrupted: true`.

Each run resumes its own unsettled agents' child Sessions. Child ownership release after a crash is already per child (M2.1 "Run lifecycle states").

### Coalesced handback

In `drain.rs` `promote`, when the head of the queue is a `workflow`-source message, every consecutive `workflow`-source queued message behind it is promoted in the same commit, in admission order, up to 8. A summary is at most about 5 KiB, so the batch stays far below any context concern.

The owner sees one Turn that reads, for example, "Workflow run handback: {audit…}" and "Workflow run handback: {review…}". The idempotent IDs and the exactly-once rule per attempt are unchanged.

### Progress summary and live updates

`WorkflowRunSummary` = `WorkflowRun` plus:
- `agents: {total, completed, running, queued, failed, cancelled, interrupted}`, where `queued` comes from the scheduler (0 when the run is not executing) and the rest from one `GROUP BY status` over `workflow_agent`;
- `tokens` and `cost_usd`, from `run_spend`;
- `elapsed_ms`, the active time (`workflow_run_budget.active_ms` plus the open interval);
- `queue_position`, set only while queued;
- `share`.

`GET /workflows/runs` takes `status` (comma list), `session_id`, `directory` (default: the request Location; `all` for every Location), `limit` (default 50, max 200) and `cursor`. It returns active runs first, then the newest. `GET /workflows/runs/{id}` returns the same summary plus `agents_detail`, the existing agent list.

`workflow.run.progress` is a non-durable `LiveEvent` with data `{run_id, status, phase, agents, tokens, cost_usd}`. It is emitted at most every 250 ms per run, only when something changed: scheduler counts, or a durable event of the run. It lets the monitor show agents moving between `queued` and `running`, which leaves no durable record. Clients resync with `GET /workflows/runs` after reconnecting or after a `Lagged` gap. The event carries no data that the list cannot give.

### CLI

`cyber workflows runs`:

```
ID                          NAME      STATUS    PHASE    AGENTS d/r/q/f   TOKENS   COST    ELAPSED
run_01JA2…K3                audit     running   Verify   31/16/53/0       1.2M     $3.41   12m04s
run_01JA2…P9                review    running   Scan      0/ 2/ 0/0       18K      $0.04   0m21s
run_01JA2…Q1                migrate   queued    -         #1 in queue     -        -       -
run_01JA2…A7                plan      completed -         5/ 0/ 0/0       240K     $0.61   4m10s
```

- `--all` lists every Location.
- `--status running,queued` filters.
- `--watch` redraws in place on a TTY, from the progress event, falling back to a 1 s poll. Otherwise it prints one line per change. With `--format json` it prints one NDJSON object per change. It exits 0 on Ctrl-C without touching any run.

### TUI monitor

`/workflows` (and the palette entry "Workflows", and Enter on a sidebar Workflows row) opens `Overlay::Workflows`, with state in `App::workflows: WorkflowsState`:
- `runs: Loadable<Vec<Summary>>`, filled by `GET /workflows/runs` and patched by `workflow.run.progress` and `workflow.*` durable events;
- `selected`, `scope` (Location or all), and `detail: Option<RunDetail>`.

The list view, at 120 columns:

```
╭ Workflows · ~/work/cybercode-cli ───────────────────── 3 running · 1 queued ╮
│   NAME      STATUS     PHASE    PROGRESS                  AGENTS d/r/q/f  COST           ELAPSED │
│ › audit     running    Verify   ▀▀▀▀▀▀▀▀▀▀▀▀~[,,_,,]:3·····  31/16/53/0    $3.41 / $10   12m04s │
│   review    running    Scan     ▀~[,,_,,]:3···············   0/ 2/ 0/0    $0.04          0m21s │
│   docs      paused     Draft    ▀▀▀▀▀~[,,_,,]:3···········   4/ 0/ 0/0    $0.30          3m12s │
│             (2 settling)                                                                       │
│   migrate   queued     -        #1 in queue                  -            -              -     │
│   plan      completed  -        ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀   5/ 0/ 0/0    $0.61          4m10s │
│                                                                                                │
│ enter open · p pause/resume · r resume · x stop · R resume interrupted · s script · a all · esc │
╰────────────────────────────────────────────────────────────────────────────────────────────────╯
```

The run view, after Enter on `audit`:

```
╭ audit · run_01JA2…K3 · running · revision 1 ──────────────── $3.41 / $10.00 · 12m04s ╮
│ ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀~[,,_,,]:3························ 31/100 agents · 31%          │
│ ▼ Scan        20/20                                                                  │
│     ✓ agt_…01  scan auth/        fast   12.1K  $0.02   41s                           │
│     ✓ agt_…02  scan billing/     fast    9.8K  $0.02   38s                           │
│     …                                                                                │
│ ▼ Verify      11/80  · 16 running · 53 queued                                        │
│   › ● agt_…33  verify finding 7  verify 31.0K  $0.18   1m02s  cyber/run-01ja2…/33    │
│     ● agt_…34  verify finding 8  verify 22.4K  $0.12   0m58s                         │
│     ◌ 53 queued                                                                      │
│ ▶ Report      0/0                                                                    │
│                                                                                      │
│ enter/t transcript · p pause · x stop · l log · s script · esc back                  │
╰──────────────────────────────────────────────────────────────────────────────────────╯
```

At 80 columns, PHASE and ELAPSED drop and the bar shrinks to 10 cells. Below 60 columns, each run takes two rows: name, status and counts, then the bar.

Enter or `t` on an agent opens its child Session transcript in the agent thread view (`children.rs`), live. Esc returns to the run view.

- `p` pauses a running run, or resumes a paused one.
- `r` resumes a paused, failed, stopped or budget-exceeded run. A `WorkflowReplayMismatch` asks "Script changed — start revision N+1?" and retries with `revision: true`.
- `x` asks for confirmation, then stops.
- `R` resumes every interrupted run.
- `s` opens `script_file` (or the saved module path) in `$EDITOR`.
- `l` shows the log.

Every action goes through the public routes.

The nyan bar reuses the improve-tui-experience indicator, in its known-progress mode (completed over total). It is drawn static, with no frames, unless the row is selected and the run is running, so a list of 20 runs schedules frames for at most one of them. `NO_COLOR` and reduced motion use that indicator's fallbacks. Screen readers get `audit, running, phase Verify, 31 of 100 agents done, 16 running, 53 queued, 3 dollars 41 of 10, 12 minutes`.

### Indicators and notifications

- **Sidebar.** A Workflows section, after Context, listing runs of the current Session first, then other active runs of the Location, at most 5 rows plus `+N more`. Each row is name, a 6-cell bar and `d/t`. The section is omitted when no run is active.
- **Status line.** The `workflows` field shows `⚙ 2 workflows`, or `⚙ 2 workflows · 1 queued`. It is absent when none is active.
- **Progress line.** With one running run of this Session, the nyan progress line shows its fraction, as improve-tui's "Workflow progress" scenario specifies. With several, it shows the sum of completed over the sum of totals, labeled `3 workflows 42/130 agents · 32%`.
- **Notifications.** When a run reaches a terminal status, or is paused by a restart, a one-line notice is added to the transcript of its owner Session when that Session is open, and a toast appears otherwise: `✓ audit completed · 100 agents · $4.10 · 21m`, `✗ review failed: AgentFailed …` or `■ migrate stopped`. `tui.notifications` delivers the same text when the terminal is unfocused. `tui.workflow_notifications` selects `own` (the default: runs of the current Session and runs this TUI started or acted on), `location` or `off`.

## Risks / Trade-offs

- **Fairness is per slot, not per token.** A run of cheap fast agents and a run of slow verify agents get equal slot turns, not equal spend. Per-run `max_concurrent` is the escape hatch, and the monitor shows cost per run.
- **Slot release order.** Within one scheduler tick, a freed slot can be taken by a run whose head call was issued later than another run's. That is intended: fairness across runs, FIFO within a run.
- **A queued run consumes no budget time, but it does wait.** The CLI `--wait` and the SDK `result()` treat `queued` as not ended, as they treat `running` today.
- **Live event volume.** The progress event is coalesced to 4 per second per run, and is emitted only when something changed. 64 runs give at most 256 per second, and that only during dispatch storms.
- **Changed test.** `the_global_limit_is_fifo_across_runs` encodes the old behavior. It becomes `the_global_limit_is_shared_fairly_across_runs`, and the spec change is called out in the PR.

## Migration

No table migration: `workflow_run.status` is free text, and the run budget's `activated_ms` is already nullable. Runs recorded by M2.1 have no `dispatched` event. They are treated as dispatched at their `started` or `resumed` time. New keys are read with defaults (`max_running_runs` 8, `max_concurrent_per_run` unset, `resume_after_restart` false) and validated through the existing `WorkflowSettings` ranges.
