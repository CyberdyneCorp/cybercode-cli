## Context

The runtime already provides what loops need:

- **Admission with a source.** `Admission.source` already lists `loop`. `Runtime::admit_message(session, admission, extra)` commits feature events atomically with `session.prompt.admitted.1`, and `Delivery::Queue` promotes one input at a time when the Session would otherwise go idle.
- **An idle hook.** `drain::run` calls `Runtime::drain_ended(id, hops, status)` once a Drain ends. Messaging watches already use it.
- **Durable timers, the messaging-watch way.** A projection table with a due time, a `sweep(now_ms)` that tests drive directly, a startup `recover_*` and a background `spawn_*_sweep` owned by `Inner.background` and cancelled by `Inner.closed`.
- **Budgets and accounting.** `Budget::from_config(config, "loop")` is accepted. Cost and tokens per step come from `session.step.ended.1`, folded into `SessionState.totals`.

## Decisions

### 1. Storage: Session-stream events plus a projection

Loop events live on the Session aggregate. A feature module `runtime/loops/records.rs` registers them with prefix `loop.` and folds them into `SessionState.loops: BTreeMap<LoopId, LoopState>`, the same way `messaging.` is folded.

| Event | Payload |
|---|---|
| `loop.created.1` | `{loop_id, kind: interval\|self_paced, prompt, interval_s?, overlap, missed, stop: {max?, for_s?, until?}, budget?, created_ms, next_due_ms}` |
| `loop.iteration.started.1` | `{loop_id, n, message_id, started_ms, cost_before, tokens_before, turns_before}`, committed in the same transaction as the iteration's `admitted` event |
| `loop.wakeup.1` | `{loop_id, n, delay_s, reason, noop}`, from `schedule_wakeup` (clamped value) |
| `loop.iteration.ended.1` | `{loop_id, n, outcome, noop, duration_ms, cost, tokens, turns, next_due_ms?}` |
| `loop.stopped.1` | `{loop_id, status: stopped\|completed\|max_iterations\|duration\|until_met\|budget_exceeded\|failed, reason, iterations}` |

The `session_loop` table (projected in the append transaction) holds the current state the sweep and the API read:

```sql
CREATE TABLE session_loop (
  loop_id     TEXT PRIMARY KEY,               -- lop_<ulid>
  session_id  TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
  kind        TEXT NOT NULL,                  -- interval | self_paced
  prompt      TEXT NOT NULL,
  spec        TEXT NOT NULL,                  -- JSON: interval_s, overlap, missed, stop, budget
  status      TEXT NOT NULL,                  -- active | stopped | completed | ... (loop.stopped status)
  next_due_ms INTEGER,                        -- NULL while an iteration runs (self-paced) or when ended
  inflight    TEXT,                           -- message_id of the running iteration
  coalesced   INTEGER NOT NULL DEFAULT 0,     -- one due tick waiting behind the running iteration
  iterations  INTEGER NOT NULL DEFAULT 0,
  failures    INTEGER NOT NULL DEFAULT 0,     -- consecutive failed iterations
  noop_streak INTEGER NOT NULL DEFAULT 0,
  last_noop   INTEGER,
  cost_usd    REAL NOT NULL DEFAULT 0, tokens INTEGER NOT NULL DEFAULT 0, turns INTEGER NOT NULL DEFAULT 0,
  created_ms  INTEGER NOT NULL, ended_ms INTEGER, reason TEXT
);
CREATE INDEX session_loop_due ON session_loop(status, next_due_ms);
```

Session export, deletion and fork use the event stream, so they cover loops with no extra work. A fork does not copy active loops: forked loops would run twice.

### 2. Lifecycle

**Create** (`POST .../loops`, or `/loop` in the TUI):
- validate the request, enforce `loops.max_per_session` (default 50), and append `loop.created`;
- an interval loop has `next_due_ms = now`, so it runs immediately;
- a self-paced loop also runs immediately; afterwards the model sets the pace.

**Sweep** (`Loops::sweep(now_ms)`, every 5 s, plus a nudge right after create and after each iteration ends). For each active loop with `next_due_ms <= now`:
1. **Stop conditions first:**
   - `iterations >= max` (or `loops.max_iterations`, default 500) → `max_iterations`;
   - `now >= created + for` → `duration`;
   - the budget is exceeded → `budget_exceeded`.
2. **Overlap.** If an iteration is still running (`inflight` set):
   - `skip` appends `loop.iteration.ended {outcome: skipped_overlap}` without counting an iteration, and moves `next_due_ms` to the next tick;
   - `coalesce` sets `coalesced = 1`, and the waiting tick runs as soon as the iteration ends.
3. **Until.** With `--until`, run the check (decision 4). Exit 0 → `loop.stopped {until_met}`, with no iteration.
4. **Admit.** Call `admit_message(session, Admission{message_id: "loopi_<loop>_<n>", source: "loop", delivery: Queue, resume: true, text: prompt}, [loop.iteration.started])`.
   - The deterministic message ID makes a retry after a crash idempotent.
   - A Session that is running a user Turn simply queues the iteration behind it.

**Iteration end** (`Loops::drain_ended(session, status)`, called next to the messaging hook). For each loop whose `inflight` message is promoted, and the Session has nothing else promotable from that loop:
- **outcome:** `failed` when the Drain status is `failed: …`, `interrupted` when it was interrupted, otherwise `completed`;
- **noop:** the `loop.wakeup` of iteration `n` if one exists, else `false`;
- **cost, tokens, turns:** the current totals minus `cost_before`, `tokens_before` and `turns_before`;
- **next_due_ms:**
  - interval: the next multiple of `interval_s` after `started_ms` that lies in the future, so a slow iteration skips ticks instead of drifting;
  - self-paced: `now + delay_s` from the wakeup;
  - self-paced without a wakeup: none, and the loop stops `completed`;
