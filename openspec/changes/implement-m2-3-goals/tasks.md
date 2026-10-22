## Slice 1 status (PR "Add Session goals")

Slice 1 delivers sections 2–8 except where noted below. Slice 2 covers section 9 and the open items:

- 3.1: use-template.
- 3.4: the model-facing `goal` tool. User routes allow `goal.set` today.
- 5.5 and its test: unattended asks. No per-Session client presence signal exists yet.
- 6.2: a `GoalCompleted` hook test. The producers are implemented.
- 6.4: the hook and export tests. The restart test runs on the runtime.
- 7.1: the `/goal` panel ships as a picker with totals; panel keys are still to do.
- 7.3: a sidebar Goal section; verdict lines and the footer ship.
- 7.4: snapshots and the PTY capture.
- 8.4: the SIGINT test; the other exit codes are tested.
- 10.2 and 10.3.

Implementation deltas are recorded in `specs/goals/spec.md`:
- the evaluator falls back to the Session's model;
- any failed or user-stopped Turn pauses the Goal, but a shutdown does not;
- checks run like `!` commands, without a permission ask;
- evaluator cost is recorded as `usage.recorded.1` with purpose `goal_evaluator`;
- with `--goal`, `--max-turns` sets each Goal's turn budget.

## 1. OpenSpec change

- [x] 1.1 Map every goals requirement and the related tui, server-api, exec-mode, hooks, agents-subagents and workflows requirements to sections (`proposal.md`).
- [x] 1.2 Record storage on the Session stream, the drain integration point, the turn definition and exec ask behavior (`design.md`).
- [x] 1.3 Spec deltas: management events, goal events, the turn definition, local notifications, restore scope, fork listing, exec asks.
- [x] 1.4 `openspec validate --all --strict` passes.

## 2. Model, config, events and storage

- [x] 2.1 `goals` config schema and validation:
  - `max_queue` (20, range 1–200)
  - `check_timeout_seconds` (600, range 1–86400)
  - `on_impossible` (`stop`|`continue`)
  - `fork_isolation` (`worktree`|`none`)
  - `unattended_ask_seconds` (300)
  - `evaluator_max_input_tokens` (8000, range 1000–200000)
  - `templates.<name> {condition, check?, budget?}`
  - `budgets.goal.max_turns` defaults to 50
- [x] 2.2 Store migration for the `goal` table and index; register the `session.goal.*.1` event kinds, including `edited` and `reordered`, and their projection.
- [x] 2.3 `Goal` domain type with `gol_` IDs and status transitions as a pure state machine with unit tests (illegal transitions refused).
- [x] 2.4 Condition length check with the exact spec message; queue cap with `GoalQueueFull`.
- [x] 2.5 Production-path test: setting and listing goals through the App HTTP router persists across an App rebuild.

## 3. Goal service and HTTP routes

- [x] 3.1 `runtime/goals.rs` service: set (clears the existing goal), add, edit, pause, resume, clear `id|all`, skip/next, reorder and use-template. Each publishes its event.
- [x] 3.2 Set and activate admit the condition as a steer prompt with source `goal`, so work starts at once.
- [x] 3.3 Routes under `/api/v1/sessions/{id}/goals`, with the errors from design §9. Update OpenAPI and regenerate the SDK (`client.goals`). Add `goal` to `/health` `features`.
- [x] 3.4 `goal.set` permission: the user's routes allow it. A model `goal` tool asks by default.
- [x] 3.5 Tests: `set_goal_starts_a_turn_toward_it`, `add_goal_activates_when_none_active`, `reorder_via_patch_publishes_reordered`, `pause_stops_continuation_and_keeps_history`, `condition_over_4000_chars_rejected_with_message`.

## 4. Evaluator loop and context

- [x] 4.1 Drain integration: `goals.after_turn` at the idle exit after Stop hooks (design §3); a `true` result continues the Drain.
- [x] 4.2 Evaluator call on the `evaluator` profile and role, using structured output `{verdict, reason}`. Input is capped by `evaluator_max_input_tokens`. A malformed reply is retried once, then becomes `not_met`.
- [x] 4.3 `not_met` admits a continuation steer prompt with the condition and reason. `met` completes the goal and activates the next queued one at the Safe Boundary, or goes idle.
- [x] 4.4 `core/goal` System Context source: active goal, last verdict and queued count.
- [x] 4.5 Evaluator cost is recorded as hidden `evaluator` messages and counted in the Session cost and `eval_*` totals.
- [x] 4.6 User prompts during a goal are promoted as steer input and leave the goal active.
- [x] 4.7 Production-path tests with a scripted provider: `not_met_continues_with_reason_then_met_completes`, `queue_runs_a_b_c_without_user_input`, `user_hint_keeps_goal_active`, `model_sees_goal_system_message`, `evaluator_cost_shown_separately`.

