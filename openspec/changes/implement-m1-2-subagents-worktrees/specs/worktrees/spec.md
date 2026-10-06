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

### Requirement: Concurrency safety
(P1) Worktree creation and removal SHALL be serialized per repository with a cross-process file lock at `<repo git dir>/cyber-worktree.lock`. The system SHALL refuse to remove a worktree that is the Location of a running Session.

#### Scenario: Remove while in use
- **WHEN** `cyber worktree remove spike` runs while a Session works in `spike`
- **THEN** it fails with `Worktree in use by ses_<id>`

#### Scenario: Lock owner exits unexpectedly
- **WHEN** a process holding the repository worktree lock terminates unexpectedly
- **THEN** another process can acquire the same lock without deleting or replacing its file

#### Scenario: Independent repository locks
- **WHEN** two processes operate on different repositories
- **THEN** a worktree lock in one repository does not prevent acquiring the other


## ADDED Requirements

### Requirement: Managed creation ownership
(P1) Managed Git worktree creation SHALL hold the shared repository lock, resolve a commit base and record pending ownership before invoking creation through the runtime's sandboxed Git execution port. It SHALL refuse unowned existing paths and preserve pending creation artifacts on failure or cancellation. Reuse SHALL verify the common repository and branch without resetting user edits.

#### Scenario: Interrupted creation remains recoverable
- **WHEN** Git creation fails after pending ownership is recorded
- **THEN** the pending record and any resulting path or branch remain available for recovery
- **AND** a later request does not blindly repeat checkout

#### Scenario: Unowned target is preserved
- **WHEN** the requested worktree target exists without managed ownership
- **THEN** creation fails before mutation and its contents remain unchanged
