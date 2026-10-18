## Why

ROADMAP P2 milestone M2.3 ("Goals: active goal + goal queue, evaluator verdicts, `check` commands, budgets, `exec --goal` exit codes") has no implementation. Only placeholders exist:
- `goal` is a reserved command name (`crates/cyber-core/src/commands.rs`).
- The `goals` config key is accepted opaquely (`config/validate.rs`).
- `budgets.goal` is a valid Budget scope (`cyber-core/src/budget.rs`).
- The `GoalEvaluated` and `GoalCompleted` hook events are accepted but have no producer.
- The `evaluator` model role and agent profile exist.
- Workflow `agent()` rejects the `goal` option as "not available in this build" (`runtime/workflow_host.rs`).
- The TUI footer has a `goal` slot with no source.

The `goals` spec defines 21 P2 requirements. A Goal turns a Session into unattended, checkable work: the Session keeps going until an evaluator, and optionally a shell `check`, agree that the condition holds, or until a budget or a blocking error stops it. Codex and Claude Code each offer one goal per session. Cyber Code adds a queue, verification commands, budgets and goals on subagents and workflow agents.

As in M2.1, every increment ships at least one test through a production path: the real `cyber` binary, the App HTTP router, or `cyber exec`.

## What Changes

1. **Model, config, events and storage.**
   - A `gol_` Goal record stored on the Session event stream as `session.goal.*.1` events and projected into a `goal` table (migration `2026102x_goals.sql`).
   - Validated `goals` config: `max_queue`, `check_timeout_seconds`, `on_impossible`, `fork_isolation`, `unattended_ask_seconds`, `evaluator_max_input_tokens`, `templates`.
   - `budgets.goal` defaults (`max_turns` 50).
2. **Goal service and HTTP routes.** Set, add, list, edit, pause, resume, clear, skip, reorder and use-template under `/api/v1/sessions/{id}/goals`, with OpenAPI and the generated SDK `client.goals`. Setting a goal checks `goal.set`: allowed for the user, `ask` for the model. `goal` joins `/health` `features`.
3. **Evaluator loop.**
   - It runs at the Drain's idle point, after Stop hooks, when the Session would otherwise go idle.
   - It records a structured verdict and admits a steer continuation on `not_met`.
   - It publishes `core/goal` as a System Context source and accounts evaluator cost separately.
4. **Checks, budgets and stops.**
   - The sandboxed `check` command, with a timeout and the last 50 lines on failure.
   - Goal budgets counted from activation, including child costs.
   - `impossible` and blocking errors.
   - Deferred evaluation while background work runs.
   - Unattended approval pauses the goal.
5. **Durability, hooks and notifications.**
   - Restore on resume and restart, with one evaluation before continuing.
   - `GoalEvaluated` and `GoalCompleted` hooks.
   - Completion and pause notices through the existing notify path.
6. **TUI.** `/goal` and its subcommands, the goal panel, `◎ goal` in the footer, one verdict status line per evaluation, and a Goal section in the sidebar.
7. **`cyber exec --goal`.** Repeatable `--goal`, `--goal-check` and `--max-turns`, with exit codes 0 (met), 3 (impossible), 4 (budget exceeded), 1 (other) and 130 (interrupted). The JSON result gains `goals[]`.
8. **Goals on agents and workflows; fork and templates.**
   - The `agent` tool's and workflow `agent()`'s `goal` option, which resolves only at the child Goal's terminal status, with `goal_status`.
   - `/goal fork` into a background Session (in a worktree by default).
   - `/goal use <template>` with `$ARGUMENTS`.

## Spec corrections (deltas)

- **goals / Goal management commands:** reorder and edit publish `session.goal.reordered.1` and `session.goal.edited.1`. The spec already says "an event announces the new order" but names none.
- **goals / Goal events:** adds those two events.
- **goals / Goal budgets:** defines a goal Turn. It is one Drain pass that ends with a model answer and reaches evaluation, not each model request inside a tool loop, which the agent `steps` ceiling still bounds.
- **goals / Completion:** notifications go through the local notify path (TUI toast, desktop notification, event). Remote push arrives with remote-control (P3).
- **goals / Goals survive resume and restart:** handoff between machines is out until a cross-machine handoff exists.
- **goals / Parallel goals across sessions:** the forked Session is listed in `cyber sessions` and the TUI session list. The agent view listing arrives with M2.5.
- **goals / Non-interactive goals and Goal mode safety:** `cyber exec` has no attached approver and already declines asks immediately. Under `exec --goal` a declined ask is a tool rejection the agent can work around, not a pause. A Goal that pauses for any other reason ends exec with exit 1 and the reason.

## Requirement mapping

| Requirement | Section |
|---|---|
| Goal model | 2 |
| Set the active goal | 3 |
| Goal queue | 3 |
| Goal management commands | 3, 7 |
| Evaluation after each turn | 4 |
| Continuation on not met | 4 |
| Completion | 4, 6 |
| Impossible and blocking errors | 5 |
| Verification check command | 5 |
| Goal budgets | 5 |
| Deferred evaluation during background work | 5 |
| Goals survive resume and restart | 6 |
| Goal context source | 4 |
| User input during a goal | 4 |
| Parallel goals across sessions | 9 |
| Goals on workflow agents and subagents | 9 |
| Goal templates | 9 |
| Non-interactive goals | 8 |
| Goal events | 2, 6 |
| Evaluator cost accounting | 4 |
| Goal mode safety | 5 |
| tui / Goal and loop indicators | 7 |
| server-api / Health and version (`goal` feature) | 3 |

## Impact

- **Crates:** cyber-core (config, budget defaults), cyber-store (migration), cyber-server (runtime goals, drain hook point, routes), cyber-tools (`agent` tool option), cyber-app (services), cyber-cli (`exec --goal`), cyber-tui (`/goal`, panel, footer, sidebar), and the SDK (generated).
- **No new dependencies.**
- **Out of scope:** `--until-goal` loops (M2.4), agent-view listing (M2.5), remote push (P3), and handoff between machines.
