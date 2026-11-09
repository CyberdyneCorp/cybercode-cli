## Why

Agent teams are next on the roadmap after cross-session messaging (local, `implement-cross-session-messaging`) and direct peers (`add-messaging-direct-peers`). ROADMAP M2.5 lists "experimental lead/teammates with a shared task list". The `agent-teams` spec has 16 P2 requirements, one P3 (distributed teammates) and one P4 (teammate panes). None is implemented: there is no `team_` record, no `team_*` or team task tool, no `/team` command, no `cyber teams` command, and no team view.

What already exists and is reused:

- **Sessions with their own Drain**, subagent rule inheritance and Mode ceilings (`runtime/delegations.rs`, `cyber-tools/src/tools/agent.rs`), and `SessionInfo.workflow_run_id` as the precedent for an orchestration owner on a Session.
- **Worktrees** for isolated children (`cyber-tools/src/worktrees/child.rs` `create_child_worktree`, the `run-<ulid>-<index>` naming of Workflow agents).
- **Budgets**: the canonical `Budget` (`cyber-core/src/budget.rs`, `budgets.<scope>`), and the run-scope soft enforcement of Workflow Runs (`runtime/budget.rs` `run_scope`, `check_run`, `budget.exceeded.1`).
- **Hooks**: `TeammateIdle` is a registered event name with no producer (`cyber-core/src/config/hooks.rs:49`); `TaskCreated`/`TaskCompleted` fire today for background Jobs only (`runtime/lifecycle_events.rs`).
- **Messaging** (from `implement-cross-session-messaging`): the caller-scoped resolver with an empty team tier, `send_message`, the system watch that fires on Drain end, `xsession_message` history, and per-target rate limits. **Peers** (from `add-messaging-direct-peers`) are the later path for distributed teammates.

The `features` config key is accepted (`config/validate.rs:61`) but the feature registry of `configuration` → "Feature flags registry" (P0) is not implemented, and teams are its first gated feature.

## What Changes

- **Feature registry (minimum).** A compiled registry with `teams` (`experimental`, default off); `features.<name>`, `--features a,-b`, `CYBER_FEATURES`; the deprecated `experimental` alias; `cyber features list|enable|disable`. Unknown names fail with `ConfigInvalidError`.
- **Team records.** `team` and `team_member` and `team_task` tables projected from durable `team.*` events; `SessionInfo.team` (`{ team_id, role, name }`).
- **Team definitions** from `.cyber/teams/*.md` and `~/.config/cyber/teams/*.md`; `/team start <name>`; ad-hoc teams through `team_spawn`.
- **Runtime.** A team owner in `runtime/teams/` that creates teammate Sessions (root Sessions tagged with the team, not children, so deleting the lead stops but does not delete them), places them in the shared Location or their own worktree (`cyber/team-<ulid>/<name>`), applies the lead's Mode as a ceiling and its deny rules, and enforces `teams.max_teammates` (default 6).
- **Shared task list.** `team_task_create`, `team_task_list`, `team_task_get`, `team_task_update` with the dependency rule and atomic claims in one SQLite transaction; `TaskCreated`/`TaskCompleted` hooks for team tasks.
- **Messaging.** The team tier of the messaging resolver (`lead`, `all`, teammate names), `all` fanning out with one record per target; idle notices to the lead through a system watch; the `TeammateIdle` hook.
- **File conflicts.** Worktree isolation by default when two or more teammates can edit; `owns` globs enforced in shared-Location teams (`Path owned by teammate <name>`).
- **`team_merge`** for the lead: conflicts are detected with `git merge-tree --write-tree` before anything touches the lead's worktree.
- **Team budgets** (`budgets.team`, definition `budget`) summed across members, soft enforcement at Turn start, a spend breakdown to the lead.
- **Stop, resume and recovery.** `/team stop`, `/team resume`, lead deletion stops the team, restart recovery.
- **Surfaces.** `/api/v1/teams` routes with OpenAPI and SDK; `cyber teams list|show|stop|resume`; a TUI team view (members, task board, open a teammate and steer it, permission requests labelled with the teammate's name) and `/team start|stop|resume|view`.

Out of scope: P3 "Distributed teammates" (it will reuse `add-messaging-direct-peers` and `runners-cloud`; this change keeps `runner` as a reserved, rejected definition field) and P4 "Teammate panes" (`teams.display` accepts only `inline`).

## Capabilities

### Modified Capabilities

- `agent-teams`: team task tools are named `team_task_*` so they do not collide with the background-task tool `task_stop` and the `/tasks` view; idle notices cover a teammate that goes idle with a task still in progress; `team.resumed.1` and `team.budget.exceeded.1` join the durable events; `TaskCreated`/`TaskCompleted` payloads for team tasks carry `task` (Jobs keep `job`).

## Impact

- New: `crates/cyber-server/src/runtime/teams/`, `crates/cyber-server/src/http/teams.rs`, `crates/cyber-tools/src/tools/team.rs`, `crates/cyber-core/src/features.rs`, `crates/cyber-core/src/teams.rs` (definitions), `crates/cyber-cli/src/commands/{features,teams}.rs`, `crates/cyber-tui/src/{team.rs,view/team.rs,screens_team.rs}`, migration `crates/cyber-store/migrations/20261025000000_teams.sql`.
- Changed: messaging resolver (team tier), `runtime/budget.rs` (team scope), worktree child naming, lifecycle hooks (`TeammateIdle`, team `TaskCreated`/`TaskCompleted`), permission evaluation (ownership guard), OpenAPI and the TypeScript SDK.
- Depends on `implement-cross-session-messaging` (hard) and `add-messaging-direct-peers` (only for the P3 follow-up). Implement after both.
