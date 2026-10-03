# loops-scheduling Specification

## Purpose
Loops and schedules re-run prompts over time: a fixed-interval or self-paced `/loop` in a Session, in-session cron tasks the model can create, and persistent local schedules run by the user's `cyber service` even when no client is open. The model is Claude Code's `/loop`, `ScheduleWakeup` and cron tools plus Desktop scheduled tasks, made durable by Cyber Code's runtime. Cloud execution of schedules is covered by the routines capability.

## Requirements

### Requirement: Fixed-interval loop
(P2) `/loop <interval> <prompt or /command>` SHALL create a Loop (`lop_` ID) that admits the prompt into the Session immediately and then every interval, where the interval accepts `Ns`, `Nm`, `Nh` or `Nd` with a minimum of 60 seconds. A shorter interval SHALL be rejected with `Loop interval must be at least 60s`.

#### Scenario: Five-minute loop
- **WHEN** the user runs `/loop 5m check the deploy status`
- **THEN** the prompt runs now and again every 5 minutes

#### Scenario: Interval too short
- **WHEN** the user runs `/loop 10s ping`
- **THEN** the loop is rejected with the minimum-interval error

### Requirement: Self-paced loop
(P2) `/loop <prompt>` without an interval SHALL create a self-paced Loop: after each iteration the model SHALL call the `schedule_wakeup` tool with `delay_seconds` (clamped to 60–3600), a one-sentence `reason` and a `noop` flag, and the next iteration SHALL be admitted after that delay. An iteration that ends without calling `schedule_wakeup` SHALL end the Loop with status `completed`.

#### Scenario: Model picks delay
- **WHEN** the model calls `schedule_wakeup` with `delay_seconds: 1800` and reason `CI takes ~30 min`
- **THEN** the next iteration is admitted 1800 seconds later and the reason is shown to the user

#### Scenario: Clamp
- **WHEN** the model requests `delay_seconds: 10`
- **THEN** the wakeup is scheduled at 60 seconds

### Requirement: Loop iteration delivery
(P2) Each iteration SHALL be admitted with `delivery: queue`, so it never interrupts a running Turn; if the previous iteration is still running when the next is due, the due iteration SHALL be skipped (overlap policy `skip`, default) or coalesced into one pending iteration (policy `coalesce`).

#### Scenario: Overlap skipped
- **WHEN** an iteration is still running when the next 5-minute tick arrives
- **THEN** the tick is skipped and recorded as `skipped_overlap`

### Requirement: Noop collapsing
(P2) Iterations SHALL report `noop: true` when nothing changed. Consecutive noop iterations SHALL be collapsed into a single counter line in clients, and SHALL NOT trigger notifications.

#### Scenario: Quiet iterations
- **WHEN** 6 consecutive iterations report noop
- **THEN** the transcript shows one line `6 quiet checks` instead of six iterations

