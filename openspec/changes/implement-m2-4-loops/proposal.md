## Why

`/loop` re-runs a prompt in a Session on an interval, or at a pace the model chooses. Users check a deploy every 5 minutes, poll CI until it's green, or babysit a PR. Claude Code has it, and `loops-scheduling` specifies it (M2.4), but nothing is built: `loop` is only a reserved command name, and `cyber loops` reports "not available in this build yet".

This change delivers the Session-scoped half of M2.4, the `/loop` users type. The scheduler-backed half follows in a second change: in-session cron tools, persistent `cyber loops add` schedules run by the service, jitter, and notifications.

## What Changes

Implemented `loops-scheduling` requirements (P2):

- **Fixed-interval loop:** `/loop <interval> <prompt>` with a minimum interval of 60 s.
- **Self-paced loop:** `/loop <prompt>` plus the model tool `schedule_wakeup` (delay clamped to 60–3600 s, a reason, a `noop` flag).
- **Loop iteration delivery:** iterations are admitted as `queue` with `source: "loop"`. Overlap policy is `skip` (the default) or `coalesce`.
- **Noop collapsing:** consecutive quiet iterations are collapsed into one `N quiet checks` line in the TUI.
- **Loop stop conditions:** `--max`, `--for`, `--until "<cmd>"`, `/loop stop [id|all]`, and `loops.max_iterations` (default 500). `--until-goal` waits for M2.3 goals.
- **Restore on resume:** loops survive a server restart. Missed-run policy is `skip` (the default) or `run-once`.
- **Loop status visibility:** `/loop list`, and `GET /api/v1/sessions/{id}/loops`.
- **Budgets for loops:** `--max-cost`, `--max-tokens`, `--max-turns` and `--timeout` across iterations, defaulted by `budgets.loop`. Exceeding one stops the loop with `budget_exceeded`.
- **Loop events and hooks:** the durable events `loop.created.1`, `loop.iteration.started.1`, `loop.iteration.ended.1` and `loop.stopped.1`. The `LoopIteration` hook is not part of this change.

Spec deltas (`loops-scheduling`):

- **Self-paced loop:** `schedule_wakeup` is also offered in interval loops, where only `noop` and `reason` are used. After 3 consecutive failed iterations the loop stops with status `failed`.
- **Loop events and hooks:** adds `loop.wakeup.1`, so a wakeup survives a crash before the iteration ends, and an `outcome` on `loop.iteration.ended.1` (`completed`, `interrupted`, `failed`, `skipped_overlap`).
- **Restore on resume:** a self-paced loop whose wakeup time passed while the server was down runs once, under either missed-run policy.

## Non-goals (follow-up change `implement-m2-4-schedules`)

- `cron_create|cron_list|cron_delete`, persistent `cyber loops add|list|show|pause|resume|run|rm`, jitter, desktop and remote notifications, the `LoopIteration`/`ScheduleRun` hooks, and `promote` (P3).
- `--until-goal`, which needs M2.3 goals.
- `/loop` in `cyber exec`. A loop needs a long-lived Session; `exec` exits when the Turn ends.

## Impact

- **`cyber-store`:** a `session_loop` table (migration).
- **`cyber-server`:**
  - `runtime/loops/`: events, fold, projection, scheduler sweep, iteration lifecycle at `drain_ended`, and restore;
  - the routes `GET|POST /sessions/{id}/loops` and `DELETE /sessions/{id}/loops[/{loop_id}]`;
  - OpenAPI and the regenerated SDK.
- **`cyber-core`:** the `/loop` argument parser (interval, flags, stop and list subcommands) and the `loops.*` config defaults.
- **`cyber-tools`:** the `schedule_wakeup` tool (offered only while an iteration is in flight), plus a sandboxed `--until` check that returns its exit code.
- **`cyber-tui`:** `/loop` dispatch, `/loop list` output, loop iteration and quiet-check lines, `/help` and the palette.
- **Docs:** `README.md`, `docs/local-core-guide.md` and `ROADMAP.md` (M2.4 status).
