## Why

`implement-m2-4-loops` shipped the Session loops users type (`/loop`). The rest of M2.4 is still missing: the model cannot schedule a reminder for later, nothing runs while every terminal is closed, and loops have no hooks, notifications or `--until-goal`. `cyber loops` still answers "not available in this build yet".

## What Changes

The remaining (P2) `loops-scheduling` requirements, delivered in two pull requests.

**Part A — in-session cron**

- **In-session cron tools:** the model gets `cron_create` (a 5-field cron expression in local time, or `at` for one shot), `cron_list` and `cron_delete`. A task is a `cron` Loop of the Session with a `cron_` ID. It shares the `loops.max_per_session` cap (default 50) with `/loop`.
- **Restore on resume:** cron tasks are restored like loops. Missed runs follow the missed-run policy. A one-shot task whose time passed under `skip` ends `expired`.
- **Loop status visibility:** `/loop list` and `GET /sessions/{id}/loops` show the `cron` kind, its expression or time, and the next run time.
- **Loop events and hooks:** cron tasks use the `loop.*` events. The `LoopIteration` hook fires when an iteration ends.
- **Loop stop conditions:** `--until-goal`.
- **Completion notifications:** iterations that report `noop: false` or fail notify, and so do loop stops for `failed` or `budget_exceeded`. `/loop --notify always|on_change|never` controls them.

**Part B — persistent schedules**

- **Persistent local schedules:** `cyber loops add|list|show|pause|resume|run|rm` manage schedules stored in the database. The `cyber service` server runs them with no client open. Each run creates a new Session or resumes one and records its outcome.
- **Unattended Mode for schedules:** the default Mode is `dont-ask`. `bypass` needs `loops.allow_bypass` from user or managed config. Asks are denied with `Denied: no approver in unattended schedule`. Runs happen only in trusted directories.
- **Jitter:** each schedule gets a deterministic offset of up to `loops.jitter_seconds` (default 60). It is included in the next run time.
- **Completion notifications** for scheduled runs: in the run's Session, and on the desktop through the server.
- **Budgets for loops** across scheduled runs.
- **Loop events and hooks:** `schedule.run.1` and the `ScheduleRun` hook.
- **Usage limit waits:** a scheduled run counts as unattended.

Out of scope: **Cloud handoff of schedules** (P3, `cyber loops promote`).

Spec deltas (`loops-scheduling`) are listed in `design.md` → Spec deltas:

- **Persistent local schedules:** there is no `cyber service install`, so the warning offers `cyber service start`. An agent subprocess cannot manage schedules.
- **Unattended Mode for schedules:** the trusted-directory rule, re-checking at every run, and the `refused` outcome.
- **In-session cron tools:** offered only to root Sessions outside Workflow Runs. A one-shot time in the past is rejected.
- **Restore on resume:** a missed one-shot cron task under `skip` expires.
- **Completion notifications:** scheduled runs report `noop` through `schedule_wakeup`, `/loop` accepts `--notify`, and the per-Session notification rate limit applies.

## Impact

- **`cyber-core`:**
  - a `cron` module with the parser and the next-occurrence logic (time-zone generic);
  - `loops` (the `--until-goal` and `--notify` flags, and the `jitter_seconds` and `allow_bypass` settings);
  - `loops::allow_bypass`, which reads the setting from user or managed layers only.
- **`cyber-store`:** new migrations for `schedule` and `schedule_run`. Part A needs none: cron tasks reuse `session_loop`.
- **`cyber-server`:**
  - `runtime/loops` (the cron kind, `--until-goal`, notifications and the `LoopIteration` hook);
  - a new `runtime/schedules` (records, projector, sweep, run lifecycle, restore, desktop notifier port);
  - `/api/v1/schedules` routes, OpenAPI and the SDK;
  - the usage-wait unattended marker.
- **`cyber-tools`:**
  - the `cron_create`, `cron_list` and `cron_delete` tools;
  - `schedule_wakeup` offered during scheduled runs;
  - the schedule deny message;
  - `LoopIteration` and `ScheduleRun` as runtime lifecycle events.
- **`cyber-cli`:** `cyber loops`, and the desktop notifier wiring in `cyber-app`.
- **`cyber-tui`:** cron rows in `/loop list`, and `/loop` help.
- **Docs:**
  - `docs/local-core-guide.md`;
  - `docs/cli.md`;
  - `README.md`;
  - `ROADMAP.md` (M2.4 status).
