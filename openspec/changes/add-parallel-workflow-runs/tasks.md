Specified 2026-10-16. Not started. All requirements are P2.

Each numbered section is one increment that ships working behavior with its own tests. Every test runs locally:
- runtime tests use the fake agent host of `crates/cyber-server/tests/workflow_runs.rs` or the scripted models of `crates/cyber-tools/tests/workflow_agents.rs`;
- production-path tests use the App over authenticated TCP with the OpenAI-compatible scripted provider in `crates/cyber-app/tests/support/scripted_model.rs`, extended with per-prompt gates and a request log of `(prompt prefix, start time)`;
- TUI tests use `TestBackend` and `insta`, against that same server.

Archive after `implement-m2-1-workflow-runtime`. Sections 9 and 10 build on `improve-tui-experience` increments 4 and 8.8.

## 1. Fair agent scheduler

- [ ] 1.1 Add `runtime/workflow_scheduler.rs`: `WorkflowScheduler { capacity, runs: BTreeMap<run_id, RunQueue>, tick }` with `enqueue(run_id, share) -> Waiter`, an RAII `Slot`, `suspend` and `unsuspend`, `remove(run_id)`, `set_share` and `snapshot(run_id) -> {queued, running}`. Grants go by least recent grant (never-granted first), then the time the run became eligible, then run ID. The scheduler is work-conserving, skips dropped waiters, and keeps `last_grant` while a queue is empty. Unit tests drive it synchronously, with no runtime or clock, through every "Fair agent scheduling" scenario: not starved (16/84 then 2), equal split (4 slots, 10 + 10), idle slots used, per-run share 4 of 16, FIFO within a run, suspend dropping waiters, and a dropped waiter not leaking a slot. Add a property test: for random arrival and release sequences with R ≤ 8 runs, no slot is idle while a run is eligible, and every head waiter is granted within R grants.
- [ ] 1.2 Replace `Inner::workflow_slots` (the semaphore) with the scheduler. `RunHost::slot()` enqueues with the run's share and races the grant against the pause, stop and cancel signals, as today. Pause and stop call `suspend`; an attempt's end calls `remove`. `configure_workflow_concurrency` sets the capacity once. Replace `Runtime::workflow_slots()` with `workflow_scheduler()` and update `crates/cyber-app/tests/windows_service.rs:226` and `crates/cyber-app/src/lib.rs:231-236`. The `Slot` travels into `ChildGuard` in place of the semaphore permit (`cyber-tools/src/tools/agent.rs` `run_workflow` and `spawn_child`).
- [ ] 1.3 Per-run share: `max_concurrent` on `StartWorkflowRun` and `ResumeWorkflowRun`, `meta.max_concurrent` (pure literal, positive integer), `workflows.max_concurrent_per_run` (1–256, unset by default) in `crates/cyber-core/src/config/workflows.rs`, capped at the global limit and recorded on the run as `share`. It is accepted by `POST /workflows/runs`, the resume body and the `workflow` tool's input schema, and is excluded from `config_digest`. Tests: a non-positive value rejects with `ExecutionFailure` (meta) or `400` (API); changing the share on resume needs no revision.
- [ ] 1.4 Rewrite `the_global_limit_is_fifo_across_runs` (`crates/cyber-server/tests/workflow_runs.rs:532`) as `the_global_limit_is_shared_fairly_across_runs`: with a limit of 2, run A fans out 6 and run B then 2, and B's agents start on the first and third releases after B queues. Keep `a_run_paused_while_queued_for_a_slot_leaves_the_queue_at_once` and `agents_queued_behind_the_global_limit_do_not_start_after_a_pause` green.

## 2. Independent lifecycle and cross-run isolation

- [ ] 2.1 Replace `workflow_lifecycle: Mutex<()>` with per-run locks (`workflow_locks`). Pause, stop, resume, dispatch and `finish` take only their run's lock. Shutdown and recovery take one run's lock at a time. Test "Stop does not block another run": the fake agent host holds A's cancelled agents for 2 s while B is paused and B's script returns, and B's pause and end are recorded within 100 ms.
- [ ] 2.2 Location writer registry: `Inner::workflow_writers` keyed by canonical Location replaces `RunHost::writers`. Tests (the `workflow_agents` scripted models in a git checkout): "Writers of two runs isolated"; "Single writer in place" with no other run; `isolation: "none"` still opts out across runs.
- [ ] 2.3 Run isolation tests: "Budget halt stays in its run" (two runs of one owner with scripted costs); "Same script, separate replay" (identical inline scripts, the second paused and resumed, and its replay record holds only its own calls); "Worktrees of two runs" (six distinct `run-<id>-<n>` names and branches); the owner Session's usage equals the sum of both runs.
- [ ] 2.4 "Concurrent runs" scenario "Two runs from one Session" through the `workflow` tool (`crates/cyber-tools/tests/workflow_agents.rs`): the owner is prompted while both run, and stopping one leaves the other running and its handback intact.

