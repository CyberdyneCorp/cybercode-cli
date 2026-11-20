## Context

`implement-m2-4-loops` built Session loops on four pieces:

- the `loop.*` events of the Session stream;
- the `session_loop` due-time index;
- a background sweep that admits queued iterations (`loopi_<loop>_<n>`);
- the Drain's idle exit (`loops_drain_ended`), which ends them.

Budgets, overlap, missed-run restore and usage-limit waits for loops already work. This change adds a third Loop kind for in-session cron, and a separate scheduler for persistent schedules. That scheduler runs in the `cyber serve` process that `cyber service start` registers.

## Decisions

### 1. In-session cron tasks are Loops of kind `cron`

A cron task is a Loop whose `kind` is `cron` and whose ID starts with `cron_`. It reuses every Loop mechanism: events, fold, `session_loop`, sweep, overlap, budgets, restore, the `/loop list` picker, the HTTP routes and `/loop stop <id>`. `loop.created.1` gains three optional fields: `cron` (the expression), `at_ms` (a one-shot time) and `notify`. Exactly one of `cron` and `at_ms` is set when `kind` is `cron`, and neither otherwise. As for interval loops, `next_due_ms` is the next occurrence after the iteration starts. An occurrence that comes while an iteration still runs is skipped (overlap `skip`), so occurrences never pile up.

A one-shot task stops `completed` after its single iteration. Iterations use `source: "loop"`, like `/loop`, so taint, usage origin and the Copilot initiator treat them as before.

The cap is shared: `/loop` loops and cron tasks together count against `loops.max_per_session` (default 50). The error is `Scheduled task limit reached (50)`.

### 2. Cron expressions (`cyber-core::cron`)

- **Fields:** five, separated by whitespace: minute, hour, day of month, month and day of week.
  - Each field accepts `*`, `N`, `A-B`, `*/S`, `A-B/S`, `N/S` and comma lists.
  - Month names `JAN`–`DEC` and day names `SUN`–`SAT` are accepted in any case. Day of week `7` is Sunday.
- **Macros:** `@hourly`, `@daily`, `@weekly`, `@monthly` and `@yearly`.
- **Day matching:** when both day of month and day of week are restricted, a day matches if either does, as classic cron does.
- **Next occurrence:** `next_after(after, tz)` returns the first whole minute strictly after `after` that matches in `tz`.
  - It is generic over `chrono::TimeZone`. The server uses `Local`; tests use UTC and fixed offsets.
  - A local time skipped by a DST change does not exist, so it is skipped.
  - An ambiguous local time takes its earlier instant.
- **Search bound:** the search is limited to 5 years. An expression with no occurrence in that time, such as `0 0 30 2 *`, is rejected when the task is created.
- **Errors:** `Invalid cron expression "<expr>": <reason>`.
- **`at`:** accepts RFC 3339 (`2026-10-03T09:00:00Z`, or with an offset) or a local `YYYY-MM-DDTHH:MM[:SS]`.
  - A time that is not in the future is rejected with `at must be in the future`.
  - A time more than one year ahead is rejected with `at must be within one year`.

### 3. The cron tools

- **`cron_create`:**
  - input `{cron?: string, at?: string, prompt: string, missed?: "skip"|"run-once", notify?: "always"|"on_change"|"never"}`, with exactly one of `cron` and `at`;
  - returns `{id, next_run}`.
- **`cron_list`:** the Session's active cron tasks, with `{id, cron|at, prompt, next_run, iterations}`.
- **`cron_delete`:**
  - `{id}`; it stops one active `cron_` task of this Session;
  - a `lop_` ID, or an unknown ID, fails with `Unknown scheduled task <id>`;
  - the model cannot stop the user's `/loop`.
- **Availability:** the tools are offered only to root Sessions (no `parent_id`) that are not Workflow agents (no `workflow_run_id`).
  - A child or a Workflow agent has a bounded lifetime its parent owns, and a timer there would outlive its purpose.
