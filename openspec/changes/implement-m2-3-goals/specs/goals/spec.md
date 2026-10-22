## MODIFIED Requirements

### Requirement: Set the active goal
(P2) `/goal <condition>` SHALL set the active Goal, replacing an existing active Goal (whose status becomes `cleared`), and SHALL immediately admit a prompt with the condition (`delivery: steer`) so work starts without a further message. Setting a goal SHALL require the `goal.set` permission (allowed by default for the user, `ask` when requested by the model). The model requests it through the `goal` tool (`{condition, check?, mode?: "set" | "add"}`), which checks `goal.set` with the condition as its resource; the user's `/goal` commands and HTTP routes never check it.

#### Scenario: Start working toward a goal
- **WHEN** the user runs `/goal all call sites compile against the v2 API and cargo test passes`
- **THEN** the goal becomes active and the agent starts a Turn toward it

#### Scenario: Model asks to set a goal
- **WHEN** the model calls `goal` in a Session with no `goal.set` rule and no attached approver
- **THEN** the call is refused and no Goal is set

### Requirement: Goal templates
(P2) The `goals.templates` config key SHALL define named templates `{ condition, check?, budget? }` usable as `/goal use <name> [extra text]`, with `$ARGUMENTS` substitution in the condition; a condition without `$ARGUMENTS` SHALL get non-empty extra text appended on a new line. `POST /api/v1/sessions/{id}/goals` SHALL accept `template` and `args` for the same result, where a given `check` or `budget` overrides the template's, and an unknown name SHALL fail with `GoalTemplateNotFoundError` (404). The Goal records the template it came from.

#### Scenario: Use template
- **WHEN** config defines `goals.templates.green = { condition: "$ARGUMENTS and all tests pass", check: "just test" }` and the user runs `/goal use green auth module refactored`
- **THEN** the active Goal condition is `auth module refactored and all tests pass` with check `just test`

### Requirement: Goals on workflow agents and subagents
(P2) The `agent` tool and workflow `agent()` calls SHALL accept a `goal` option (`{ condition, check?, budget? }`) that becomes the child Session's active Goal before its task prompt is admitted; the task prompt starts the work, the Goal reaches the child through `core/goal`, and the call SHALL resolve only when the child goes idle with that Goal terminal (or paused), returning `goal_status` (and `goal_reason`) with the result. A resumed child keeps its own Goal, so `goal` with `resume` is refused. A workflow agent's goal budget SHALL be capped by the run's remaining `max_turns`, `max_tokens`, `max_cost_usd` and `max_wall_seconds`.

#### Scenario: Workflow agent with goal
- **WHEN** a workflow calls `agent("Migrate file X", { goal: { condition: "file compiles", check: "cargo check" } })`
- **THEN** the child keeps working until the check passes or the goal ends, and the result includes `goal_status`

### Requirement: Goal management commands
(P2) The system SHALL provide `/goal` (show active Goal, queue and last verdict), `/goal list`, `/goal edit [id]` (opens the condition in the composer or editor), `/goal pause`, `/goal resume`, `/goal clear [id|all]`, `/goal skip` (mark the active Goal `cleared` and activate the next) and `/goal next` (alias of skip). In the TUI `/goal` panel, `e` edits, `p` pauses, `r` resumes and `c` clears the selected Goal, plain ↑/↓ move the selection and Shift+↑/↓ move the selected queued Goal one place. Equivalent HTTP routes SHALL exist under `/api/v1/sessions/{id}/goals`. Editing a Goal SHALL publish `session.goal.edited.1` and reordering the queue SHALL publish `session.goal.reordered.1` with the new order of Goal IDs.

#### Scenario: Pause goal
- **WHEN** the user runs `/goal pause`
- **THEN** evaluation and automatic continuation stop until `/goal resume`, and the Goal keeps its history

#### Scenario: Reorder via API
- **WHEN** a client PATCHes the queue order through the HTTP API
- **THEN** pending Goals are reordered and a `session.goal.reordered.1` event lists the new order

### Requirement: Completion
(P2) On `met` (and a passing `check` when configured), the system SHALL set the Goal to `met`, publish `session.goal.completed.1`, notify the user through the local notification path (TUI notice, desktop notification and the event; remote devices once remote-control is available) and activate the next queued Goal, or return the Session to idle when the queue is empty.

