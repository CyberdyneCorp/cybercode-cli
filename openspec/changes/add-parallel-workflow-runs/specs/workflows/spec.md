## ADDED Requirements

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

A queued run SHALL start no engine, spawn no agent, and accrue no `max_wall_seconds` time. A queued run MAY be paused, which leaves the queue and moves it to `paused`, or stopped, which moves it to `stopped` and hands its summary back. A resume puts a run back at the end of the queue.

#### Scenario: Ninth run queued
- **WHEN** 8 runs are running and a ninth is started with the default limit
- **THEN** the ninth is recorded `queued` with `queue_position: 1` and has no agents until one of the 8 ends

#### Scenario: Dispatch order
- **WHEN** runs Q1 and then Q2 are queued and one running run ends
- **THEN** Q1 is dispatched with `workflow.run.dispatched.1`, and Q2 moves to `queue_position: 1`

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

A run's share SHALL be its `max_concurrent`: the invocation option (API `max_concurrent`, `cyber workflows run --max-concurrent`, the `workflow` tool's `max_concurrent`), else `meta.max_concurrent`, else `workflows.max_concurrent_per_run` (default unset). Each source SHALL be a positive integer, and the share SHALL be capped at the global limit. A share may be changed by a resume. It SHALL NOT be part of the configuration digest, so changing it SHALL NOT require a revision.

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
(P2) On startup, recovery SHALL process every active run in `started_ms` order. A `running` or `waiting_input` run SHALL become `paused` with `interrupted: true`, and its unsettled agents SHALL be marked `interrupted`. A `queued` run SHALL stay queued in its order and SHALL be dispatched as run slots allow. Then the summary of every ended run that was not handed back SHALL be admitted.

When `workflows.resume_after_restart` is `true` (default `false`), the runs this recovery paused SHALL then be resumed in `started_ms` order through the run queue, as if the user had resumed each one. A run whose saved module changed SHALL stay `paused` with a `WorkflowReplayMismatch` reason. `POST /api/v1/workflows/runs/resume_interrupted` (for the request Location, or `directory=all`) and `cyber workflows resume --interrupted` SHALL resume the same set on demand: runs whose latest lifecycle event is a pause with `interrupted: true`. Each resumed run SHALL continue its own unsettled agents' child Sessions.

#### Scenario: Crash with two runs mid-flight
- **WHEN** the server is killed while runs A and B each have agents in flight and run C is queued
- **THEN** after restart A and B are `paused` with their in-flight agents `interrupted`, C is dispatched, and resuming A and B completes both, each resuming only its own interrupted child Sessions

#### Scenario: Automatic resume after restart
- **WHEN** `workflows.resume_after_restart` is `true` and the server restarts with two interrupted runs, one of whose saved scripts was edited
- **THEN** the unedited run is resumed and the edited one stays `paused` with reason `WorkflowReplayMismatch`

#### Scenario: Resume every interrupted run
- **WHEN** the user runs `cyber workflows resume --interrupted` after a restart left three runs paused with `interrupted: true` and one paused by the user
- **THEN** the three interrupted runs are resumed, and the user-paused run stays paused

## MODIFIED Requirements

### Requirement: Concurrency limit
(P2) The system SHALL run at most `workflows.max_concurrent` (default 16, range 1–256, reduced to the number of CPU cores when lower) workflow agents at once across all runs of a server, queueing further `agent()` calls and sharing the slots between runs as "Fair agent scheduling" defines. `parallel({ concurrency })` SHALL further limit a group but SHALL NOT exceed the global limit or the run's share. A `concurrency` that is not a positive integer SHALL reject with `ExecutionFailure`.

#### Scenario: Large fan-out queued
- **WHEN** a script launches 100 agents in parallel with the default limit on a 32-core machine
- **THEN** 16 run at a time and the rest wait

#### Scenario: Limit shared by runs
- **WHEN** two runs each launch 50 agents with a limit of 16
- **THEN** at most 16 of their agents run at once in total

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

### Requirement: Run monitor
(P2) The `/workflows` view and `GET /api/v1/workflows/runs` SHALL list runs with name, status, current phase, agents `done/running/queued/failed`, tokens, cost and elapsed time, and SHALL let the user open any agent's transcript, pause (`p`), resume, stop (`x`, as in the `tui` Workflow monitor) and open the script. Live updates SHALL stream over the event stream.

Each listed run SHALL carry:
- `agents: { total, completed, running, queued, failed, cancelled, interrupted }`, where `queued` counts issued calls waiting for a slot of the global limit;
- `tokens` and `cost_usd` of the run's agents;
- `elapsed_ms` of active time (time spent `queued` or `paused` excluded);
- `share`;
- `queue_position` while `queued`.

The list SHALL accept `status` (comma-separated), `session_id`, `directory` (default the request Location, `all` for every Location), `limit` (default 50, max 200) and `cursor`. It SHALL return active runs first, then the newest. `GET /api/v1/workflows/runs/{id}` SHALL return the same summary with the run's agents.

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