- **Permissions:** they need no permission. Their retry safety is `Never` for `cron_create` (a retry would create a second task), `ReadOnly` for `cron_list` and `Idempotent` for `cron_delete`.
  - Each task runs later in the Session's own Mode, with the authority the model already has now.
  - The cap and `loops.max_iterations` bound them.
- **Persistent schedules:** nothing the model can call creates one (decision 6).

### 4. `--until-goal`

`/loop … --until-goal` records the ID of the Session's current Goal (active or paused) in `stop.until_goal`. With no current Goal, the loop is rejected with `--until-goal needs a Goal in this Session`. The sweep checks before each iteration, and the iteration end checks again: when that Goal's status is terminal, the loop stops with the new status `goal_ended` and the reason `goal <id> <status>`.

### 5. Notifications

- **Which statuses:** a Loop has `notify` (`always`, `on_change` (default) or `never`; `/loop --notify`, `cron_create.notify`).
  - An ended iteration notifies when `notify` is `always`, or when it is `on_change` and the outcome is `failed`, or `completed` with `noop: false`.
  - A stop with status `failed` or `budget_exceeded` notifies unless `notify` is `never`.
- **How:** notifications are `session.notification.sent.1` events through `record_notification`. Clients already show them (TUI toast and desktop) and relay them to remote devices.
- **Rate limit:** the per-Session limit (10 per hour) applies. A notification past the limit is dropped, as for Goals, because the loop events still record the outcome.
- **Text:** title `Loop <short id>` or `Cron <short id>`, body `<one-line prompt>: <outcome>`.

### 6. Persistent schedules (`runtime/schedules`)

**Storage.** A schedule is its own aggregate, keyed by its `cron_<ulid>` ID. The `schedule.*` events are projected into new tables (new migration files):

```sql
CREATE TABLE schedule (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, cron TEXT NOT NULL, directory TEXT NOT NULL,
  spec TEXT NOT NULL,          -- JSON: prompt, agent, mode, target, notify, missed, budget, jitter_ms
  status TEXT NOT NULL,        -- active | paused | budget_exceeded | deleted
  next_due_ms INTEGER, inflight TEXT,  -- run ID in flight
  runs INTEGER NOT NULL DEFAULT 0, cost_usd REAL NOT NULL DEFAULT 0,
  tokens INTEGER NOT NULL DEFAULT 0, turns INTEGER NOT NULL DEFAULT 0,
  created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL) STRICT;
CREATE TABLE schedule_run (
  id TEXT PRIMARY KEY, schedule_id TEXT NOT NULL REFERENCES schedule(id) ON DELETE CASCADE,
  n INTEGER NOT NULL, due_ms INTEGER NOT NULL, started_ms INTEGER, ended_ms INTEGER,
  session_id TEXT, message_id TEXT, outcome TEXT NOT NULL, noop INTEGER NOT NULL DEFAULT 0,
  cost_usd REAL NOT NULL DEFAULT 0, reason TEXT) STRICT;
```

The tables are not named `schedule_occurrence`, which the reserved-budget collector already watches. Schedule budgets are soft (decision 9).

**Events.**

| Event | Payload |
|---|---|
| `schedule.created.1` | `{schedule_id, name, cron, directory, prompt, agent?, mode, model, session_id?, notify, missed, budget?, jitter_ms, created_ms, next_due_ms}`; no `session_id` creates a new Session per run |
| `schedule.updated.1` | `{schedule_id, status: active\|paused\|budget_exceeded\|deleted, reason?, next_due_ms?}` |
| `schedule.run.1` | `{schedule_id, run_id, n, phase: started\|reported\|ended, due_ms, session_id?, message_id?, outcome?, noop, duration_ms, cost, tokens, turns, cost_before, tokens_before, turns_before, reason?, next_due_ms?}` |

`schedule.run.1` is the single durable run record the spec names:

- the `started` phase is appended before the run is admitted;
- `reported` records `schedule_wakeup`'s `noop` and `reason`;
- `ended` carries the outcome: `completed`, `failed`, `interrupted`, `refused`, `skipped_overlap` or `skipped_missed`.

`rm` appends `updated {deleted}`. The row is kept for its history, and every listing hides it.

