## MODIFIED Requirements

### Requirement: Worktree management commands
(P1) The CLI SHALL provide `cyber worktree list` (name, path, branch, ahead/behind, dirty, owning sessions), `cyber worktree remove <name> [--force]`, and `cyber worktree prune [--older-than <days>]` (default 14). The server SHALL expose `GET/POST/DELETE /api/v1/worktrees`. Ownership inspection SHALL hold the repository lifecycle lock, verify ready records and retain pending or invalid records as recovery diagnostics without repairing or deleting them.

#### Scenario: Dirty worktree removal refused
- **WHEN** the user runs `cyber worktree remove spike` and it has uncommitted changes
- **THEN** the command fails unless `--force` is given

#### Scenario: Listing preserves recovery evidence
- **WHEN** a repository contains ready, pending or malformed ownership records
- **THEN** listing returns ready records only after verifying their repository, branch and registration
- **AND** pending and invalid records remain visible with their distinct recovery status
- **AND** listing preserves user edits and ownership records

#### Scenario: Listing refuses unsafe records and contention
- **WHEN** the repository lifecycle lock is held or an ownership record is oversized, symlinked or inconsistent with its name and repository
- **THEN** contention returns a retryable busy error and unsafe records receive invalid-record diagnostics
- **AND** a symlinked ownership directory is refused without following it


### Requirement: Session worktrees
(P1) `cyber --worktree [name]` and `cyber exec --worktree [name]` SHALL create, or reuse, a managed worktree and start the Session in it. When `name` is omitted it SHALL be generated as `<adjective>-<noun>-<4 hex>`. The Session's Location SHALL be the worktree path.

#### Scenario: Start in a new worktree
- **WHEN** the user runs `cyber --worktree fix-login`
- **THEN** a worktree `fix-login` is created on branch `cyber/fix-login` and the TUI opens with that path as its Location

#### Scenario: Explicit user startup preserves permission rules
- **WHEN** an authenticated client explicitly requests a new managed-worktree Session
- **THEN** its request authorizes that creation/setup operation without persisting a broader Session permission rule or changing the selected mode
- **AND** denied rules, plan mode and read-only sandbox policy still refuse creation
- **AND** startup does not create an additional source Session

#### Scenario: API startup streams and preserves failure
- **WHEN** a client posts a valid creation request to `/api/v1/worktrees`
- **THEN** setup output uses its creation call ID on the live event stream before completion
- **AND** the response identifies the actual worktree Location, retained Session and setup outcome
- **AND** replay of the same idempotency key returns that outcome without redispatching side effects

#### Scenario: Startup keeps source checkout trust scoped
- **WHEN** the source checkout has approved setup definitions and startup creates a new checkout
- **THEN** the already resolved source recipe executes as part of creation
- **AND** source-defined credential environment names remain filtered under the target sandbox policy, including names from disabled providers
- **AND** explicit environment allow rules and user-selected full access keep their existing meaning
- **AND** the new checkout receives no trust approval from that operation
- **AND** unapproved source setup definitions remain inactive

#### Scenario: Startup selection is unambiguous
- **WHEN** a client combines worktree startup with Session resume/fork or ephemeral exec
- **THEN** the CLI rejects the combination before worktree creation
- **AND** named and generated worktree startup remain available for fresh persistent Sessions

### Requirement: Setup commands
(P1) After creation the system SHALL run `worktrees.setup` commands (for example `["npm ci"]`) in the worktree inside the sandbox, streaming output to the Session. A failing setup SHALL be reported but SHALL NOT delete the worktree.

#### Scenario: New Session creation runs setup automatically
- **WHEN** the runtime creates a new Session through its managed-worktree creation entry point
- **THEN** creation uses a cancellation-owned sandboxed Git port and the Session starts at the verified ready worktree
- **AND** configured setup runs automatically with its progress attributed to that Session
- **AND** setup failure returns the retained Session and worktree with the failed outcome

