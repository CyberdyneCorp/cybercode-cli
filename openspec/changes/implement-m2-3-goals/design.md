## Context

This design targets the M2.3 requirements in `openspec/specs/goals/spec.md` (21 P2 requirements) and the related ones in tui, server-api, exec-mode, hooks, agents-subagents and workflows. It builds on code that already exists:

- **The Drain** (`crates/cyber-server/src/runtime/drain.rs`). It loops over passes. When nothing is eligible and the last pass answered, it runs Stop hooks (`lifecycle.rs::stop_hooks`), which may admit a continuation. Otherwise it returns and the Session goes idle. Goals hook in at exactly this point.
- **Budgets** (`cyber-core/src/budget.rs`, `runtime/budget.rs`). `Budget { max_turns, max_tokens, max_cost_usd, max_wall_seconds, enforcement }` with soft dispatch boundaries and `budget.activated/warned/exceeded.1` events. `goal` is already a valid scope.
- **Model roles.** `evaluator` resolves through `model_roles.evaluator`, falling back to `small`. The `evaluator` hidden agent profile exists.
- **Structured output** (`runtime/structured.rs`) for schema-validated replies.
- **System Context sources** (`core/memory`, `core/skills` and others). Changes arrive as Mid-Conversation System Messages at a Safe Boundary.
- **Process Jobs and background work** (batch B: `process_jobs.rs`, `jobs.rs`), workflow runs and background subagents. All of them can be asked "is anything the Session started still running?".
- **Notify** (batch B `notify` tool and toasts) and the hooks runner. `GoalEvaluated` and `GoalCompleted` are accepted names with no producer.
- **`cyber exec`** exit codes `1/4/5/130` (`commands/exec/mod.rs`). Exit 3 is free.

## Goals / Non-Goals

**Goals:** every goals requirement except the parts named below, each proven through a production path.

**Non-Goals:**
- `--until-goal` loops (loops-scheduling, M2.4).
- Listing in the agent view (M2.5).
- Remote push (remote-control, P3).
- Session handoff between machines.
- An editor for goal templates (they come from config).

## Decisions

### 1. Storage: Session-stream events plus a projection

Goal events live on the Session aggregate (`session.goal.*.1`). They commit atomically with the Turn they follow, and Session export and deletion already cover them. A `goal` projection table holds current state:

```sql
CREATE TABLE goal (
  goal_id TEXT PRIMARY KEY,           -- gol_<ulid>
  session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
  position INTEGER NOT NULL,          -- 0 = active slot, 1.. = queue order
  condition TEXT NOT NULL,            -- 1..4000 chars
  check_cmd TEXT,
  budget TEXT,                        -- JSON Budget, defaults applied at set time
  template TEXT,
  status TEXT NOT NULL,               -- pending|active|paused|met|impossible|cleared|budget_exceeded
  reason TEXT,                        -- last verdict or pause reason (<= 300 chars, check tail separate)
  turns INTEGER NOT NULL DEFAULT 0,
  tokens INTEGER NOT NULL DEFAULT 0, cost_usd REAL NOT NULL DEFAULT 0,
  eval_count INTEGER NOT NULL DEFAULT 0,
  eval_tokens INTEGER NOT NULL DEFAULT 0, eval_cost_usd REAL NOT NULL DEFAULT 0,
  pending_eval INTEGER NOT NULL DEFAULT 0,   -- 1 = evaluate before continuing (restore, deferred)
  created_ms INTEGER NOT NULL, activated_ms INTEGER, ended_ms INTEGER
);
CREATE INDEX goal_session ON goal(session_id, status, position);
```

Evaluation history is the event stream: `session.goal.evaluated.1` carries `{goal_id, verdict, reason, check?: {command, exit_code, tail}, eval_tokens, eval_cost_usd, turn}`. `GET .../goals/{id}` returns the last 50 evaluations from the event index.

The storage-events spec lists Goal among the aggregates. Here a Goal is addressed by `gol_` ID through the projection, while its events ride the Session stream, as the goals spec's event names require.

### 2. Lifecycle and the queue

The statuses are `pending → active → (paused ↔ active) → met | impossible | cleared | budget_exceeded`. At most one Goal per Session is `active` or `paused`; the rest are `pending`, ordered by `position`.

