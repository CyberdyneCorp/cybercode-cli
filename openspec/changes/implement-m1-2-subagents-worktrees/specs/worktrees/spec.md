## MODIFIED Requirements

### Requirement: Worktree location and branch naming
(P1) Managed worktrees SHALL be created at `worktrees.root`, defaulting to `<data>/worktrees/<project_id>/<name>`, on a new branch `worktrees.branch_prefix` + name (default `cyber/`), from base `worktrees.base` (default the current `HEAD`). Names SHALL match `^[a-z0-9][a-z0-9._-]{0,62}$`.

#### Scenario: Custom root
- **WHEN** config sets `worktrees.root` to `../wt`
- **THEN** worktrees are created under `../wt/<name>` relative to the repository root

#### Scenario: Reject unsafe worktree names
- **WHEN** a worktree name contains separators, is empty, exceeds 63 ASCII characters, or does not match the required syntax
- **THEN** validation fails before any filesystem or Git operation

#### Scenario: Validate settings after trust
- **WHEN** resolved worktree configuration has an invalid cleanup policy or non-string setup command
- **THEN** configuration loading fails with a worktrees-specific diagnostic
- **AND** untrusted project setup definitions remain inactive until approved
