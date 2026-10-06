## MODIFIED Requirements

### Requirement: Untracked file inclusion
(P1) After creating a worktree, the system SHALL copy untracked files matching patterns in `.worktreeinclude` (gitignore syntax, read from the repository root) from the main checkout. Typical entries are `.env` and `config/local.*`. Copies SHALL never overwrite tracked files.

#### Scenario: .env copied
- **WHEN** `.worktreeinclude` contains `.env*` and the main checkout has `.env.local`
- **THEN** the new worktree contains a copy of `.env.local`

#### Scenario: Ignored environment file is included
- **WHEN** `.worktreeinclude` selects an untracked `.env.local` that Git otherwise ignores
- **THEN** the new worktree contains its content and file permissions
- **AND** negated patterns remain excluded and destination tracked files remain unchanged

#### Scenario: Existing untracked destination is preserved
- **WHEN** a selected destination file already exists during initial inclusion
- **THEN** inclusion fails without overwriting it and creation remains pending

#### Scenario: Inclusion cannot escape either checkout
- **WHEN** a selected source is a symlink or Git enumeration returns an unsafe relative path
- **THEN** inclusion fails without copying outside the source or worktree roots

#### Scenario: Reuse preserves copied user edits
- **WHEN** a previously included file is edited in a ready worktree
- **THEN** reuse does not recopy that file from the main checkout

#### Scenario: Creation from a linked checkout still uses primary inclusion sources
- **WHEN** a managed worktree is created from an existing linked checkout
- **THEN** `.worktreeinclude` and selected untracked files are read from the primary checkout
- **AND** edits in the existing linked checkout are preserved

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

#### Scenario: Windows canonical path reaches Git safely
- **WHEN** Rust resolves a managed worktree root to a Windows verbatim drive path
- **THEN** only the explicit Git argument is converted to an equivalent supported spelling, with command-local long-path support
- **AND** canonical ownership remains unchanged and ambiguous path components fail before branch creation
- **AND** Windows process creation uses a short launch directory while Git selects the requested directory explicitly, without broadening sandbox access
- **AND** long linked targets use verified metadata and an explicit checkout destination without an early long-directory change

#### Scenario: Linked metadata no longer points back to the owned worktree
- **WHEN** the metadata directory's `gitdir` backpointer no longer resolves to the owned worktree's regular `.git` marker
- **THEN** creation verification or reuse fails without accepting the redirected repository
- **AND** existing user files remain unchanged

#### Scenario: Initial checkout does not overwrite a concurrent file
- **WHEN** a file appears in the fresh worktree after registration but before initial checkout
- **THEN** checkout fails without overwriting that file
- **AND** pending ownership remains available for recovery