- `set` clears the current one (`cleared`), activates the new Goal and admits its condition as a steer prompt with source `goal`.
- `add` appends, or activates when no Goal is active or paused.
- `skip`/`next` clears the active Goal and activates the next.
- A queue longer than `goals.max_queue` is refused with `GoalQueueFull`.
- Activating the next Goal after `met` happens at the next Safe Boundary. The service admits the condition prompt, and the prompt is promoted at the boundary.
- After `impossible`, the queue stops unless `goals.on_impossible = "continue"`.

### 3. Evaluator loop (drain integration)

`drain` gains one step at its idle exit, after Stop hooks:

```text
if !eligible(...) {
    if answered && stop_hooks(...)? { continue }            // existing
    if answered && goals.after_turn(handle, cancel)? { continue }  // new
    return Ok(())
}
```

`Goals::after_turn` does nothing when no Goal is active or the active Goal is paused. Otherwise it runs these steps in order:

1. **Deferred work.** If background Jobs, background subagents or workflow runs started by this Session are still running, set `pending_eval = 1` and return `false`. Their handbacks arrive as queued input and start a later Drain, whose idle point evaluates.
2. **Budget.** Count the Turn (`turns += 1`) and roll up tokens and cost, including `children_*` deltas since activation. Then check the Goal's Budget. If it's exceeded, set `budget_exceeded`, notify with the spend figures and return `false`.
3. **Evaluate.** Run the `evaluator` profile on `model_roles.evaluator` with structured output `{verdict: met|not_met|impossible, reason: string<=300}`.
   - The input is the condition, the latest assistant text, a summary of this Turn's tool results (names, status and a truncated output head), and earlier verdicts.
   - The input is capped at `goals.evaluator_max_input_tokens`. Older material is dropped first, then the tool summaries are truncated.
   - The call is a hidden assistant message of the `evaluator` agent, counted in the Session's cost and in `eval_*`.
   - A malformed reply is retried once, then treated as `not_met` with reason `evaluator returned an invalid verdict`.
4. **Check.** On `met` with a `check`, run it (decision 4). A non-zero exit turns the verdict into `not_met`, with the last 50 lines as the check tail.
5. **Act.**
   - `not_met`: append `evaluated` and admit a continuation steer prompt (source `goal`) containing the condition and the reason or check tail. Return `true` so the Drain continues.
   - `met`: append `evaluated` and `completed`, set `met`, notify, and activate the next queued Goal, or go idle.
   - `impossible`: append `evaluated` and `ended {status: impossible}`, notify, and stop.

The evaluator is never shown the system prompt or tools. It sees text only, so it cannot act.

User prompts during a Goal are promoted as usual and do not touch the Goal. Evaluation simply happens after the Turn that processed them.

### 4. Check command

The check runs through the same host path as the `bash` tool: the Session's Location, the effective sandbox profile, and the `bash` permission.
- An `ask` while a client is attached goes through the normal request.
- With no client attached, decision 6 applies.
- A denied permission makes the verdict `not_met` with reason `check not permitted: <rule>`.

The timeout is `goals.check_timeout_seconds` (default 600). On timeout the process tree is killed and the reason is `check timed out after Ns`. Output is captured with the Jobs log cap, and the last 50 lines go into the reason.

### 5. Blocking errors

`TurnEnd::Failed { kind }` with a kind the user must fix pauses the active Goal with that reason: `auth`, `model_unavailable`, `disk_full`, `permission_rejected` without feedback, or `ContextOverflow` after compaction failed. Continuation stops and a notice is sent. Transient provider errors keep the existing retry behavior and are not goal pauses.

### 6. Mode safety and unattended asks

A Goal never changes the Mode.

When a goal-driven Turn raises a permission request and no client has been connected for `goals.unattended_ask_seconds` (default 300), the request stays pending and the Goal pauses with `Waiting for approval`. A notification names the request. The client-presence signal is the existing SSE subscriber count per Session.

Under `cyber exec` there is no approver, and exec already declines requests at once. The agent sees a rejection and may work around it, so the Goal does not pause; this is the spec delta.

### 7. Restore

On server start (replay) and `cyber --resume`, an `active` Goal is marked `pending_eval = 1`. The first Drain of that Session evaluates once on the last answered Turn before admitting any continuation. If the Session has no running Drain, the restore path starts an evaluation-only pass. It runs no model work Turn, only the evaluator, and continues only on `not_met`.

### 8. Context source `core/goal`