**Lifecycle.**

1. A sweep every 5 s (plus nudges) visits active schedules with `next_due_ms <= now`.
2. A schedule with a run in flight records `skipped_overlap` and moves on.
3. Otherwise the gate runs (decision 7). When it refuses, the run ends `refused` with the reason.
4. The target Session is prepared:
   - no `session_id`: a new root Session in `directory`, with the schedule's agent, model (the Location's default at `add` unless `--model`) and Mode, `unattended: true`, and the title `<name> · <local time>`;
   - `session_id`: the Session must exist, be a root Session in `directory`, and not be archived (checked by the gate). A Session that is running skips the run (`skipped_overlap`). Otherwise it is switched to the schedule's Mode first.
5. The prompt is admitted queued with `source: "loop"` and the deterministic ID `schedi_<schedule>_<n>`, so a retry after a crash is idempotent.
6. The Drain's idle exit (`schedules_drain_ended`, next to the loop hook) ends the run:
   - the outcome comes from the Drain status;
   - `noop` comes from the last `reported` phase;
   - cost, tokens and turns are the Session's totals minus those at admission.

   Then the budget is checked, notifications go out (decision 8), and the `ScheduleRun` hook fires.

**Restore.**

- A started run whose message was promoted, in a Session that does not run, ends `interrupted`.
- A run that was never promoted stays queued.
- A due time that passed while the server was down follows the schedule's `missed` policy, as interval loops do: `skip` records `skipped_missed` and moves to the next occurrence; `run-once` runs once.

**Next run with jitter.** `jitter_ms` is fixed when the schedule is created: the first 8 bytes of SHA-256 of the ID, modulo `loops.jitter_seconds × 1000` (default 60 s, capped at 3600 s; `0` disables it). It is stored, so it never changes. The next run is `cron.next_after(now − jitter) + jitter`, which is always in the future. `list` and `show` print it.

**CLI** (`cyber loops`, through the background server like `cyber teams`):

```
cyber loops add --cron "<expr>" [--name N] [--dir P] [--agent A] [--mode M] [--new-session|--session ID]
                [--notify always|on_change|never] [--missed skip|run-once]
                [--max-cost USD] [--max-tokens N] [--max-turns N] [--timeout DUR] "<prompt>"
cyber loops list | show <id> | pause <id> | resume <id> | run <id> | rm <id>
```

- **When the server is down:** `add` warns, when no healthy server was registered before it ran, that schedules run only while the Cyber service runs and that `cyber service start` starts it.
  - This replaces the spec's `cyber service install`, which does not exist: the service is the registered `cyber serve`, started on demand.
- **`run`:** runs now, outside the cron schedule, through the same gate.
- **Inside an agent subprocess:** `add`, `resume` and `run` are refused when `CYBER_SESSION_ID` is set, as `cyber features` does for `computer_use`. The model cannot create or wake a persistent schedule.
  - The routes already require the owner credential, which model subprocesses never receive.

**HTTP.**

- `GET|POST /api/v1/schedules`
- `GET|DELETE /api/v1/schedules/{id}`
- `POST /api/v1/schedules/{id}/pause|resume|run`

They are not in the IDE token scope. The error types are `ScheduleNotFoundError` and `InvalidRequestError`.

### 7. The unattended gate

Every scheduled run checks, at creation (so that `add` fails early) and again at run time, because config, trust and checkouts change:

1. **The directory:**
   - it must exist and be a directory, and its configuration must load (a load error refuses);
   - its checkout must be trusted (`TrustReport.trusted`: nothing to approve, or the approved digest). Otherwise: `Refused: <dir> is not trusted; run cyber trust inspect there`.
2. **`bypass`:** the Mode may be `bypass` only when `loops.allow_bypass` is `true` and the setting came from a `global:` or `managed:` layer (`Resolved.sources["/loops/allow_bypass"]`).
   - A project, local, environment, profile or `--config` layer cannot enable it, whether or not the checkout is trusted.
   - The server's own `bypass` gate (`admit_mode`: a sandbox or `--dangerously-bypass-permissions`) and org policy still apply on top.
