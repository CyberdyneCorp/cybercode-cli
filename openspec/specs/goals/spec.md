# goals Specification

## Purpose
A Goal is a completion condition attached to a Session: after each Turn an evaluator checks it, and the Session keeps working until the Goal is met, judged impossible, or out of budget. Codex (`/goal`, one per chat) and Claude Code (`/goal`, one per session) support a single goal. Cyber Code adds an ordered goal queue, verification commands, budgets, and goals on workflow agents, so long multi-stage work runs unattended with checkable end states.

## Requirements

### Requirement: Goal model
(P2) The system SHALL attach to each Session at most one active Goal and an ordered goal queue of up to `goals.max_queue` (default 20) pending Goals. Each Goal (`gol_` ID) SHALL have `condition` (text, 1–4000 characters), optional `check` (shell command), optional `budget`, `status` (`pending`, `active`, `paused`, `met`, `impossible`, `cleared`, `budget_exceeded`) and evaluation history.

#### Scenario: Condition too long
- **WHEN** the user sets a goal of 5000 characters
- **THEN** it is rejected with `Goal conditions are limited to 4000 characters; put details in a file and reference it`

### Requirement: Set the active goal
(P2) `/goal <condition>` SHALL set the active Goal, replacing an existing active Goal (whose status becomes `cleared`), and SHALL immediately admit a prompt with the condition (`delivery: steer`) so work starts without a further message. Setting a goal SHALL require the `goal.set` permission (allowed by default for the user, `ask` when requested by the model).

#### Scenario: Start working toward a goal
- **WHEN** the user runs `/goal all call sites compile against the v2 API and cargo test passes`
- **THEN** the goal becomes active and the agent starts a Turn toward it

### Requirement: Goal queue
(P2) `/goal add <condition>` SHALL append a Goal to the queue (activating it immediately when no Goal is active). When the active Goal becomes `met`, the next pending Goal SHALL activate automatically at the next Safe Boundary and its condition SHALL be admitted as a steer prompt.

#### Scenario: Sequential goals
- **WHEN** goals A (active), B and C are queued and A is met
- **THEN** B becomes active and work continues without user input, then C after B

### Requirement: Goal management commands
(P2) The system SHALL provide `/goal` (show active Goal, queue and last verdict), `/goal list`, `/goal edit [id]` (opens the condition in the composer or editor), `/goal pause`, `/goal resume`, `/goal clear [id|all]`, `/goal skip` (mark the active Goal `cleared` and activate the next) and `/goal next` (alias of skip). Equivalent HTTP routes SHALL exist under `/api/v1/sessions/{id}/goals`.

#### Scenario: Pause goal
- **WHEN** the user runs `/goal pause`
- **THEN** evaluation and automatic continuation stop until `/goal resume`, and the Goal keeps its history

#### Scenario: Reorder via API
- **WHEN** a client PATCHes the queue order through the HTTP API
- **THEN** pending Goals are reordered and an event announces the new order

### Requirement: Evaluation after each turn
(P2) After each Turn that ends with no pending eligible input, the system SHALL run the evaluator agent with `model_roles.evaluator` (default `model_roles.small`) on the condition, the latest assistant output, a summary of the Turn's tool results and the `check` result if any, and SHALL record a verdict `met`, `not_met` or `impossible` with a reason of at most 300 characters as a durable `session.goal.evaluated.1` event.

#### Scenario: Verdict recorded
- **WHEN** a Turn ends while a Goal is active
- **THEN** a `session.goal.evaluated.1` event with verdict and reason is appended and shown as one status line

### Requirement: Continuation on not met
(P2) On `not_met`, the system SHALL admit a synthetic continuation prompt (`delivery: steer`) containing the condition and the evaluator's reason, and SHALL start another Turn instead of returning control to the user.

#### Scenario: Keep working
- **WHEN** the evaluator returns `not_met: 3 tests still failing in auth_test.rs`
- **THEN** a continuation prompt including that reason is admitted and the next Turn begins

