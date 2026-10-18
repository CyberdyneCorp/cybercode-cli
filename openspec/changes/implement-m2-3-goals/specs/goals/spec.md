## MODIFIED Requirements

### Requirement: Goal management commands
(P2) The system SHALL provide `/goal` (show active Goal, queue and last verdict), `/goal list`, `/goal edit [id]` (opens the condition in the composer or editor), `/goal pause`, `/goal resume`, `/goal clear [id|all]`, `/goal skip` (mark the active Goal `cleared` and activate the next) and `/goal next` (alias of skip). Equivalent HTTP routes SHALL exist under `/api/v1/sessions/{id}/goals`. Editing a Goal SHALL publish `session.goal.edited.1` and reordering the queue SHALL publish `session.goal.reordered.1` with the new order of Goal IDs.

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
(P2) The system SHALL support pursuing independent Goals in parallel by running them in separate Sessions; `/goal fork <condition>` SHALL create a new background Session (in a new worktree when `goals.fork_isolation` is `worktree`, the default) with the condition as its active Goal, and list it in `cyber sessions` and the TUI session list (the agent view lists it once that view exists).

#### Scenario: Fork a goal
- **WHEN** the user runs `/goal fork upgrade all dependencies with green tests`
- **THEN** a new background Session in its own worktree starts working on that Goal while the current Session continues

### Requirement: Non-interactive goals
(P2) `cyber exec --goal "<condition>" [--goal-check <cmd>] [--max-turns N]` SHALL run until the Goal reaches a terminal status and exit with 0 for `met`, 3 for `impossible`, 4 for `budget_exceeded`, 1 for other errors (including a Goal paused by a blocking error, with the reason printed) and 130 when interrupted. `--goal` MAY be repeated to form a queue; `--goal-check` SHALL apply to the preceding `--goal`; the exit code SHALL reflect the first non-met Goal. Because `cyber exec` has no attached approver, permission asks SHALL be declined immediately as tool rejections and SHALL NOT pause the Goal.

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