3. **Asks:** in a scheduled run, an `ask` under `dont-ask`, or an `ask` that no approver could answer, is denied with `Denied: no approver in unattended schedule`, and the Drain continues. The tool host uses this message while a `schedi_` input is the Session's current run.

A refused run notifies (unless `notify` is `never`) and does not pause the schedule. The next occurrence tries again.

### 8. Notifications for scheduled runs

- **Rules:** the same rules as decision 5, applied to the run's outcome. A `refused` run counts as a failure.
- **In the Session:** a notification is recorded in the run's Session (`record_notification`), so clients and remote devices see it.
- **On the desktop:** usually no client is attached, so the server also calls a `DesktopNotifier` port. `cyber-app` wires an OS notifier:
  - macOS runs `osascript` with title and body passed as `argv`, never inlined in the script;
  - Linux runs `notify-send`;
  - other platforms use none.
  - It is best effort and never blocks the run. Tests inject a recording notifier.

### 9. Budgets across scheduled runs

- **Where the budget comes from:** the `add` flags, else `Budget::from_config(directory config, "loop")`.
- **What is checked:** before each run, the accumulated `cost_usd`, `tokens` and `turns`, and `created_ms + max_wall_seconds`. After each run, the accumulated totals are checked again.
- **When it is exceeded:** `schedule.updated {budget_exceeded, reason}`, a notification, and no further runs. The budget is fixed at `add`, so `resume` refuses a `budget_exceeded` schedule; the user removes it and adds a new one.
- **Each run's Session:** a new Session also gets the directory's `budgets.session` as usual.

### 10. Usage-limit waits

`unattended_marker` also returns true when the Session's last promoted input is a `schedi_` row. A scheduled run that resumes an attended Session therefore waits for the reset instead of failing. New run Sessions are already `unattended: true`. The schedule's own `max_wall_seconds` is not a deadline of the wait: it is checked between runs.

### 11. Hooks

`LoopIteration` and `ScheduleRun` join the runtime lifecycle events. Both are observational: decisions are ignored and failures are logged.

- **`LoopIteration`** fires after `loop.iteration.ended.1` commits. It receives `{loop: <LoopState>, iteration: {n, outcome, noop, duration_ms, cost, reason}}`.
- **`ScheduleRun`** fires after the run's `ended` phase commits, in the run's Session. It receives `{schedule: {id, name, cron, directory}, run: {run_id, n, outcome, noop, duration_ms, cost, reason}}`.
  - A `refused` run has no Session, so its `ScheduleRun` has none and is not dispatched; its `schedule.run.1` record remains.

## Spec deltas

The spec deltas are in `specs/loops-scheduling/spec.md`:

- **Persistent local schedules:** `cyber service start` replaces `cyber service install`. Management from an agent subprocess is refused.
- **Unattended Mode for schedules:** trusted directories, the re-check at every run, `refused`, and the scope of the deny message.
- **In-session cron tools:** root Sessions only, the `at` bounds, `cron_delete` limited to `cron_` tasks, and `notify`.
- **Restore on resume:** a one-shot task whose time passed under `skip` ends `expired`. This includes the restore rules of `implement-m2-4-loops`.
- **Completion notifications:** `schedule_wakeup` during scheduled runs, `/loop --notify`, and the rate limit.

## Risks / Trade-offs

- **[The model schedules work in its own Session]** That work is bounded:
  - by the shared cap of 50 and `loops.max_iterations`;
  - by the Session's Mode and budget, which still apply to every Turn;
  - by `/loop stop`, which the user can always use.
- **[A schedule configured with `bypass`]** It needs a user- or managed-layer opt-in and the server's own `bypass` gate, both re-checked at every run.
- **[A checkout edited between runs]** The trust digest changes and the next run is refused until the user approves it again.
- **[Desktop notifications spawn a process]** Only for scheduled runs, with arguments passed as `argv` and a 5 s timeout.
- **[The scheduler lives in `cyber serve`]** Schedules run only while it runs. `add` says so. A launchd or systemd unit is future work.