## 3. Run queue

- [ ] 3.1 Add `runtime/workflow_queue.rs` and `workflows.max_running_runs` (default 8, range 1–64). A run holds a slot from dispatch until its `Control` is no longer busy. Start and resume record `queued` when no slot is free. Add `workflow.run.dispatched.1` (registered, projected to `running`, activating the budget clock). The budget row of a queued run starts with `activated_ms` NULL. Dispatch goes FIFO by queue time, then ID. `queue_position` is computed on read.
- [ ] 3.2 Pause and stop of a queued run (`paused`, which leaves the queue; `stopped` with one summary). A resume re-queues at the end. Dispatch re-reads the run under its lock, so a pause racing dispatch never starts an engine.
- [ ] 3.3 The `workflow` tool, the API and the CLI report `queued` with `queue_position`. `run --wait` and the SDK's `result()` keep waiting while queued.
- [ ] 3.4 Tests: "Ninth run queued" (limit 2, three runs), "Dispatch order", "Stop a queued run", "Pause a queued run", "Wall budget waits in the queue" (a fixed clock in the budget projection test, plus a 0.3 s wall budget that survives a 1 s queue wait).

## 4. Recovery of several runs

- [ ] 4.1 `recover_workflow_runs`: in `started_ms` order, `running` and `waiting_input` runs become `paused` with `interrupted: true`; `queued` runs stay queued and are dispatched after recovery; then handbacks are admitted. Fix the current pause of every `is_active` run (`workflow_control.rs:339-353`).
- [ ] 4.2 `workflows.resume_after_restart` (default `false`). `POST /api/v1/workflows/runs/resume_interrupted` (`directory`, `all`) answers `{ resumed: [run_id], refused: [{ run_id, reason }] }`. `cyber workflows resume --interrupted`.
- [ ] 4.3 Tests (two runtimes over one store, as `shutdown_and_recovery_leave_running_runs_paused_for_resume`): "Crash with two runs mid-flight" with a third queued; each resume continues only its own interrupted child Sessions (the fake host records `resume: true` per agent ID); "Automatic resume after restart" with one edited saved module; "Resume every interrupted run" leaving a user-paused run paused.

## 5. Coalesced handback

- [ ] 5.1 In `drain.rs` `promote`, promote up to 8 consecutive `workflow`-source queued messages at the head of the queue together, in admission order. Steer and other sources are unchanged.
- [ ] 5.2 Tests: "Two runs end together" (the scripted model sees one request containing both summaries in admission order); a user message queued between two summaries splits them into two Turns; recovery after a restart still admits each summary exactly once.

## 6. Progress summary, filters and live progress

- [ ] 6.1 `WorkflowRunSummary`: agent counts (one `GROUP BY` plus the scheduler snapshot), `tokens` and `cost_usd` from `run_spend`, `elapsed_ms` from active time, `share` and `queue_position`. Used by `GET /workflows/runs` and `GET /workflows/runs/{id}`, which keeps `agents_detail`.
- [ ] 6.2 Filters `status`, `session_id`, `directory` (`all`), `limit` and `cursor`, active first and then the newest. Regenerate OpenAPI (`sdk/openapi.json`) and the generated TypeScript types.
- [ ] 6.3 `LiveEvent::WorkflowProgress`, coalesced at 250 ms per run and emitted only on change, routed by the run's Location. Envelope type `workflow.run.progress`.
- [ ] 6.4 Tests (`crates/cyber-server/tests/http.rs` and `crates/cyber-app/tests/workflows.rs`): "Queued agents counted" (16 running and 84 queued with the scripted provider gated); "Filter active runs" across two Locations; pagination; the progress event's rate bound with a fixed clock; no event without a change; resync after a forced `Lagged` gives the same counts.

## 7. CLI

- [ ] 7.1 `cyber workflows runs` columns (ID, NAME, STATUS, PHASE, AGENTS d/r/q/f or `#N in queue`, TOKENS, COST, ELAPSED), `--all` and `--status`. JSON output is the summary list.
- [ ] 7.2 `runs --watch`: in-place redraw on a TTY from the instance stream (`scope=all` with `--all`), falling back to a 1 s poll; one line or one NDJSON object per change otherwise; exit 0 on Ctrl-C with no run touched.
- [ ] 7.3 `run --max-concurrent N` and `resume --interrupted`.
- [ ] 7.4 Tests with the real binary (`crates/cyber-cli/tests/cli.rs`) against a server with the scripted provider: the table with two running runs and one queued; `--status`; "Watch several runs" in non-TTY mode, asserting change lines and exit 0 after SIGINT while the runs stay active; `--format json --watch` NDJSON.

## 8. SDK