#### Scenario: Notification on completion
- **WHEN** the last queued goal is met while the TUI is attached
- **THEN** a notice reports the Goal as met and the Session goes idle

### Requirement: Goal budgets
(P2) Each Goal SHALL enforce a Budget (`observability-costs`) with `max_turns` defaulting to `budgets.goal.max_turns` (50) and optional `max_tokens`, `max_cost_usd` and `max_wall_seconds`, counted from activation and including subagent costs. A goal Turn SHALL be one Drain pass that ends with a model answer and reaches evaluation; model requests inside a tool loop SHALL NOT each count as a goal Turn (the agent `steps` ceiling bounds those). When exceeded, the Goal SHALL become `budget_exceeded`, continuation SHALL stop and the user SHALL be notified with spend figures.

#### Scenario: Turn budget
- **WHEN** a Goal with default budget reaches 50 Turns without being met
- **THEN** it becomes `budget_exceeded` and the Session stops continuing

#### Scenario: Tool loop counts once
- **WHEN** a goal Turn makes five tool calls before answering
- **THEN** the Goal's turn count increases by one

### Requirement: Goals survive resume and restart
(P2) Active and queued Goals SHALL be persisted with the Session and restored on `cyber --resume` and server restart. On restore, an active Goal SHALL be evaluated once before continuing rather than continuing blindly. Session handoff between machines is out of scope until a cross-machine handoff exists.

#### Scenario: Restart mid-goal
- **WHEN** the server restarts while a Goal is active
- **THEN** after restart the evaluator runs on the last Turn and continuation proceeds only on `not_met`

### Requirement: Parallel goals across sessions
(P2) The system SHALL support pursuing independent Goals in parallel by running them in separate Sessions; `/goal fork <condition>` SHALL create a new background Session (in a new worktree when `goals.fork_isolation` is `worktree`, the default) with the condition as its active Goal, the Session's agent, model and Mode, and list it in `cyber sessions` and the TUI session list of the source checkout (its worktree is `goal-<ulid>` on branch `cyber/goal-<ulid>`, linked to the source Location; without isolation or outside a git checkout it shares the source's Location); the agent view lists it once that view exists. `POST /api/v1/sessions/{id}/goals/fork` SHALL do the same and return the new Session, its directory, its branch and its Goal.

#### Scenario: Fork a goal
- **WHEN** the user runs `/goal fork upgrade all dependencies with green tests`
- **THEN** a new background Session in its own worktree starts working on that Goal while the current Session continues

### Requirement: Non-interactive goals
(P2) `cyber exec --goal "<condition>" [--goal-check <cmd>] [--max-turns N]` SHALL run until the Goal reaches a terminal status and exit with 0 for `met`, 3 for `impossible`, 4 for `budget_exceeded`, 1 for other errors (including a Goal paused by a blocking error, with the reason printed) and 130 when interrupted. `--goal` MAY be repeated to form a queue; `--goal-check` SHALL apply to the preceding `--goal`; with `--goal`, `--max-turns` SHALL set each Goal's budget `max_turns` instead of the Session's step ceiling; the exit code SHALL reflect the first non-met Goal. Because `cyber exec` has no attached approver, permission asks SHALL be declined immediately as tool rejections and SHALL NOT pause the Goal.

#### Scenario: CI goal
- **WHEN** CI runs `cyber exec --goal "lint passes" --goal-check "npm run lint"`
- **THEN** the process exits 0 once the check passes after the evaluator says met

#### Scenario: Impossible goal in CI
- **WHEN** the evaluator returns `impossible` for the only Goal
- **THEN** the process exits 3 and the JSON result lists the Goal with status `impossible`

### Requirement: Goal events
(P2) The system SHALL publish durable events `session.goal.set.1`, `session.goal.queued.1`, `session.goal.activated.1`, `session.goal.evaluated.1`, `session.goal.paused.1`, `session.goal.resumed.1`, `session.goal.edited.1`, `session.goal.reordered.1`, `session.goal.completed.1` and `session.goal.ended.1` (with terminal status), and hooks SHALL be able to subscribe to `GoalEvaluated` and `GoalCompleted`.

