## MODIFIED Requirements

### Requirement: Feature flag
(P2) Agent teams SHALL be available only when the feature `teams` is enabled (`features.teams: true`, `--features teams` or `CYBER_FEATURES=teams`; `experimental.teams` is accepted as a deprecated alias with a warning). Without it, no team state SHALL be created and the `team_*` tools SHALL NOT be offered.

#### Scenario: Disabled by default
- **WHEN** a user asks for a team without enabling the flag
- **THEN** no teammates are spawned and the model is told teams are disabled

## ADDED Requirements

### Requirement: Distributed teammates
(P3) A teammate definition MAY set `runner` (pool, `rnr_` ID or peer name). Such teammates SHALL run as remote Sessions (`runners-cloud`) or on the peer, each in its own worktree of the same repository. The Team's task list and message history SHALL be hosted by the lead's server and exposed to remote teammates through the task and messaging APIs over the Runner or peer channel; the lead's server SHALL be the single writer of team state. `team_merge` SHALL fetch a remote teammate's branch as a git bundle before merging. A teammate whose Runner goes offline SHALL be marked `offline` in the team view and its tasks returned to `todo` after `teams.offline_grace_seconds` (default 300).

#### Scenario: Teammates on two machines
- **WHEN** a team definition places `api` on `runner: "desk"` and `ui` locally
- **THEN** both share one task board hosted by the lead, message each other by name, and the lead can merge `api`'s branch

### Requirement: Teammate panes
(P4) `teams.display` SHALL accept `inline` (default: the team view inside the TUI) and `tmux` or `iterm2`, which open one pane per teammate attached to that teammate's Session, created and closed with the Team.

#### Scenario: tmux panes
- **WHEN** `teams.display` is `tmux` and a team with three teammates starts inside tmux
- **THEN** three panes open, each attached to one teammate, and they close on `/team stop`