## 5. Checks, budgets, stops and safety

- [x] 5.1 Check command through the bash host path: sandbox, `bash` permission and timeout. A non-zero exit becomes `not_met` with the last 50 lines.
- [x] 5.2 Goal Budget from activation, counting goal Turns and rolling up child tokens and cost. Exceeding it sets `budget_exceeded` and notifies with the spend figures.
- [x] 5.3 `impossible` stops the goal, and the queue stops unless `on_impossible = continue`. Blocking `TurnEnd::Failed` kinds pause the goal with the reason.
- [x] 5.4 Deferred evaluation while Session-started background Jobs, background subagents or workflow runs are running (`pending_eval`).
- [x] 5.5 Unattended asks: with no subscriber for `unattended_ask_seconds`, the goal pauses with `Waiting for approval` and a notification is sent; the request stays pending.
- [x] 5.6 Tests: `failing_check_turns_met_into_not_met_with_tail`, `goal_turn_budget_exceeded_at_50`, `auth_error_pauses_goal`, `impossible_stops_queue_by_default`, `background_job_defers_evaluation_until_idle`, `unattended_ask_pauses_goal_waiting_for_approval`, `goal_never_raises_mode`.

## 6. Durability, hooks and notifications

- [x] 6.1 On replay or resume, an active goal is marked for evaluation. The first pass evaluates once before continuing, and an evaluation-only pass runs when no Drain is running.
- [x] 6.2 `GoalEvaluated` and `GoalCompleted` hook producers (observational, JSON on stdin).
- [x] 6.3 Completion, pause and budget notices through the notify path (TUI toast, desktop notification, event).
- [x] 6.4 Tests: `restart_mid_goal_evaluates_before_continuing` (a real server restart), `goal_completed_hook_receives_goal_json`, and Session export including goals.

## 7. TUI

- [ ] 7.1 `/goal` panel: active goal, queue, last verdict and reason, turns and cost against the budget, evaluator totals, and the keys e/p/r/c/↑↓.
- [x] 7.2 `/goal` subcommands and `--check`; add them to the slash catalog (`BUILTIN_COMMANDS` guard test).
- [ ] 7.3 One verdict status line per evaluation; `◎ goal <condition>` in the footer; a Goal section in the sidebar.
- [ ] 7.4 Insta snapshots (panel, footer, sidebar, light theme) and a pty acceptance capture.

## 8. `cyber exec --goal`

- [x] 8.1 `--goal` (repeatable), `--goal-check` (binds to the previous `--goal`) and `--max-turns`.
- [x] 8.2 Exit codes 0, 3, 4, 1 and 130 per design §9. The JSON result gains `goals[]`.
- [x] 8.3 Under exec, asks are declined immediately, and a declined ask doesn't pause the goal.
- [ ] 8.4 CLI tests: `exec_goal_met_exits_zero_after_check_passes`, `exec_goal_impossible_exits_3`, `exec_goal_budget_exits_4`, `exec_goal_queue_exit_reflects_first_non_met`, `exec_goal_sigint_exits_130`.

## 9. Goals on agents and workflows; fork and templates

- [x] 9.1 The `agent` tool `goal` option: the child's active goal; the call settles on its terminal status, with `goal_status`.
- [x] 9.2 Workflow `agent()` `goal`: remove it from `UNAVAILABLE_OPTIONS`; the goal budget is capped by the run's remaining budget; the result includes `goal_status`.
- [x] 9.3 `/goal fork` and `POST .../goals/fork`: a background Session in a new managed worktree (`fork_isolation`), listed in `cyber sessions`.
- [x] 9.4 `/goal use <template> [text]` with `$ARGUMENTS` substitution; an unknown template returns `GoalTemplateNotFound`.
- [x] 9.5 Tests: `workflow_agent_with_goal_runs_until_check_passes`, `agent_tool_goal_returns_goal_status`, `goal_fork_starts_background_session_in_worktree`, `goal_use_template_substitutes_arguments`.

## 10. Documentation and acceptance

- [ ] 10.1 Docs: README feature list, `docs/features.md`, a goals guide (conditions, checks, budgets, exec in CI) and ROADMAP M2.3 status.
- [ ] 10.2 End-to-end acceptance on the real binary with a scripted provider:
  - a goal queue of three
  - a failing then passing check
  - a budget stop
  - restart mid-goal
  - `exec --goal` exit codes
  - a workflow agent with a goal
  - TUI captures as PNG
- [ ] 10.3 Archive the change once all sections are green.