- [ ] 8.1 `client.workflows.list(filter)` (lazy pagination, handles carrying summaries), `client.workflows.get(id)`, `max_concurrent` on `run`, `queued` not counted as ended, and `events()` including `workflow.run.progress`.
- [ ] 8.2 Tests (`sdk/typescript/test`, against the embedded server with the scripted provider): "Attach to running runs" with three runs; `get` on a run started over HTTP. Add an example `examples/parallel-workflows.ts`, type-checked in CI.

## 9. TUI monitor

- [ ] 9.1 `crates/cyber-tui/src/workflows.rs` (`WorkflowsState`: summaries, selection, scope, run detail, and patching from `workflow.run.progress` and `workflow.*` events, with a re-read on reconnect) and `view/workflows.rs` (list and run views as sketched in design.md, with the 120, 80 and 60 column layouts). `/workflows`, the palette entry and the `workflows` keybinding context with `workflows_pause`, `workflows_resume`, `workflows_stop`, `workflows_resume_interrupted`, `workflows_open`, `workflows_script`, `workflows_log` and `workflows_scope`.
- [ ] 9.2 Actions over the public routes: `p`, `r` (with the revision prompt on `WorkflowReplayMismatch`), `x` with confirmation, `R`, `s` (`$EDITOR`), `l`, and Enter/`t` into the agent thread view (`children.rs`), with Esc back to the run and the selection kept. Refusals are shown on the row.
- [ ] 9.3 Per-row nyan bar in known-progress mode, using the improve-tui indicator. Only the selected running row animates. The `NO_COLOR`, 16-color and reduced-motion fallbacks apply. Screen-reader sentences for each row.
- [ ] 9.4 Tests: snapshots at 160×48, 120×40, 80×24 and 50×20, and in `NO_COLOR`, with four runs (running, paused settling, queued, completed) from the production server; "Pause a run" (`paused (6 settling)` then `paused`); "Several runs at once" (pausing one leaves the others' rows unchanged); "Drill into an agent" with a streaming child transcript; `x` confirmation; `r` revision prompt; the frame-rate bound with 20 rows (at most one row animates).

## 10. TUI indicators and notifications

- [ ] 10.1 The sidebar Workflows section (current Session first, at most 5 rows plus `+N more`, Enter opens the monitor), the `workflows` status line field, and the aggregate known-progress feed for the nyan progress line. Tests: "Two runs in the sidebar", "Aggregate progress" (32%), "Queued count in the footer", and the section gone after the last run ends.
- [ ] 10.2 Run-end notices: the transcript line or toast, `tui.notifications` when unfocused, `tui.workflow_notifications` (`own`, `location`, `off`), and once per attempt per TUI across reconnects. Tests: "Background run finishes" (unfocused, OSC 9 captured), "Run of another Session", "No duplicate after reconnect", and the restart notice `⏸ … paused by restart`.

## 11. End-to-end acceptance

- [ ] 11.1 `crates/cyber-app/tests/parallel_workflows.rs`, on the production App with `workflows.max_concurrent: 4` and `max_running_runs: 3`, and the scripted provider with gated responses. Start three runs within one second:
  - A, `cyber workflows run` with the real binary, 12 agents;
  - B, `POST /api/v1/workflows/runs`, 2 agents;
  - C, the `workflow` tool in Session S, 6 agents with `max_concurrent: 2`.
  
  Assert all three are listed as running with correct `queued` counts. Releasing gates one at a time, assert the provider's start log follows the scheduler's least-recently-granted order, that B's two agents start within the first three releases after B queues, and that C never exceeds 2.
- [ ] 11.2 Independent control in the same test: pause B (its queued calls leave, the running one finishes, A and C are unaffected); stop C (cancelled agents; A's next start is not delayed); resume B and C; a fourth run queues and is dispatched when A ends. Each run's summary reaches its owner exactly once. S's two summaries (C's two attempts) are counted per attempt.
- [ ] 11.3 Crash and recovery with the real `cyber serve` process: SIGKILL with A and B mid-flight (gates held) and a third run queued. Restart: A and B are `paused` (`interrupted`), and the queued run is dispatched. `cyber workflows resume --interrupted` resumes both, each agent's child Session resumes once (provider log), and all runs complete with the agent counts of an uninterrupted run. Repeat with `resume_after_restart: true`, with no manual step.
- [ ] 11.4 Drive the TUI against the 11.1 server with `TestBackend`: `/workflows` shows the three runs with live counts, `p`, `x` and `r` act on the selected run only, and the run-end notices appear. Record the snapshots as acceptance evidence.

## 12. Documentation and archive

- [ ] 12.1 Update `docs/features.md`, `docs/local-core-guide.md` (several runs, the run queue, fair scheduling and `max_concurrent`, recovery and `resume --interrupted`, `/workflows` keys, `tui.workflow_notifications`) and `docs/implementation/p2-status.md` with the evidence from section 11.
- [ ] 12.2 Run `openspec validate --all --strict`. Archive after `implement-m2-1-workflow-runtime`, and update the ROADMAP requirement table with `scripts/spec_inventory.py`.
