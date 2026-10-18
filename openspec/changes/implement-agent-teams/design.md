## Context

`agent-teams` has 18 requirements: 16 P2, 1 P3, 1 P4. This change implements the 16 P2 requirements behind the `teams` feature.

| Requirement | Phase | This change |
|---|---|---|
| Feature flag | P2 | yes, with the minimum feature registry (section 1) |
| Team composition | P2 | yes |
| Team definitions | P2 | yes; `runner` is reserved and rejected until P3 |
| Shared task list | P2 | yes, tools renamed `team_task_*` (spec delta) |
| Task claims | P2 | yes |
| Teammate messaging | P2 | yes, the team tier of the messaging resolver |
| Idle notifications | P2 | yes, clarified for an idle teammate with a task in progress (spec delta) |
| User access to teammates | P2 | yes |
| File conflict avoidance | P2 | yes |
| Merging teammate work | P2 | yes |
| Team budgets | P2 | yes |
| Permissions in teams | P2 | yes |
| Shutdown and resume | P2 | yes |
| Team view | P2 | yes, TUI and CLI |
| Team events | P2 | yes, plus `team.resumed.1` and `team.budget.exceeded.1` (spec delta) |
| Distributed teammates | P3 | out; reuses `add-messaging-direct-peers` later |
| Teammate panes | P4 | out; `teams.display` accepts only `inline` |

Code this builds on:

- **Sessions.** `SessionInfo` (`runtime/model.rs:50`) already carries an orchestration owner (`workflow_run_id`) and a child link (`parent_id`, `subagent_name`). Children are deleted with their parent; teammates must survive the lead's deletion (the team is stopped instead), so teammates are root Sessions with a `team` tag, not children.
- **Subagent inheritance.** Agent selection, the Mode ceiling and deny-rule inheritance of `cyber-tools/src/tools/agent.rs` and `runtime/delegations.rs`.
- **Worktrees.** `create_child_worktree` (`cyber-tools/src/worktrees/child.rs:44`) and `child_naming`, which already special-cases Workflow agents (`run-<ulid>-<index>` on `cyber/run-<ulid>/<index>`); removal goes through the owned-worktree path fixed in `f48f646`.
- **Budgets.** `runtime/budget.rs` `run_scope`, `run_spend`, `check_run` give soft run-scope enforcement for Workflow Runs over `usage_step`; teams add one more scope.
- **Hooks.** `TeammateIdle` is registered with no producer (`config/hooks.rs:49`). `TaskCreated`/`TaskCompleted` fire for Jobs with `{ job }` (`runtime/lifecycle_events.rs:294`).
- **Messaging** (`implement-cross-session-messaging`, section 17 "Agent teams hand-off"): the resolver's team tier is an empty extension point; `Messaging::send` handles delivery, limits, records and recovery; `watch.rs` fires on Drain end; `xsession_message` is the history.

## Goals / Non-Goals

**Goals:** a lead and up to six teammates work as full Sessions; the task board and claims are race-free; all team state is durable and a restart or `/team resume` continues where it stopped; teammates never exceed the lead's permissions; nothing new is added to message delivery; everything is invisible unless `teams` is enabled.