A typed source renders the following, and changes arrive as Mid-Conversation System Messages:

```text
Active goal (gol_…): <condition>
Last verdict: not_met — <reason>
Queued goals: N
```

When no Goal is active, the source is empty and contributes nothing.

### 9. Surfaces

**HTTP** (`/api/v1/sessions/{id}/goals`):
- `GET` returns the active Goal, the queue and totals.
- `POST` with `{condition, check?, budget?, mode: set|add, template?, args?}`.
- `GET|PATCH|DELETE /{goal_id}`: edit the condition, check or budget; `DELETE` clears.
- `POST /{goal_id}/pause|resume`, `POST /skip`, `PATCH /order {goal_ids:[...]}` (publishes `reordered`) and `POST /fork {condition, check?, budget?}`.
- Errors: `GoalNotFound`, `GoalConditionTooLong`, `GoalQueueFull` and `GoalTemplateNotFound`.

**Permission:** the user's routes allow `goal.set`. The model's `goal` tool, a small tool exposed only when `goals` is enabled, uses `goal.set` with an `ask` default.

**TUI:**
- `/goal` opens the panel: active Goal, queue, last verdict, Turns and cost against budget, evaluator totals, and the keys e/p/r/c for edit, pause, resume and clear. Plain ↑/↓ move the selection; Shift+↑/↓ move the selected queued Goal (`PATCH .../order`, then the panel reopens).
- `/goal <text>`, `/goal add|list|edit|pause|resume|clear|skip|next|fork|use` and `/goal --check "<cmd>" <text>`.
- Each evaluation shows one status line, `◎ not met: 3 tests failing…`.
- The footer shows `◎ goal <truncated condition>`.
- The sidebar has a Goal section.

**exec:** `--goal` (repeatable, forming a queue in order), `--goal-check` (applies to the most recent `--goal`) and `--max-turns` (budget `max_turns` for every goal). It runs until every Goal is terminal, or until the first non-met one, which stops the queue. Exit codes are listed below, and the JSON result adds `goals: [{id, condition, status, turns, cost_usd, last_reason}]`.

| Outcome | Exit |
|---|---|
| All Goals met | 0 |
| First non-met Goal is `impossible` | 3 |
| First non-met Goal is `budget_exceeded` (or exec `--timeout`) | 4 |
| Paused (blocking error) or other error | 1 |
| SIGINT | 130 |

**Agents and workflows:**
- The `agent` tool and workflow `agent()` accept `goal: {condition, check?, budget?}`. The child Session gets it as its active Goal, and the call settles when that Goal is terminal, returning `goal_status`.
- `goal` is removed from `UNAVAILABLE_OPTIONS`.
- A workflow agent's goal budget is capped by the run's remaining budget.

**Fork and templates:**
- `/goal fork` creates a background root Session in a new managed worktree (`goals.fork_isolation`, default `worktree`) with the condition as its active Goal. The new Session records the source Session's directory as `source_directory` (a `session` column, also set for worktree teammates), and a directory-filtered listing matches root Sessions by it, so `cyber sessions list` in the source checkout shows the fork.
- `/goal use <name> [text]` resolves `goals.templates.<name>`, substituting `$ARGUMENTS`.

### 10. Events and hooks

The events are `session.goal.set.1`, `queued.1`, `activated.1`, `evaluated.1`, `paused.1`, `resumed.1`, `completed.1`, `ended.1` (with terminal status), and the new `edited.1` and `reordered.1`.

The `GoalEvaluated` hook gets `{goal, verdict, reason}` on stdin. `GoalCompleted` gets the Goal JSON. Both are observational: their decisions are ignored and failures are logged.

## Risks / Trade-offs

- **Evaluator false positives:** mitigated by `check`, which the docs recommend for anything testable.
- **Evaluator cost:** every idle point costs one small-model call, so it is capped by `evaluator_max_input_tokens` and visible in `/goal`.
- **Runaway continuation:** bounded by `max_turns` (default 50) and the Session's own budgets. Stop hooks run before goals, so a Stop hook can still end the work.
- **Drain complexity:** the integration is one call at the idle exit, and all goal logic lives in a new `runtime/goals.rs` module (plus `goals_eval.rs` and `goals_check.rs`). Each function stays within the backend complexity target.

## Migration

The change adds one migration (the `goal` table) and new events. There are no changes to existing data, and older Sessions simply have no goals.
