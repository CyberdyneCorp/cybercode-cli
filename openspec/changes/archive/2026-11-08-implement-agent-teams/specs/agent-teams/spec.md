## MODIFIED Requirements

### Requirement: Shared task list
(P2) Each Team SHALL have a shared task list of tasks (`task_` IDs) with `title`, `description`, `status` (`todo`, `in_progress`, `blocked`, `done`, `cancelled`), `owner`, `depends_on` and timestamps, exposed to members through the `team_task_create`, `team_task_list`, `team_task_get` and `team_task_update` tools. The `team_` prefix keeps them apart from the background-task tool `task_stop` and the `/tasks` view. A task SHALL NOT move to `in_progress` while any dependency is not `done`, and a dependency cycle SHALL be rejected when it is written. Only the lead SHALL reassign a task owned by another member or cancel it.

#### Scenario: Dependency blocks start
- **WHEN** a teammate tries to start a task whose dependency is `todo`
- **THEN** the update fails with `Task depends on unfinished task_...`

#### Scenario: Dependency cycle rejected
- **WHEN** a member sets `task_a` to depend on `task_b` while `task_b` depends on `task_a`
- **THEN** the update fails with `Task dependency cycle: task_a → task_b → task_a` and neither task changes

### Requirement: Idle notifications
(P2) When a teammate becomes idle, the system SHALL notify the lead with the teammate's name and last status line, so the lead can assign more work or shut it down. With no `in_progress` task owned, the notice SHALL read `<name> is idle: all assigned tasks done` when every task it owns is `done`, else `<name> is idle: <last status line>`; with an `in_progress` task owned, it SHALL read `<name> is idle with task_<id> in progress: <last status line>`. No notice SHALL be sent while the Team is stopping or stopped.

#### Scenario: Teammate finishes
- **WHEN** `tests` completes its last task and goes idle
- **THEN** the lead receives `tests is idle: all assigned tasks done`

#### Scenario: Teammate stalls with work open
- **WHEN** `api` goes idle while `task_3` it owns is still `in_progress`
- **THEN** the lead receives `api is idle with task_3 in progress:` followed by `api`'s last status line

### Requirement: Team events
(P2) The system SHALL publish durable events `team.created.1`, `team.member.spawned.1`, `team.member.updated.1`, `team.task.updated.1`, `team.member.idle.1`, `team.budget.exceeded.1`, `team.stopped.1` and `team.resumed.1` in the lead's event stream, and hooks SHALL be able to subscribe to `TeammateIdle`, `TaskCreated` and `TaskCompleted`. For team tasks, `TaskCreated` and `TaskCompleted` SHALL carry `{ task, team_id, member }` with the matcher subject `team_task`; for background Jobs they keep `{ job }` and the Job kind as subject.

#### Scenario: Hook on task completion
- **WHEN** any team task moves to `done`
- **THEN** `TaskCompleted` hooks receive the task JSON with matcher subject `team_task`

#### Scenario: Resume is recorded
- **WHEN** the user resumes a stopped Team
- **THEN** `team.resumed.1` is appended to the lead's event stream with the Team ID and the lead Session
