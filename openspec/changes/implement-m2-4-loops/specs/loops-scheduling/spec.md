## MODIFIED Requirements

### Requirement: Self-paced loop
(P2) `/loop <prompt>` without an interval SHALL create a self-paced Loop: after each iteration the model SHALL call the `schedule_wakeup` tool with `delay_seconds` (clamped to 60–3600), a one-sentence `reason` and a `noop` flag, and the next iteration SHALL be admitted after that delay. An iteration that ends without calling `schedule_wakeup` SHALL end the Loop with status `completed`. `schedule_wakeup` SHALL be offered to the model only while an iteration of an active Loop of the Session is running. In an interval Loop it SHALL record only `noop` and `reason`; the interval keeps the pace.

#### Scenario: Model picks delay
- **WHEN** the model calls `schedule_wakeup` with `delay_seconds: 1800` and reason `CI takes ~30 min`
- **THEN** the next iteration is admitted 1800 seconds later and the reason is shown to the user

#### Scenario: Clamp
- **WHEN** the model requests `delay_seconds: 10`
- **THEN** the wakeup is scheduled at 60 seconds

#### Scenario: Not offered outside a loop
- **WHEN** a Session has no active Loop
- **THEN** the model's tool list does not contain `schedule_wakeup`

### Requirement: Loop stop conditions
(P2) A Loop SHALL accept `--max <N>` iterations, `--until-goal` (stop when the Session's active Goal reaches a terminal status), `--until "<command>"` (stop when the command exits 0, run before each iteration) and `--for <duration>`, and SHALL stop on `/loop stop [id|all]`. Without stop conditions, a Loop SHALL end after `loops.max_iterations` (default 500). A Loop whose last 3 iterations ended with outcome `failed` SHALL stop with status `failed`. The `--until` command SHALL run sandboxed in the Session's Location without a permission prompt, as a user `!` command does, with a timeout of `loops.until_timeout_seconds` (default 60). A timeout or a non-zero exit SHALL continue the Loop.

#### Scenario: Until command succeeds
- **WHEN** a loop has `--until "curl -sf https://staging/health"`
- **THEN** the loop ends the first time the command exits 0 and reports why

#### Scenario: Repeated failures stop the loop
- **WHEN** three consecutive iterations of a loop fail with a provider error
- **THEN** the loop stops with status `failed` and the last error as its reason

### Requirement: Restore on resume
(P2) Session-scoped Loops and cron tasks SHALL be persisted with the Session and restored on resume, server restart or handoff, unless expired. Iterations missed while the Session was not running SHALL follow the missed-run policy (default `skip`; `run-once` admits a single catch-up iteration). A self-paced Loop whose wakeup time passed while the server was not running SHALL run one iteration under either policy. An iteration that was running when the server stopped SHALL end with outcome `interrupted`.

#### Scenario: Missed runs while offline
- **WHEN** a 1-hour loop was not running for 5 hours with policy `run-once`
- **THEN** on restore exactly one catch-up iteration runs, then the normal schedule resumes

#### Scenario: Skip after restart
- **WHEN** a 5-minute loop with policy `skip` was not running for 1 hour
- **THEN** on restore no catch-up iteration runs and the next iteration is admitted at the next 5-minute tick

### Requirement: Loop events and hooks
(P2) The system SHALL publish durable events `loop.created.1`, `loop.iteration.started.1`, `loop.wakeup.1` (the clamped `schedule_wakeup` request), `loop.iteration.ended.1` (with noop flag and `outcome`: `completed`, `interrupted`, `failed` or `skipped_overlap`), `loop.stopped.1` and `schedule.run.1`, and hooks SHALL be able to subscribe to `LoopIteration` and `ScheduleRun`.

#### Scenario: Event on iteration
- **WHEN** an iteration finishes
- **THEN** `loop.iteration.ended.1` carries the loop ID, noop flag, duration and cost

#### Scenario: Wakeup survives a crash
- **WHEN** the model calls `schedule_wakeup` and the server stops before the iteration ends
- **THEN** after restart the iteration ends as `interrupted` and the next iteration is scheduled from the recorded wakeup
