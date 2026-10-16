## MODIFIED Requirements

### Requirement: Workflow agent isolation
(P2) Workflow agents launched with `{ isolation: "worktree" }` SHALL each receive their own managed worktree named `run-<id>-<index>` on branch `cyber/run-<id>/<index>`, where `<id>` is the lowercase ULID of the `run_` ID and `<index>` is the agent's 1-based start ordinal in the run, so names satisfy the managed worktree name pattern. Worktrees from completed runs SHALL be retained until the run is archived or `cyber worktree prune` runs.

#### Scenario: Parallel migration agents
- **WHEN** a workflow fans out 10 agents with worktree isolation
- **THEN** 10 distinct worktrees exist and no two agents write to the same checkout

#### Scenario: Valid worktree names
- **WHEN** run `run_01JABCDEFGHJKMNPQRSTVWXYZ0` starts its third isolated agent
- **THEN** the worktree is named `run-01jabcdefghjkmnpqrstvwxyz0-3` on branch `cyber/run-01jabcdefghjkmnpqrstvwxyz0/3`