**Non-Goals:** distributed teammates (P3), tmux/iTerm2 panes (P4), automatic conflict resolution in `team_merge`, nested teams (a teammate cannot lead a team), cross-team messaging by team-local names (other teams' members are addressed by Session ID or title, like any Session).

## Decisions

### 1. Feature registry (minimum)

`crates/cyber-core/src/features.rs`: a static table `[(name, maturity, default)]` with `teams` (`experimental`, off). Resolution order, highest first: org `policy.features.lock`, `--features` (comma list, `-name` disables), `CYBER_FEATURES`, merged config `features.<name>`, the deprecated `experimental.<name>` (warning `experimental.teams is deprecated; use features.teams`), the default. An unknown name in any source fails with `ConfigInvalidError: unknown feature "<name>"`. `cyber features list [--json]` prints name, maturity, default, effective value and source; `cyber features enable|disable <name> [--scope user|project]` edits the config document with the existing JSONC editor. The server resolves features once per config load and exposes `Features::enabled("teams")`; `GET /api/v1/features` returns the list for clients.

When `teams` is off: no `team_*` tool is advertised, team routes return `404 FeatureDisabledError`, `/team` prints `Agent teams are disabled. Enable them with "cyber features enable teams".`, and a model that asks for a team gets that sentence from the `core/teams` context source being absent plus the tool being missing (scenario "Disabled by default" is checked by the absence of `team_spawn` and the TUI message).

### 2. Data model

Migration `20261025000000_teams.sql`:

```sql
CREATE TABLE team (
    id            TEXT PRIMARY KEY,               -- team_<ulid>
    name          TEXT NOT NULL,                  -- definition name or "adhoc"
    lead_id       TEXT NOT NULL,                  -- the lead Session (no FK: survives lead deletion as stopped)
    directory     TEXT NOT NULL,
    status        TEXT NOT NULL,                  -- active | stopping | stopped
    stop_reason   TEXT,                           -- user | budget | lead_deleted
    budget        TEXT,                           -- Budget JSON
    instructions  TEXT,                           -- lead instructions from the definition body
    created_ms    INTEGER NOT NULL,
    updated_ms    INTEGER NOT NULL
);
CREATE TABLE team_member (
    team_id       TEXT NOT NULL REFERENCES team(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,                  -- unique in the team; "lead" for the lead
    session_id    TEXT NOT NULL,
    role          TEXT NOT NULL,                  -- lead | teammate
    agent         TEXT NOT NULL,
    model         TEXT,
    isolation     TEXT NOT NULL,                  -- shared | worktree
    worktree_id   TEXT,
    branch        TEXT,
    owns          TEXT,                           -- JSON array of globs
    status        TEXT NOT NULL,                  -- working | idle | waiting_input | stopped
    last_status   TEXT,                           -- last status line, <= 200 chars
    PRIMARY KEY (team_id, name),
    UNIQUE (session_id)
);
CREATE TABLE team_task (
    id            TEXT PRIMARY KEY,               -- task_<ulid>
    team_id       TEXT NOT NULL REFERENCES team(id) ON DELETE CASCADE,
    seq           INTEGER NOT NULL,               -- display order in the team
    title         TEXT NOT NULL,
    description   TEXT NOT NULL DEFAULT '',
    status        TEXT NOT NULL,                  -- todo | in_progress | blocked | done | cancelled
    owner         TEXT,                           -- member name
    depends_on    TEXT NOT NULL DEFAULT '[]',     -- JSON array of task ids in the same team
    created_by    TEXT NOT NULL,
    created_ms    INTEGER NOT NULL,
    updated_ms    INTEGER NOT NULL,
    version       INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX team_task_board ON team_task (team_id, status, seq);
```

All three tables are projections of durable events in the lead's event stream (the team's single writer): `team.created.1`, `team.member.spawned.1`, `team.member.updated.1` (status, last status line), `team.task.updated.1` (full task after the change, with `previous_status`), `team.member.idle.1`, `team.budget.exceeded.1`, `team.stopped.1`, `team.resumed.1`. A store rebuild re-projects them. `SessionInfo` gains `team: Option<TeamTag { team_id, role, name }>`, set at creation and never changed, so every surface can label a teammate without a join.

### 3. Team definitions

`crates/cyber-core/src/teams.rs` parses `*.md` files with YAML frontmatter, loaded from `.cyber/teams/` (project, trust-gated like agents) and `~/.config/cyber/teams/` (user); a project definition with the same name wins. Fields: `name` (`[a-z][a-z0-9-]{0,31}`), `description`, `teammates: [{ name, agent, model?, isolation?: shared|worktree, owns?: [glob], runner? }]`, `budget` (a `Budget`); the body is the lead instructions. Validation names the field: teammate names unique, not `lead` or `all`, at most `teams.max_teammates`; an unknown agent fails at start, not load; `runner` fails with `Distributed teammates are not available yet (runner)`. Errors surface in `cyber teams definitions` and `/team start` and never block other definitions.

### 4. Runtime

`crates/cyber-server/src/runtime/teams/`:

```
mod.rs       Teams facade on Runtime: create, spawn, stop, resume, view, members
records.rs   events, projectors, state fold
spawn.rs     teammate Session creation, isolation choice, permissions ceiling
tasks.rs     task board, dependency rule, atomic claims
idle.rs      system watches on teammates, idle notices, TeammateIdle
budget.rs    team scope over runtime/budget.rs
ownership.rs owns-glob guard for shared-Location teams
merge.rs     team_merge
recovery.rs  startup reconciliation
```

- **Creating a team.** `/team start <name>` or `POST /api/v1/teams` with a definition name makes the current Session the lead, records `team.created.1`, admits the definition body to the lead as a `team_instructions` system message, then spawns the defined teammates. `team_spawn` from a lead without a team creates an ad-hoc team (`name: "adhoc"`) first. A Session that is already a teammate cannot create or join another team (`Teammates cannot lead a team`).
- **Spawning.** `team_spawn({ teammates: [{ name, agent?, model?, isolation?, owns?, prompt? }] })` (lead only) creates each teammate with `CreateSession` (root, `team` tag, title `<team name>/<member>`), agent default `build`, the lead's model unless set. The size check runs inside the same lock as the spawn: the 7th teammate under the default fails with `Team size limit reached (6)` and nothing is created. `prompt` is admitted as the teammate's first input with `source: "team"`.
- **Isolation.** `isolation` defaults to `worktree` when, counting the new members, two or more teammates have an agent that can edit (the agent's tool set includes `edit`, `write`, `patch` or `bash`), else `shared`. Worktrees use `create_child_worktree` with team naming `team-<ulid>-<name>` on branch `cyber/team-<ulid>/<name>`, based on the lead's current HEAD.
- **Permissions ceiling.** A teammate's effective Mode is the stricter of its own and the lead's at each Turn start (so a later lead switch to `plan` applies from the teammate's next Turn); the lead's `deny` rules are prepended to the teammate's ruleset the way subagents inherit them. A teammate cannot switch to a Mode above the ceiling (`Mode limited by the team lead (<mode>)`).
- **Teammate status.** `working` while a Drain runs, `waiting_input` while a permission or question request is open, `idle` after the Drain ends, `stopped` after stop; changes are `team.member.updated.1`.

### 5. Shared task list and claims

Tools (advertised to team members only): `team_task_create({ title, description?, depends_on?, owner? })`, `team_task_list({ status?, owner? })`, `team_task_get({ id })`, `team_task_update({ id, status?, owner?, title?, description?, depends_on?, expected_version? })`.

- All writes go through the lead's runtime (single writer) in one SQLite transaction that reads the row, checks, appends `team.task.updated.1` and projects it.
- **Dependency rule.** A move to `in_progress` fails with `Task depends on unfinished task_<id>` (the first unfinished dependency) unless every dependency is `done`. Dependencies must be tasks of the same team; cycles are rejected at write (`Task dependency cycle: task_a → task_b → task_a`).
- **Claims.** A claim is `team_task_update({ id, status: "in_progress", owner: <self> })` (owner defaults to the caller). Inside the transaction, if `owner` is set to another member and the status is `in_progress`, the call fails with `Task already claimed by <name>`. Two concurrent claims serialize on the transaction; exactly one commits. `expected_version` gives optimistic concurrency for other edits (`Task changed since version <n>`).
- Only the lead may reassign a task owned by someone else or cancel it; a teammate may update its own tasks and create tasks.
- **Hooks.** `TaskCreated` and `TaskCompleted` (on a move to `done`) fire for team tasks with `{ task, team_id, member }` and matcher subject `team_task`; Job tasks keep `{ job }` and their Job kind subject (spec delta).

### 6. Messaging integration

The resolver's team tier (`implement-cross-session-messaging` design section 3) resolves, for a caller that is a team member, `lead`, `all` and member names before any other tier. `all` expands to every other member that is not `stopped`; `Messaging::send` runs once per target, so each target gets its own record, outcome, inbound evaluation and rate limit, and the tool result lists one outcome per name. Team members have an implicit inbound `deliver` for senders in the same team (team-local `refuse`/`hold` rules still win). No delivery code is added.

### 7. Idle notifications

When a teammate's Drain ends with nothing promotable, `idle.rs` (registered as a system watch on every active teammate) records `team.member.idle.1` and admits a notice to the lead with ID `msgx_team_idle_<team>_<member>_<seq>` and `source: "team"`:

- no `in_progress` task owned: `<name> is idle: all assigned tasks done` when it owns `done` tasks and nothing open, else `<name> is idle: <last status line>`;
- with an `in_progress` task: `<name> is idle with task_<id> in progress: <last status line>` (spec delta).

The last status line is the teammate's last assistant text line (≤ 200 characters). `TeammateIdle` hooks run after the notice is recorded, with `{ team_id, member, session_id, tasks_in_progress, last_status }`; their decisions are only logged. Notices are recovered at startup like watch notices. An idle notice is not sent while the team is `stopping` or `stopped`.

### 8. User access to teammates

The TUI team view opens a teammate's Session in the normal transcript (the TUI switches Session as `/sessions` does). A prompt typed there is admitted to that Session with `delivery: steer` and `source: user`; it never reaches the lead. `POST /api/v1/sessions/:id/prompt` already does this for any client.

### 9. File ownership

In a team with any `shared` teammate, `ownership.rs` hooks into permission evaluation for `edit`, `write`, `patch` and the write targets the bash analysis reports: if the path (relative to the team directory) matches another member's `owns` glob, the call is denied with `Path owned by teammate <name>` before any rule or ask. The lead is not exempt (it uses `team_merge` or messages). Members in their own worktree are not checked (their paths are outside the shared Location).

### 10. `team_merge`

`team_merge({ member, message? })` (lead only, worktree members only):

1. Refuse if the lead's worktree has uncommitted changes (`Commit or stash your changes before merging`), or the member's branch has no commits beyond the merge base (`Nothing to merge from <name>`).
2. Run `git merge-tree --write-tree <lead HEAD> <member branch>` (git ≥ 2.38; older git fails with `team_merge needs git 2.38 or later`). On conflicts, return the conflicted paths and stop; nothing in the lead's worktree changes.
3. On a clean result, `git merge --no-ff -m <message> <branch>` in the lead's worktree under the normal sandbox, then report the merge commit.

The member's worktree and branch are kept until the team is removed.

### 11. Team budgets

The team budget is the definition `budget`, else `budgets.team`, else none. Spend is the sum over member Sessions of `usage_step` rows since `team.created.1` (turns, tokens, cost) and wall time since creation, read with the existing `run_spend` style query keyed by `team_member.session_id`. `check_team` runs at each member's Turn start (soft enforcement, like Workflow Runs): when the budget is exceeded, the Turn does not start, `team.budget.exceeded.1` is recorded once with a per-member breakdown, every teammate stops after its current Turn (the team moves to `stopping`, then `stopped` with reason `budget`), and the lead receives `Team budget exceeded (max_cost_usd 20.00, spent 20.10): api $8.40, ui $6.20, tests $5.50`. The lead's own Turns are not blocked, so it can report.

### 12. Stop, resume, lead deletion and recovery

- `/team stop`, `POST /api/v1/teams/:id/stop`, `cyber teams stop`: status `stopping`; each teammate gets the cooperative stop that Sessions already support (the current Turn finishes, queued input stays queued); when all are idle, `team.stopped.1` with reason `user`. Worktrees are kept.
- `/team resume`: allowed from `stopped` (not after `lead_deleted` unless `--lead <session>` names a new lead); records `team.resumed.1`, members back to `idle`, idle watches re-armed, and each teammate with an `in_progress` task or queued input gets its Drain restarted. Sessions, tasks and `xsession_message` history are untouched, so teammates continue with their context.
- Deleting the lead runs the stop with reason `lead_deleted` before the Session is deleted.
- `recovery.rs` at startup: teams in `stopping` finish stopping; re-arm idle watches for `active` teams; admit unrecorded idle notices.

### 13. Events and hooks summary

| Event | Stream | Payload |
|---|---|---|
| `team.created.1` | lead | `team_id, name, directory, budget?, definition?` |
| `team.member.spawned.1` | lead | `team_id, name, session_id, role, agent, model?, isolation, worktree_id?, branch?, owns` |
| `team.member.updated.1` | lead | `team_id, name, status, last_status?` |
| `team.task.updated.1` | lead | `team_id, task, previous_status?, by` |
| `team.member.idle.1` | lead | `team_id, name, tasks_in_progress, last_status` |
| `team.budget.exceeded.1` | lead | `team_id, budget, spent, members: [{ name, turns, tokens, cost_usd }]` |
| `team.stopped.1` | lead | `team_id, reason` |
| `team.resumed.1` | lead | `team_id, lead_id` |

Hooks: `TeammateIdle` (section 7), `TaskCreated`/`TaskCompleted` for team tasks (section 5).

### 14. HTTP API, OpenAPI and SDK

`GET /api/v1/teams` (list, `?status`), `POST /api/v1/teams` (`{ lead_session_id, definition?, teammates?, budget? }`), `GET /api/v1/teams/:id` (team, members with status, current task, cost, last message from `xsession_message`), `GET /api/v1/teams/:id/tasks`, `POST /api/v1/teams/:id/stop`, `POST /api/v1/teams/:id/resume`, `GET /api/v1/teams/:id/events` (SSE of the lead's `team.*` events plus members' request events, for the team view), `GET /api/v1/team-definitions`. All registered in `http/openapi.rs`; `scripts/generate_sdk.py` regenerates the TypeScript SDK. Errors: `FeatureDisabledError` (404), `TeamLimitError` (409), `TaskClaimedError` (409), `TaskDependencyError` (409).

### 15. CLI

`cyber teams list [--format json]`, `cyber teams show <team|name> [--format json]` (members with status, current task, cost, last message; task board by status), `cyber teams stop <team>`, `cyber teams resume <team> [--lead <session>]`, `cyber teams definitions`. `cyber features list|enable|disable` (section 1).

### 16. TUI

- `/team start <name>`, `/team stop`, `/team resume`, `/team` (opens the team view); added to `BUILTIN_COMMANDS` (the guard test enforces it).
- **Team view** (`view/team.rs`): a members table (name, role, status badge `working`/`idle`/`waiting_input`/`stopped`, current task, cost, last message) and a task board with `todo`, `in_progress`, `blocked`, `done` columns (cancelled hidden behind `c`), owners shown on each card. `enter` on a member opens its Session; `esc` returns to the lead. It refreshes from `GET /teams/:id/events`.
- **Sidebar:** a `Team` section on lead and teammate Sessions with member status dots.
- **Labelled requests:** permission and question requests from teammates appear in the lead's TUI with the prefix `[tests]` and are answered there; answering routes the reply to the teammate's request ID.
- Snapshots via `insta` in `screens_team.rs`; NO_COLOR and screen-reader fallbacks like the other views.

### 17. Permissions and security

Team tools are gated: `team_spawn`, `team_merge` (action `team.manage`, default `allow` for the lead, `deny` for teammates), task tools (`team.task`, `allow` for members). `team_merge` runs git under the normal sandbox; the merge never runs hooks from the member's branch (`-c core.hooksPath=/dev/null`). Messages between teammates keep the untrusted rendering of messaging. A teammate never gains a permission the lead does not have (section 4).

## Risks / Trade-offs

- **Lead as single writer.** All task writes go through the lead's runtime lock; contention is bounded by 7 members and a short transaction.
- **Soft budgets overshoot** by up to one Turn per member, the same trade-off Workflow Runs accept; reserved enforcement is not offered.
- **Ownership is path-based.** A bash command whose write targets the analysis cannot determine is not blocked by `owns`; the sandbox still applies. Worktree isolation (the default with two editors) avoids the issue.
- **`git merge-tree --write-tree` needs git 2.38**; older git fails clearly instead of falling back to a worktree-touching merge.

## Migration Plan

Additive migration; no backfill. Feature off by default, so existing users see no change. Archive after `implement-cross-session-messaging`.

## Open Questions

None blocking. P3 distributed teammates will add `runner` resolution over peers and a git-bundle fetch in `team_merge`.