### Requirement: Completion
(P2) On `met` (and a passing `check` when configured), the system SHALL set the Goal to `met`, publish `session.goal.completed.1`, notify the user (including remote devices when connected) and activate the next queued Goal, or return the Session to idle when the queue is empty.

#### Scenario: Notification on completion
- **WHEN** the last queued goal is met while the user is away with remote control connected
- **THEN** a push notification reports the Goal as met and the Session goes idle

### Requirement: Impossible and blocking errors
(P2) On `impossible`, or when a Turn fails with an error the user must fix (authentication failure, model unavailable, disk full, permission rejected without feedback), the system SHALL set the Goal to `paused` with the reason (for `impossible` the status SHALL be `impossible`), stop continuation, and notify the user. Queued Goals SHALL NOT start automatically after an `impossible` Goal unless `goals.on_impossible` is `continue`.

#### Scenario: Auth error pauses goal
- **WHEN** a Turn fails with a provider authentication error during an active Goal
- **THEN** the Goal is paused with that reason and no further Turns run

### Requirement: Verification check command
(P2) A Goal MAY set `check` (for example `cargo test --quiet`). After the evaluator returns `met`, the system SHALL run the check in the Session's Location under the sandbox with a timeout of `goals.check_timeout_seconds` (default 600); a non-zero exit SHALL turn the verdict into `not_met` with the last 50 lines of output as the reason. Check commands SHALL require `bash` permission like any command.

#### Scenario: Tests contradict evaluator
- **WHEN** the evaluator says `met` but `cargo test` exits 101
- **THEN** the verdict becomes `not_met` and the failing output is included in the continuation prompt

#### Scenario: Set goal with check
- **WHEN** the user runs `/goal --check "npm test" all lint errors fixed`
- **THEN** the Goal's `check` is `npm test`

### Requirement: Goal budgets
(P2) Each Goal SHALL enforce a Budget (`observability-costs`) with `max_turns` defaulting to `budgets.goal.max_turns` (50) and optional `max_tokens`, `max_cost_usd` and `max_wall_seconds`, counted from activation and including subagent costs. When exceeded, the Goal SHALL become `budget_exceeded`, continuation SHALL stop and the user SHALL be notified with spend figures.

#### Scenario: Turn budget
- **WHEN** a Goal with default budget reaches 50 Turns without being met
- **THEN** it becomes `budget_exceeded` and the Session stops continuing

### Requirement: Deferred evaluation during background work
(P2) When a Turn ends while background subagents, background tasks or workflow runs started by the Session are still running, evaluation SHALL be skipped for that Turn and performed at the end of the next Turn that ends with no such work running; handbacks from that work SHALL be delivered as queued input first.

#### Scenario: Waiting on background tests
- **WHEN** a Turn ends with a background `cargo test` job still running
- **THEN** no evaluation runs until the job's completion notice is processed and a later Turn ends idle

### Requirement: Goals survive resume and restart
(P2) Active and queued Goals SHALL be persisted with the Session and restored on `cyber --resume`, server restart and Session handoff between machines. On restore, an active Goal SHALL be evaluated once before continuing rather than continuing blindly.

#### Scenario: Restart mid-goal
- **WHEN** the server restarts while a Goal is active
- **THEN** after restart the evaluator runs on the last Turn and continuation proceeds only on `not_met`

### Requirement: Goal context source
(P2) The active Goal, its last verdict and the count of queued Goals SHALL be provided to the model as a typed System Context source `core/goal`, so changes arrive as Mid-Conversation System Messages without rewriting the cached baseline.

#### Scenario: Model sees goal
- **WHEN** a Goal is activated mid-session
- **THEN** the next Turn includes a system message stating the active Goal and its condition

### Requirement: User input during a goal
(P2) User prompts admitted while a Goal is active SHALL be promoted with `steer` delivery as usual and SHALL NOT clear the Goal; evaluation SHALL resume after the Turn that processes them.