- a coalesced tick runs now;
- 3 consecutive `failed` outcomes stop the loop with `failed`.

User prompts during an iteration are promoted as usual. The iteration ends when the Session next goes idle, so its cost includes the user's Turns. That is accepted and documented.

### 3. `schedule_wakeup`

- **Input:** `{delay_seconds: integer, reason: string (≤ 200 chars), noop: boolean}`.
- **When offered:** only when the Session has an active loop with an in-flight iteration (`BuiltinHost::definitions` asks the runtime through `TurnContext.session_id`). Its retry safety is `Idempotent`, it needs no permission, and plan mode offers it.
- **Effect:** `Runtime::loop_wakeup(session, call)` clamps the delay to 60–3600 s and appends `loop.wakeup` for the in-flight iteration. The last call in an iteration wins. It returns `Next iteration in <N>s` (self-paced) or `Recorded` (interval).
- **Guidance:** a Context Source (`core/loop`) appears only while an iteration runs. It tells the model it is in iteration `n` of a loop, and to call `schedule_wakeup` (self-paced: required to continue) with `noop: true` when nothing changed.

### 4. `--until` check

- The command is typed by the user in `/loop`, so it runs like a user `!` command: sandboxed in the Session's Location, never asking for permission.
- `cyber-tools` gains `ToolHost::check(directory, session, command, timeout) -> CheckOutcome {exit_code: Option<i32>, timed_out: bool, tail: String}`. It shares `run_process` with `bash::run_user`, so the sandbox and the process-tree kill are the same.
- The timeout is `loops.until_timeout_seconds` (default 60). A timeout or a non-zero exit continues the loop, and the tail goes into the next iteration's `started` record for display.

### 5. Budgets

The budget comes from the request flags, else `Budget::from_config(config, "loop")`. It is checked in the sweep before each iteration, against the loop's accumulated `cost_usd`, `tokens` and `turns`, and against `created_ms + max_wall_seconds`.

An exceeded budget stops the loop with `loop.stopped {budget_exceeded, reason: "<limit> <spent>/<max>"}`. An iteration that is already running is never cut off; the check happens between iterations. That makes this a soft limit, which matches `enforcement: soft`.

### 6. Restore

`recover_loops()` runs at startup after `recover_messages`. For each active loop:
- **Iteration in flight:** if `inflight` is promoted and the Session isn't running, the iteration ends with outcome `interrupted`. If it isn't promoted yet, it stays queued and will run.
- **Missed ticks:** `next_due_ms` in the past by more than one interval:
  - `skip`: move it to the next future tick;
  - `run-once`: leave it due, so exactly one catch-up runs, then the normal schedule resumes.
- **Self-paced:** a passed wakeup runs once, under either policy.

`spawn_loop_sweep()` then starts the periodic sweep.

### 7. Surfaces

**`/loop` grammar** (parsed in `cyber-core::loops::parse`, shared by the TUI and the server):

```
/loop [<interval>] [--max N] [--for <dur>] [--until "<cmd>"] [--overlap skip|coalesce]
      [--missed skip|run-once] [--max-cost USD] [--max-tokens N] [--max-turns N] [--timeout <dur>] <prompt or /command>
/loop list
/loop stop [<loop_id>|all]
```

- A leading token matching `^\d+[smhd]$` is the interval.
- Durations use the same units.
- Flags are accepted before the prompt only. Everything after the first non-flag token is the prompt, so prompts may contain `--`.
- Errors: `Loop interval must be at least 60s`, `Scheduled task limit reached (50)` (shared limit), `Unknown loop <id>`, and `Loop prompt is empty`.

**HTTP:**
- `GET /sessions/{id}/loops` returns the active and the 20 most recent ended loops, with their last 50 iterations `{n, message_id, outcome, noop, cost, started_ms}`.
- `POST /sessions/{id}/loops` takes the parsed fields (or `{command: "<raw /loop args>"}`) and returns `201` with the loop.
- `DELETE /sessions/{id}/loops/{loop_id}` stops one loop; `DELETE /sessions/{id}/loops` stops all of them.
- Errors: `LoopNotFoundError`, `LoopLimitError`, `InvalidRequestError`.

**TUI:**
- `/loop …` posts to the server. `/loop list` shows the loops as system lines.
- An iteration's user message (ID prefix `loopi_`) renders as `↻ <prompt> · loop <short id> #n`.
- A run of consecutive iterations whose `noop` is true, each with its assistant reply, collapses into one line `↻ N quiet checks · loop <short id>`.
- The footer shows `↻ loop · next in 4m` while a loop is active.
- `/loop` appears in `/help` and in the palette (**Loop prompt**).

## Risks / Trade-offs

- **[A sweep every 5 s]** It is one indexed query, and it is idle when no loop is active (the background task checks `EXISTS`).
- **[An iteration overlaps the user's own work]** Iterations are queued, never steered, so they never interrupt a Turn, and overlap `skip` keeps the backlog at most one.
- **[A model that never calls `schedule_wakeup`]** A self-paced loop then completes after one iteration, and the guidance source tells the model that is what will happen.
- **[Runaway cost]** This is bounded by `loops.max_iterations` (500), `budgets.loop`, and the Session budget, which still applies to every Turn.