### Requirement: Loop stop conditions
(P2) A Loop SHALL accept `--max <N>` iterations, `--until-goal` (stop when the Session's active Goal reaches a terminal status), `--until "<command>"` (stop when the command exits 0, run before each iteration) and `--for <duration>`, and SHALL stop on `/loop stop [id|all]`. Without stop conditions, a Loop SHALL end after `loops.max_iterations` (default 500).

#### Scenario: Until command succeeds
- **WHEN** a loop has `--until "curl -sf https://staging/health"`
- **THEN** the loop ends the first time the command exits 0 and reports why

### Requirement: In-session cron tools
(P2) The model SHALL have `cron_create` (5-field cron expression in the user's local time zone, or `at` ISO timestamp for one-shot, plus prompt), `cron_list` and `cron_delete` tools, creating Session-scoped scheduled tasks (`cron_` IDs) with at most `loops.max_per_session` (default 50) per Session. Invalid expressions SHALL be rejected with the parse error.

#### Scenario: Reminder
- **WHEN** the model calls `cron_create` with `at: "2026-10-03T09:00"` and prompt `summarize overnight CI`
- **THEN** the prompt is admitted to the Session at that time

#### Scenario: Limit
- **WHEN** a Session already has 50 scheduled tasks
- **THEN** `cron_create` fails with `Scheduled task limit reached (50)`

### Requirement: Restore on resume
(P2) Session-scoped Loops and cron tasks SHALL be persisted with the Session and restored on resume, server restart or handoff, unless expired. Iterations missed while the Session was not running SHALL follow the missed-run policy (default `skip`; `run-once` admits a single catch-up iteration).

#### Scenario: Missed runs while offline
- **WHEN** a 1-hour loop was not running for 5 hours with policy `run-once`
- **THEN** on restore exactly one catch-up iteration runs, then the normal schedule resumes

### Requirement: Persistent local schedules
(P2) `cyber loops add --cron "<expr>" [--dir <path>] [--agent <a>] [--mode <m>] [--new-session|--session <id>] "<prompt>"` SHALL create a persistent schedule stored in the database and executed by the `cyber service` daemon even when no client is open. `cyber loops list`, `show <id>`, `pause <id>`, `resume <id>`, `run <id>` (run now) and `rm <id>` SHALL manage them. Each scheduled run SHALL create or resume a Session and record its outcome.

#### Scenario: Nightly review without open terminal
- **WHEN** a schedule `0 2 * * *` exists and the user's terminal is closed but the daemon runs
- **THEN** at 02:00 a Session runs the prompt and its result is recorded

#### Scenario: Daemon not running
- **WHEN** the user adds a persistent schedule and `cyber service` is not installed
- **THEN** the command warns that schedules run only while the service runs and offers `cyber service install`

### Requirement: Unattended Mode for schedules
(P2) Persistent schedules SHALL run in the Mode set on the schedule (default `dont-ask`), and SHALL NOT run in `bypass` unless `loops.allow_bypass` is true in user or managed config. Permission asks in `dont-ask` SHALL be denied with feedback so the run can continue.

#### Scenario: Ask denied in schedule
- **WHEN** a scheduled run attempts a command needing approval under `dont-ask`
- **THEN** the command is denied and the model receives `Denied: no approver in unattended schedule`

### Requirement: Jitter
(P2) Persistent schedules SHALL apply a deterministic per-schedule jitter of up to `loops.jitter_seconds` (default 60) to spread load, and SHALL report the next run time including jitter.

#### Scenario: Next run shown
- **WHEN** the user runs `cyber loops list`
- **THEN** each schedule shows its next run time with jitter applied

### Requirement: Completion notifications
(P2) Loop iterations and scheduled runs that report `noop: false` or fail SHALL produce a notification (desktop, and remote devices when remote-control is connected) with the schedule name and a one-line result; schedule-level `notify` SHALL accept `always`, `on_change` (default) or `never`.

#### Scenario: Failure notified
- **WHEN** a nightly scheduled run fails
- **THEN** the user receives a notification with the error

### Requirement: Loop status visibility
(P2) `/loop list` and `GET /api/v1/sessions/{id}/loops` SHALL show each Loop's prompt, kind (`interval`, `self-paced`, `cron`), next run time, iterations run, last noop flag and stop conditions.

#### Scenario: Inspect loops
- **WHEN** the user runs `/loop list`
- **THEN** all active loops of the Session are listed with their next run time

### Requirement: Budgets for loops
(P2) A Loop or schedule SHALL accept `max_cost_usd` and `max_tokens` budgets across iterations; when exceeded, it SHALL stop with status `budget_exceeded` and notify.

#### Scenario: Loop budget
- **WHEN** a loop with `--max-cost 2` has spent $2.01 across iterations
- **THEN** it stops and the user is notified

### Requirement: Loop events and hooks
(P2) The system SHALL publish durable events `loop.created.1`, `loop.iteration.started.1`, `loop.iteration.ended.1` (with noop flag), `loop.stopped.1` and `schedule.run.1`, and hooks SHALL be able to subscribe to `LoopIteration` and `ScheduleRun`.

#### Scenario: Event on iteration
- **WHEN** an iteration finishes
- **THEN** `loop.iteration.ended.1` carries the loop ID, noop flag, duration and cost

### Requirement: Cloud handoff of schedules
(P3) A persistent schedule SHALL be convertible to a Routine (`cyber loops promote <id>`) that runs on a Runner, as defined by the routines capability, preserving prompt, Mode, agent and budgets.

#### Scenario: Promote to routine
- **WHEN** the user runs `cyber loops promote cron_123 --runner cloud`
- **THEN** a Routine with the same schedule is created and the local schedule is disabled
