## ADDED Requirements

### Requirement: Replay of workflow host calls
(P2) Resumable runs SHALL persist results of all observable host calls, including ask, read-only tools, phase checkpoints and agent results, keyed by stable call identity and invocation sequence. Replay SHALL return recorded results instead of rereading changed files or asking the user again. A run SHALL record script and configuration digests. An edited script SHALL create an explicit revision with reuse limited to compatible recorded calls; divergent host-call sequences SHALL fail with `WorkflowReplayMismatch` rather than silently reuse results. Unsettled agents SHALL resume their existing child Session and tool recovery records before any replacement is spawned.

#### Scenario: Files change while run is stopped
- **WHEN** a workflow resumes after its earlier glob result would now differ
- **THEN** replay uses the recorded glob result for that revision and resumes existing child Sessions

## MODIFIED Requirements

### Requirement: Durable agent results and resume
(P2) The system SHALL key every `agent()` call by a hash of its prompt, normalized options and call ordinal for that (prompt, options) pair, and SHALL persist each settled result as a durable `workflow.agent.settled.1` event before resolving the promise. Relaunching a run (after pause, crash, server restart or script edit) SHALL re-execute the script from the start, resolving calls whose key already has a settled result immediately from the store, and resuming unsettled existing child Sessions before spawning calls with new or changed keys. Script edits and other host calls SHALL follow Replay of workflow host calls.

#### Scenario: Resume after crash
- **WHEN** the server restarts while a run has 30 of 50 agents settled
- **THEN** resuming the run returns the 30 cached results instantly and resumes the existing child Sessions for the remaining 20, reconciling any unknown tool outcomes

#### Scenario: Edited prompt reruns
- **WHEN** the user edits one agent prompt in the script and relaunches
- **THEN** only that call (and calls depending on its output) spawn new agents

### Requirement: Run lifecycle states
(P2) A Workflow Run (`run_` ID) SHALL have status `queued`, `running`, `waiting_input`, `paused`, `completed`, `failed`, `budget_exceeded` or `stopped`. Pause SHALL stop scheduling new agents while letting in-flight agents finish; stop SHALL cancel in-flight agents and mark them `cancelled`; resume SHALL continue a `paused`, `failed` or `stopped` run through the resume mechanism. Transitions SHALL be durable events (`workflow.run.started.1`, `workflow.run.paused.1`, `workflow.run.resumed.1`, `workflow.run.completed.1`, `workflow.run.failed.1`, `workflow.run.stopped.1`).

#### Scenario: Pause lets agents finish
- **WHEN** the user pauses a run with 4 agents in flight
- **THEN** those 4 complete and are stored, and no new agent starts until resume

#### Scenario: Resume refused while agents alive
- **WHEN** the user relaunches a stopped run whose cancelled agents have not exited yet
- **THEN** the relaunch is refused with `Run has agents still exiting; retry shortly`

### Requirement: Run budgets
(P2) A run SHALL accept budgets `max_agents` (default 200), `max_tokens`, `max_cost_usd` and `max_wall_minutes` (default 240) from `meta`, invocation options or `workflows.default_budget`. When a budget would be exceeded, new `agent()` calls SHALL reject with `BudgetExceeded` naming the budget, in-flight agents SHALL finish, and `ctx.budget` SHALL expose `{ spent_tokens, spent_cost_usd, agents_started, remaining }` to the script at any time. Cost and token dispatch SHALL obey the soft/reserved enforcement and uncertain usage rules in observability-costs, including checks before each child provider Turn. A halted budgeted run SHALL enter `budget_exceeded`.

#### Scenario: Cost budget reached
- **WHEN** a run with `max_cost_usd: 5` has spent $5.02
- **THEN** the next `agent()` call rejects with `BudgetExceeded: max_cost_usd` and the script may return a partial result
