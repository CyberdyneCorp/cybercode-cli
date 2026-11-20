## MODIFIED Requirements

### Requirement: In-session cron tools
(P2) The model SHALL have `cron_create` (5-field cron expression in the user's local time zone, or `at` ISO timestamp for one-shot, plus prompt), `cron_list` and `cron_delete` tools, creating Session-scoped scheduled tasks (`cron_` IDs) with at most `loops.max_per_session` (default 50) per Session. Invalid expressions SHALL be rejected with the parse error. The tools SHALL be offered only to root Sessions that are not Workflow agents. A scheduled task SHALL be a Loop of kind `cron` that shares the per-Session cap with `/loop` loops. An `at` time that is not in the future, or is more than one year ahead, SHALL be rejected. `cron_delete` SHALL stop only `cron_` tasks of its Session. `cron_create` SHALL accept `missed` (`skip` or `run-once`) and `notify` (`always`, `on_change` or `never`). The model SHALL NOT be able to create a persistent schedule.

#### Scenario: Reminder
- **WHEN** the model calls `cron_create` with `at: "2026-10-03T09:00"` and prompt `summarize overnight CI`
- **THEN** the prompt is admitted to the Session at that time

#### Scenario: Limit
- **WHEN** a Session already has 50 scheduled tasks
- **THEN** `cron_create` fails with `Scheduled task limit reached (50)`

#### Scenario: Invalid expression
- **WHEN** the model calls `cron_create` with `cron: "61 * * * *"`
- **THEN** the call fails with `Invalid cron expression "61 * * * *"` and the reason

#### Scenario: A user loop is out of reach
- **WHEN** the model calls `cron_delete` with the ID of a `/loop` the user started
- **THEN** the call fails with `Unknown scheduled task <id>` and the loop keeps running

### Requirement: Restore on resume
(P2) Session-scoped Loops and cron tasks SHALL be persisted with the Session and restored on resume, server restart or handoff, unless expired. Iterations missed while the Session was not running SHALL follow the missed-run policy (default `skip`; `run-once` admits a single catch-up iteration). A self-paced Loop whose wakeup time passed while the server was not running SHALL run one iteration under either policy. An iteration that was running when the server stopped SHALL end with outcome `interrupted`. A recurring cron task that missed occurrences SHALL resume at its next occurrence under `skip`. A one-shot cron task whose time passed under `skip` SHALL end with status `expired`.

#### Scenario: Missed runs while offline
- **WHEN** a 1-hour loop was not running for 5 hours with policy `run-once`
- **THEN** on restore exactly one catch-up iteration runs, then the normal schedule resumes

#### Scenario: Skip after restart
- **WHEN** a 5-minute loop with policy `skip` was not running for 1 hour
- **THEN** on restore no catch-up iteration runs and the next iteration is admitted at the next 5-minute tick

#### Scenario: Missed one-shot reminder
- **WHEN** a cron task with `at` 09:00 and policy `skip` is restored at 11:00
- **THEN** it ends with status `expired` and its prompt is not admitted

### Requirement: Persistent local schedules
(P2) `cyber loops add --cron "<expr>" [--dir <path>] [--agent <a>] [--mode <m>] [--new-session|--session <id>] "<prompt>"` SHALL create a persistent schedule stored in the database and executed by the `cyber service` daemon even when no client is open. `cyber loops list`, `show <id>`, `pause <id>`, `resume <id>`, `run <id>` (run now) and `rm <id>` SHALL manage them. Each scheduled run SHALL create or resume a Session and record its outcome. `add`, `resume` and `run` SHALL be refused inside an agent subprocess (`CYBER_SESSION_ID` set), and the schedule routes SHALL require the owner credential.

#### Scenario: Nightly review without open terminal
- **WHEN** a schedule `0 2 * * *` exists and the user's terminal is closed but the daemon runs
- **THEN** at 02:00 a Session runs the prompt and its result is recorded

#### Scenario: Daemon not running
- **WHEN** the user adds a persistent schedule and no Cyber service was running
- **THEN** the command warns that schedules run only while the service runs and offers `cyber service start`

#### Scenario: Agent subprocess
- **WHEN** a model's shell command runs `cyber loops add --cron "* * * * *" "x"`
- **THEN** the command is refused and no schedule is created

### Requirement: Unattended Mode for schedules
(P2) Persistent schedules SHALL run in the Mode set on the schedule (default `dont-ask`), and SHALL NOT run in `bypass` unless `loops.allow_bypass` is true in user or managed config. Permission asks in `dont-ask` SHALL be denied with feedback so the run can continue. A project, local, environment, profile or command-line layer SHALL NOT enable `loops.allow_bypass`. A schedule SHALL run only in an existing directory whose checkout is trusted. The directory, the trust and the Mode SHALL be checked when the schedule is added and again before every run. A run that fails a check SHALL end with outcome `refused` and its reason. Asks that no approver can answer during a scheduled run, in any Mode, SHALL be denied with the same message.

#### Scenario: Ask denied in schedule
- **WHEN** a scheduled run attempts a command needing approval under `dont-ask`
- **THEN** the command is denied and the model receives `Denied: no approver in unattended schedule`

#### Scenario: Project config cannot allow bypass
- **WHEN** a trusted checkout's `cyber.jsonc` sets `loops.allow_bypass: true` and the user adds a schedule with `--mode bypass`
- **THEN** the schedule is rejected because `bypass` needs `loops.allow_bypass` in user or managed config

#### Scenario: Untrusted checkout
- **WHEN** a schedule's directory has project definitions that were changed after approval
- **THEN** the next run ends `refused`, the user is notified, and no Session runs the prompt

### Requirement: Completion notifications
(P2) Loop iterations and scheduled runs that report `noop: false` or fail SHALL produce a notification (desktop, and remote devices when remote-control is connected) with the schedule name and a one-line result; schedule-level `notify` SHALL accept `always`, `on_change` (default) or `never`. `/loop` and `cron_create` SHALL accept the same `notify` setting. A scheduled run SHALL report `noop` through `schedule_wakeup`, which SHALL be offered during scheduled runs; its delay SHALL be ignored there. A Loop stop with status `failed` or `budget_exceeded` SHALL notify unless `notify` is `never`. Notifications recorded in a Session SHALL follow the Session's notification rate limit.

#### Scenario: Failure notified
- **WHEN** a nightly scheduled run fails
- **THEN** the user receives a notification with the error

#### Scenario: Quiet run
- **WHEN** a scheduled run calls `schedule_wakeup` with `noop: true` and its schedule has `notify: on_change`
- **THEN** no notification is sent and the run is recorded with `noop: true`