#### Scenario: Setup failure reported
- **WHEN** `npm ci` fails during setup
- **THEN** the Session shows the failure and stays in the worktree

#### Scenario: Ordered streaming setup
- **WHEN** trusted resolved configuration supplies multiple setup commands
- **THEN** commands execute sequentially through the runtime's sandboxed, cancellation-owned execution port in the verified owned worktree
- **AND** stdout and stderr chunks reach the Session as they arrive
- **AND** the first nonzero or signal termination stops subsequent commands

#### Scenario: Interrupted setup preserves user work
- **WHEN** setup execution fails or its owning future is cancelled
- **THEN** the worktree and files already written remain intact and the repository lock is released
- **AND** the runtime settles the process tree before retry or cleanup

#### Scenario: Setup remains scoped to managed roots
- **WHEN** setup runs with workspace-write policy
- **THEN** it can write in the owned worktree and its managed temporary/output directories
- **AND** writing to an unrelated sibling directory is denied unless explicitly configured as a writable root
- **AND** protected paths and credential filtering remain enforced

#### Scenario: Setup stream delivery fails
- **WHEN** delivery of a live stdout or stderr chunk fails
- **THEN** the command's owned process tree is terminated and the failure is reported without deleting the worktree

#### Scenario: Setup leader exits before descendants
- **WHEN** a setup leader exits while its descendants still hold stdout or stderr pipes
- **THEN** the descendants are terminated and setup settles with the leader's exit code

#### Scenario: Repeated setup reuses durable results
- **WHEN** a setup request repeats the same owned worktree and command recipe after settlement
- **THEN** recorded command results are reused without redispatching side effects
- **AND** a nonzero, signal or execution failure still stops later commands

#### Scenario: Setup intent survives process loss
- **WHEN** a setup command's durable intent exists without a settled result after process loss or future disposal
- **THEN** a new setup request refuses redispatch with an outcome-unknown recovery diagnostic
- **AND** changing or emptying the command recipe cannot hide that intent

#### Scenario: Setup command admission races
- **WHEN** two lifecycle owners attempt to admit the same setup command
- **THEN** optimistic concurrency permits at most one durable command intent and dispatch authorization

#### Scenario: Recreated worktree does not reuse earlier setup
- **WHEN** an owned worktree is removed and a new one is created with the same name and path
- **THEN** its new persistent creation ID gives setup an independent journal
- **AND** legacy ownership without a creation ID requires recovery before journaled setup

#### Scenario: Attached Session receives live setup output
- **WHEN** setup runs for an existing Session located in the owned worktree
- **THEN** its attached clients receive `session.worktree.setup` updates on the Location instance stream before command completion
- **AND** updates identify the Session, worktree creation ID, call and command index, with distinct stdout and stderr channels
- **AND** arbitrary output bytes are carried as bounded base64 chunks without including setup command source
- **AND** other Locations remain filtered from a Location-scoped stream

#### Scenario: Runtime closes during Session setup
- **WHEN** the runtime shuts down during an owned setup command
- **THEN** shutdown cancels its owned token and waits for settlement or bounded future disposal, stopping its process tree without removing the worktree
- **AND** an intent without acknowledged settlement remains outcome unknown for recovery

#### Scenario: Setup ownership mismatch
- **WHEN** supplied managed ownership differs from the persisted ready record
- **THEN** setup fails before any command is dispatched

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

#### Scenario: Creation permission and read-only admission
- **WHEN** worktree creation is denied, its token is already cancelled, or the sandbox policy is read-only
- **THEN** the runtime refuses before reserving ownership or creating the target
- **AND** creation uses the persisted source Session's mode and rules rather than invocation overrides

#### Scenario: Git creation scope is distinct from setup scope
- **WHEN** managed Git creation runs with workspace-write policy
- **THEN** only shared repository metadata, the reserved target, managed temporary/output directories and explicitly configured writable roots are writable
- **AND** ordinary source checkout files and unrelated sibling worktrees are not implicitly writable
- **AND** setup receives its normal protected-path policy without the Git metadata provisioning grants