#### Scenario: User adds a hint
- **WHEN** the user types a hint while the agent works toward a Goal
- **THEN** the hint is applied at the next Safe Boundary and the Goal stays active

### Requirement: Parallel goals across sessions
(P2) The system SHALL support pursuing independent Goals in parallel by running them in separate Sessions; `/goal fork <condition>` SHALL create a new background Session (in a new worktree when `goals.fork_isolation` is `worktree`, the default) with the condition as its active Goal, and list it in the agent view.

#### Scenario: Fork a goal
- **WHEN** the user runs `/goal fork upgrade all dependencies with green tests`
- **THEN** a new background Session in its own worktree starts working on that Goal while the current Session continues

### Requirement: Goals on workflow agents and subagents
(P2) The `agent` tool and workflow `agent()` calls SHALL accept a `goal` option (`{ condition, check?, budget? }`) that becomes the child Session's active Goal; the call SHALL resolve only when that Goal reaches a terminal status, returning the status with the result.

#### Scenario: Workflow agent with goal
- **WHEN** a workflow calls `agent("Migrate file X", { goal: { condition: "file compiles", check: "cargo check" } })`
- **THEN** the child keeps working until the check passes or the goal ends, and the result includes `goal_status`

### Requirement: Goal templates
(P2) The `goals.templates` config key SHALL define named templates `{ condition, check?, budget? }` usable as `/goal use <name> [extra text]`, with `$ARGUMENTS` substitution in the condition.

#### Scenario: Use template
- **WHEN** config defines `goals.templates.green = { condition: "$ARGUMENTS and all tests pass", check: "just test" }` and the user runs `/goal use green auth module refactored`
- **THEN** the active Goal condition is `auth module refactored and all tests pass` with check `just test`

### Requirement: Non-interactive goals
(P2) `cyber exec --goal "<condition>" [--goal-check <cmd>] [--max-turns N]` SHALL run until the Goal reaches a terminal status and exit with 0 for `met`, 3 for `impossible`, 4 for `budget_exceeded`, 1 for other errors and 130 when interrupted. `--goal` MAY be repeated to form a queue; the exit code SHALL reflect the first non-met Goal.

#### Scenario: CI goal
- **WHEN** CI runs `cyber exec --goal "lint passes" --goal-check "npm run lint"`
- **THEN** the process exits 0 once the check passes after the evaluator says met

### Requirement: Goal events
(P2) The system SHALL publish durable events `session.goal.set.1`, `session.goal.queued.1`, `session.goal.activated.1`, `session.goal.evaluated.1`, `session.goal.paused.1`, `session.goal.resumed.1`, `session.goal.completed.1` and `session.goal.ended.1` (with terminal status), and hooks SHALL be able to subscribe to `GoalEvaluated` and `GoalCompleted`.

#### Scenario: Hook on completion
- **WHEN** a hook is configured for `GoalCompleted` with a command
- **THEN** the command runs with the Goal JSON on stdin when any Goal is met

### Requirement: Evaluator cost accounting
(P2) Evaluator calls SHALL be recorded as hidden assistant messages of the `evaluator` agent, counted in the Session's cost and the Goal's budget, and SHALL use at most `goals.evaluator_max_input_tokens` (default 8000) of context by summarizing older Turns.

#### Scenario: Evaluator cost visible
- **WHEN** a Goal completes after 12 evaluations
- **THEN** `/goal` shows the evaluator token and cost totals separately from the work Turns

### Requirement: Goal mode safety
(P2) A Goal SHALL NOT raise the Session's Mode. When a Turn under a Goal hits a permission `ask` and no user is attached (no client connected for `goals.unattended_ask_seconds`, default 300), the request SHALL remain pending, the Goal SHALL pause with reason `Waiting for approval`, and a notification SHALL be sent.

#### Scenario: Unattended approval
- **WHEN** a goal-driven Turn needs approval while the user is away for 5 minutes
- **THEN** the Goal pauses and the user receives a notification with the pending request
