# agent-teams Specification

## Purpose
Agent teams coordinate several full Sessions: a lead plans and assigns work, named teammates each work in their own context, and all share a task list and message each other directly. This follows Claude Code's experimental agent teams, built on Cyber Code's cross-session messaging and worktrees, with budgets and resumable state. Teams are behind the experimental flag `experimental.teams` in P2.

## Requirements

### Requirement: Feature flag
(P2) Agent teams SHALL be available only when `experimental.teams` is `true` (or `CYBER_EXPERIMENTAL_TEAMS=1`). Without it, no team state SHALL be created and the `team_*` tools SHALL NOT be offered.

#### Scenario: Disabled by default
- **WHEN** a user asks for a team without enabling the flag
- **THEN** no teammates are spawned and the model is told teams are disabled

### Requirement: Team composition
(P2) A Team (`team_` ID) SHALL consist of one lead Session and up to `teams.max_teammates` (default 6) teammate Sessions, each with a unique name, an agent, an optional model, and a Location (shared or its own worktree). Teammates SHALL be full Sessions with their own context and Drain.

#### Scenario: Spawn teammates
- **WHEN** the lead calls `team_spawn` with names `api`, `ui` and `tests`
- **THEN** three teammate Sessions start and appear in the team view

#### Scenario: Teammate limit
- **WHEN** the lead tries to spawn a 7th teammate with the default limit
- **THEN** the call fails with `Team size limit reached (6)`

### Requirement: Team definitions
(P2) The system SHALL load reusable team definitions from `.cyber/teams/*.md` and `~/.config/cyber/teams/*.md` with frontmatter `name`, `description`, `teammates` (list of `{ name, agent, model?, isolation?, owns? }`) and `budget`, and the body as lead instructions. `/team start <name>` SHALL instantiate it.

#### Scenario: Start a defined team
- **WHEN** `.cyber/teams/feature.md` defines lead instructions and three teammates
- **THEN** `/team start feature` creates the team with those teammates and instructions

### Requirement: Shared task list
(P2) Each Team SHALL have a shared task list of tasks (`task_` IDs) with `title`, `description`, `status` (`todo`, `in_progress`, `blocked`, `done`, `cancelled`), `owner`, `depends_on` and timestamps, exposed to members through `task_create`, `task_list`, `task_get` and `task_update` tools. A task SHALL NOT move to `in_progress` while any dependency is not `done`.

#### Scenario: Dependency blocks start
- **WHEN** a teammate tries to start a task whose dependency is `todo`
- **THEN** the update fails with `Task depends on unfinished task_...`

### Requirement: Task claims
(P2) Claiming a task (setting `owner` and `in_progress`) SHALL be atomic: when two members claim the same task concurrently, exactly one SHALL succeed and the other SHALL receive `Task already claimed by <name>`.

#### Scenario: Concurrent claim
- **WHEN** `api` and `tests` claim `task_7` at the same moment
- **THEN** one owns it and the other gets the already-claimed error

### Requirement: Teammate messaging
(P2) Team members SHALL message each other by name using the local transport of cross-session-messaging (`send_message` with `to: "<teammate>"` or `to: "lead"` or `to: "all"`), with the same delivery rules (between tool calls when busy, new Turn when idle).

#### Scenario: Teammate asks lead
- **WHEN** `ui` sends `which API shape should I use?` to `lead`
- **THEN** the lead receives it at its next Safe Boundary and can reply to `ui`

### Requirement: Idle notifications
(P2) When a teammate becomes idle with no `in_progress` task, the system SHALL notify the lead with the teammate's name and last status line, so the lead can assign more work or shut it down.

#### Scenario: Teammate finishes
- **WHEN** `tests` completes its last task and goes idle
- **THEN** the lead receives `tests is idle: all assigned tasks done`

### Requirement: User access to teammates
(P2) The user SHALL be able to open any teammate's Session from the team view and send it prompts directly, without going through the lead; such prompts SHALL be admitted with `steer` delivery.

#### Scenario: Direct instruction
- **WHEN** the user opens teammate `api` and types `use pagination tokens`
- **THEN** the instruction is admitted to `api` only

### Requirement: File conflict avoidance
(P2) Teammates with edit access SHALL either run in their own worktree (`isolation: worktree`, default when two or more teammates can edit) or declare ownership globs (`owns`); an edit by a teammate to a path owned by another teammate SHALL be denied with `Path owned by teammate <name>` in shared-Location teams.

#### Scenario: Ownership enforced
- **WHEN** teammate `ui` (owns `web/**`) edits `server/api.rs` owned by `api`
- **THEN** the edit is denied with the ownership message

### Requirement: Merging teammate work
(P2) When teammates use worktrees, the lead SHALL have a `team_merge` tool that merges a teammate's branch into the lead's branch, reporting conflicts without resolving them automatically.

#### Scenario: Conflict reported
- **WHEN** the lead merges `ui`'s branch and a conflict occurs
- **THEN** the merge stops, the conflicting files are listed, and the lead's worktree stays unchanged

### Requirement: Team budgets
(P2) A Team SHALL enforce `budget.max_cost_usd`, `max_tokens` and `max_wall_minutes` across all members; on exceed, teammates SHALL stop after their current Turn and the lead SHALL be notified with spend per member.

#### Scenario: Budget exceeded
- **WHEN** a team with `max_cost_usd: 20` spends $20.10
- **THEN** all teammates stop and the lead receives a spend breakdown

### Requirement: Permissions in teams
(P2) Teammates SHALL run with the lead's Mode as a ceiling and inherit the lead's deny rules; permission requests from teammates SHALL surface to the user with the teammate's name.

#### Scenario: Teammate approval prompt
- **WHEN** teammate `tests` needs approval to run `docker compose up`
- **THEN** the user sees the request labelled with `tests`

### Requirement: Shutdown and resume
(P2) `/team stop` SHALL stop all teammates after their current Turn and mark the Team `stopped`; `/team resume` SHALL restart teammates with their existing Sessions, task list and messages intact. Deleting the lead Session SHALL stop the Team.

#### Scenario: Resume next day
- **WHEN** the user resumes a stopped team
- **THEN** teammates continue with their prior context and the shared task list

### Requirement: Team view
(P2) The TUI SHALL provide a team view listing each member with status (`working`, `idle`, `waiting_input`, `stopped`), current task, cost and last message, plus the task board grouped by status. `cyber teams list|show|stop|resume` SHALL expose the same information from the CLI.

#### Scenario: View task board
- **WHEN** the user opens the team view
- **THEN** tasks are shown in `todo`, `in_progress`, `blocked` and `done` columns with owners

### Requirement: Team events
(P2) The system SHALL publish durable events `team.created.1`, `team.member.spawned.1`, `team.task.updated.1`, `team.member.idle.1` and `team.stopped.1`, and hooks SHALL be able to subscribe to `TeammateIdle` and `TaskCompleted`.

#### Scenario: Hook on task completion
- **WHEN** any team task moves to `done`
- **THEN** `TaskCompleted` hooks receive the task JSON
