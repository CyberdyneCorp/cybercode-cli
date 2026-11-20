## 1. Part A — core

- [x] 1.1 `cyber-core::cron`: the 5-field parser (ranges, steps, lists, names, macros, day OR rule), time-zone-generic `next_after` (DST gaps and folds), the 5-year bound, and the `at` parser; unit tests for every rule and error.
- [x] 1.2 `cyber-core::loops`: `--until-goal` and `--notify` flags, `Notify`, and the shared `Scheduled task limit reached` error; grammar tests.

## 2. Part A — runtime

- [x] 2.1 The `cron` Loop kind: the `cron` and `at_ms` fields on `loop.created.1`, validation, fold, next due after each iteration, one-shot completion, and `create_cron`, `cron_tasks` and `delete_cron`.
- [x] 2.2 Restore: missed occurrences of cron tasks under `skip` and `run-once`; a missed one-shot under `skip` ends `expired`.
- [x] 2.3 `--until-goal`: capture the current Goal, check it before and after each iteration, and the `goal_ended` status.
- [x] 2.4 Notifications for iterations and stops under `notify`, through `record_notification`.
- [x] 2.5 The `LoopIteration` hook after `loop.iteration.ended.1`.
- [x] 2.6 Runtime tests: cron reminder at its time, cap shared with `/loop`, recurring next due, restore under skip and run-once, expired one-shot, until-goal stop, notifications by policy, and the `LoopIteration` hook payload.

## 3. Part A — tools and surfaces

- [x] 3.1 `cron_create`, `cron_list` and `cron_delete`, offered only to root non-workflow Sessions, with `cron_delete` limited to `cron_` tasks; tool tests.
- [x] 3.2 `GET /sessions/{id}/loops` returns the cron fields; OpenAPI and the regenerated SDK.
- [x] 3.3 TUI: `/loop list` rows show the kind (`every 5m`, `self-paced`, `cron 0 9 * * 1-5`, `at 09:00`) the next run time and the stop conditions; row tests.
- [x] 3.4 Docs: the local core guide (cron tools, `--until-goal`, `--notify`, `LoopIteration`) and the ROADMAP M2.4 status.

## 4. Part B — persistent schedules

- [x] 4.1 Migrations `schedule` and `schedule_run`; the `schedule.created.1`, `schedule.updated.1` and `schedule.run.1` events, validation and projector.
- [x] 4.2 Settings `loops.jitter_seconds` and `loops.allow_bypass` (user and managed layers only), with tests that a project layer cannot enable it.
- [x] 4.3 Create, list, show, pause, resume, run now and remove on `Runtime`, with the gate (directory, trust, Mode) at creation.
- [x] 4.4 The sweep: due runs with jitter; overlap skip; the gate at run time with the `refused` outcome; new or resumed Session (switching its Mode); queued admission `schedi_<id>_<n>`.
- [x] 4.5 Run end at the Drain's idle exit: outcome, `noop` from `schedule_wakeup`, deltas, budget stop, notifications (Session and `DesktopNotifier`), and the `ScheduleRun` hook.
- [x] 4.6 Restore (interrupted runs, missed policy) and the background sweep, wired in `cyber-app` with the OS notifier.
- [x] 4.7 `Denied: no approver in unattended schedule` for asks in scheduled runs; the `unattended_marker` for `schedi_` runs.
- [x] 4.8 Routes `/api/v1/schedules…`, OpenAPI, the SDK, and HTTP tests.
- [x] 4.9 `cyber loops add|list|show|pause|resume|run|rm`, the service warning, refusal inside agent subprocesses, CLI tests, and `docs/cli.md`.
- [x] 4.10 Runtime tests: a nightly run with no client, overlap skip, jitter determinism and the next run, untrusted directory refused, bypass refused without the user opt-in, a dont-ask ask denied with the schedule message, budget stop with notification, interrupted run restore, missed run-once, and a usage-limit wait in a resumed Session.
- [x] 4.11 Docs: the local core guide (schedules, unattended rules, notifications), the README, and the ROADMAP M2.4 status.

## 5. Verification

- [x] 5.1 `cargo fmt`, clippy with `-D warnings`, touched-crate nextest, OpenSpec validate, `spec_lint`, `spec_inventory`, and `help-markdown` equal to `docs/cli.md`.
- [ ] 5.2 Live: a cron reminder and a nightly schedule against a real model (needs keys).