#### Scenario: Hook on completion
- **WHEN** a hook is configured for `GoalCompleted` with a command
- **THEN** the command runs with the Goal JSON on stdin when any Goal is met

### Requirement: Goal mode safety
(P2) A Goal SHALL NOT raise the Session's Mode. When a Turn under a Goal hits a permission `ask` in an interactive Session and no client is attached (no client connected for `goals.unattended_ask_seconds`, default 300), the request SHALL remain pending, the Goal SHALL pause with reason `Waiting for approval`, and a notification SHALL be sent. Under `cyber exec` the ask SHALL be declined immediately instead (see Non-interactive goals).

#### Scenario: Unattended approval
- **WHEN** a goal-driven Turn needs approval while the user is away for 5 minutes
- **THEN** the Goal pauses and the user receives a notification with the pending request

### Requirement: Evaluation after each turn
(P2) After each Turn that ends with no pending eligible input, the system SHALL run the evaluator with `model_roles.evaluator`, else `model_roles.small`, else the Session's own model, on the condition, the latest assistant output, a summary of the Turn's tool results and the earlier verdicts, and SHALL record a verdict `met`, `not_met` or `impossible` with a reason of at most 300 characters as a durable `session.goal.evaluated.1` event. The evaluator SHALL be called without tools and SHALL answer one JSON object; a malformed reply SHALL be retried once and then recorded as `not_met` with reason `evaluator returned an invalid verdict`.

#### Scenario: Verdict recorded
- **WHEN** a Turn ends while a Goal is active
- **THEN** a `session.goal.evaluated.1` event with verdict and reason is appended and shown as one status line

#### Scenario: No evaluator configured
- **WHEN** neither `model_roles.evaluator` nor a small model is configured and `cyber exec --model corp/main --goal …` runs
- **THEN** the Session's model `corp/main` evaluates the Goal

### Requirement: Impossible and blocking errors
(P2) On `impossible` the system SHALL set the Goal to `impossible`, stop continuation and notify the user. When a Turn under the active Goal fails (any provider or runtime error that ends it, after the retry policy), or ends because the user interrupted it or rejected a permission, the system SHALL set the Goal to `paused` with the reason, stop continuation, and notify the user; a Turn cancelled by a server shutdown SHALL NOT pause the Goal, which is evaluated after the restart instead. Queued Goals SHALL NOT start automatically after an `impossible` Goal unless `goals.on_impossible` is `continue`.

#### Scenario: Auth error pauses goal
- **WHEN** a Turn fails with a provider authentication error during an active Goal
- **THEN** the Goal is paused with that reason and no further Turns run

### Requirement: Verification check command
(P2) A Goal MAY set `check` (for example `cargo test --quiet`). After the evaluator returns `met`, the system SHALL run the check in the Session's Location under the sandbox with a timeout of `goals.check_timeout_seconds` (default 600); a non-zero exit or a timeout SHALL turn the verdict into `not_met`, with the last 50 lines of output in the continuation prompt. The check is the user's own command: like a `!` command and a loop's `--until`, it SHALL run without a permission prompt, so unattended runs (`cyber exec --goal-check`) can verify Goals.

#### Scenario: Tests contradict evaluator
- **WHEN** the evaluator says `met` but `cargo test` exits 101
- **THEN** the verdict becomes `not_met` and the failing output is included in the continuation prompt

#### Scenario: Set goal with check
- **WHEN** the user runs `/goal --check "npm test" all lint errors fixed`
- **THEN** the Goal's `check` is `npm test`

### Requirement: Evaluator cost accounting
(P2) Evaluator calls SHALL be recorded as hidden `usage.recorded.1` calls with purpose `goal_evaluator`, counted once in the Session's cost, usage statistics and the Goal's evaluator totals (`eval_count`, `eval_tokens`, `eval_cost_usd` on `session.goal.evaluated.1`), and SHALL use at most `goals.evaluator_max_input_tokens` (default 8000) of context by dropping older verdicts first, then tool summaries.

#### Scenario: Evaluator cost visible
- **WHEN** a Goal completes after 12 evaluations
- **THEN** `/goal` shows the evaluator token and cost totals separately from the work Turns
