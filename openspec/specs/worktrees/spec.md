# worktrees Specification

## Purpose
Gives parallel Sessions, subagents and Workflow agents their own git checkouts so their edits never collide. It covers creation, naming, copying untracked files, sparse checkouts, cleanup, and a hook-based fallback for non-git version control. Draws on Claude Code's `--worktree`, `.worktreeinclude` and subagent `isolation: worktree`, Codex's per-chat worktrees, and OpenCode v1's managed worktrees.

## Requirements

### Requirement: Session worktrees
(P1) `cyber --worktree [name]` and `cyber exec --worktree [name]` SHALL create, or reuse, a managed worktree and start the Session in it. When `name` is omitted it SHALL be generated as `<adjective>-<noun>-<4 hex>`. The Session's Location SHALL be the worktree path.

#### Scenario: Start in a new worktree
- **WHEN** the user runs `cyber --worktree fix-login`
- **THEN** a worktree `fix-login` is created on branch `cyber/fix-login` and the TUI opens with that path as its Location

### Requirement: Worktree location and branch naming
(P1) Managed worktrees SHALL be created at `worktrees.root`, defaulting to `<data>/worktrees/<project_id>/<name>`, on a new branch `worktrees.branch_prefix` + name (default `cyber/`), from base `worktrees.base` (default the current `HEAD`). Names SHALL match `^[a-z0-9][a-z0-9._-]{0,62}$`.

#### Scenario: Custom root
- **WHEN** config sets `worktrees.root` to `../wt`
- **THEN** worktrees are created under `../wt/<name>` relative to the repository root

### Requirement: Untracked file inclusion
(P1) After creating a worktree, the system SHALL copy untracked files matching patterns in `.worktreeinclude` (gitignore syntax, read from the repository root) from the main checkout. Typical entries are `.env` and `config/local.*`. Copies SHALL never overwrite tracked files.

#### Scenario: .env copied
- **WHEN** `.worktreeinclude` contains `.env*` and the main checkout has `.env.local`
- **THEN** the new worktree contains a copy of `.env.local`

### Requirement: Setup commands
(P1) After creation the system SHALL run `worktrees.setup` commands (for example `["npm ci"]`) in the worktree inside the sandbox, streaming output to the Session. A failing setup SHALL be reported but SHALL NOT delete the worktree.

#### Scenario: Setup failure reported
- **WHEN** `npm ci` fails during setup
- **THEN** the Session shows the failure and stays in the worktree

### Requirement: Subagent isolation
(P2) An agent definition or subagent call with `isolation: "worktree"` SHALL run the child Session in a fresh managed worktree branched from the parent's current `HEAD` plus uncommitted changes, which are committed to a temporary ref first. On completion the result SHALL include `worktree_path`, `branch` and a diff summary, and the parent SHALL decide whether to merge.

#### Scenario: Isolated subagent result
- **WHEN** a subagent with `isolation: "worktree"` finishes after editing 3 files
- **THEN** the parent receives the branch name and a diff summary of the 3 files

### Requirement: Workflow agent isolation
(P2) Workflow agents launched with `{ isolation: "worktree" }` SHALL each receive their own managed worktree named `run-<id>-<index>` on branch `cyber/run-<id>/<index>`, where `<id>` is the lowercase ULID of the `run_` ID and `<index>` is the agent's 1-based start ordinal in the run, so names satisfy the managed worktree name pattern. Worktrees from completed runs SHALL be retained until the run is archived or `cyber worktree prune` runs.

#### Scenario: Parallel migration agents
- **WHEN** a workflow fans out 10 agents with worktree isolation
- **THEN** 10 distinct worktrees exist and no two agents write to the same checkout

#### Scenario: Valid worktree names
- **WHEN** run `run_01JABCDEFGHJKMNPQRSTVWXYZ0` starts its third isolated agent
- **THEN** the worktree is named `run-01jabcdefghjkmnpqrstvwxyz0-3` on branch `cyber/run-01jabcdefghjkmnpqrstvwxyz0/3`

### Requirement: Enter and exit tools
(P1) The system SHALL provide `enter_worktree { name?, path? }`, which creates or switches the current Session into a worktree and changes the Location, and `exit_worktree { keep?: boolean }`, which returns to the original Location. Both SHALL be gated by the `worktree` permission. `exit_worktree` SHALL be unavailable to Sessions that were started isolated.

#### Scenario: Model enters a worktree
- **WHEN** the model calls `enter_worktree` with name `spike`
- **THEN** subsequent tool calls resolve paths in the `spike` worktree

### Requirement: Cleanup on exit
(P1) When a Session that created a worktree ends, the system SHALL remove the worktree and delete its branch if it has no uncommitted changes and no commits beyond its base. Otherwise it SHALL keep both and print the path and branch. `worktrees.cleanup` SHALL accept `auto` (default), `keep`, or `ask`.

#### Scenario: Unchanged worktree removed
- **WHEN** a Session in a fresh worktree exits without changes
- **THEN** the worktree directory and branch are removed

### Requirement: Worktree management commands
(P1) The CLI SHALL provide `cyber worktree list` (name, path, branch, ahead/behind, dirty, owning sessions), `cyber worktree remove <name> [--force]`, and `cyber worktree prune [--older-than <days>]` (default 14). The server SHALL expose `GET/POST/DELETE /api/v1/worktrees`.

#### Scenario: Dirty worktree removal refused
- **WHEN** the user runs `cyber worktree remove spike` and it has uncommitted changes
- **THEN** the command fails unless `--force` is given

### Requirement: Sparse checkouts
(P2) When `worktrees.sparse` lists path patterns, or a subagent passes `sparse: [...]`, the system SHALL create the worktree with cone-mode sparse checkout limited to those paths plus root files.

#### Scenario: Monorepo package only
- **WHEN** `sparse: ["packages/api"]` is set
- **THEN** the worktree contains `packages/api` and root files only

### Requirement: Concurrency safety
(P1) Worktree creation and removal SHALL be serialized per repository with a cross-process file lock at `<repo git dir>/cyber-worktree.lock`. The system SHALL refuse to remove a worktree that is the Location of a running Session.

#### Scenario: Remove while in use
- **WHEN** `cyber worktree remove spike` runs while a Session works in `spike`
- **THEN** it fails with `Worktree in use by ses_<id>`

### Requirement: Non-git version control
(P2) For Locations not under git, worktree operations SHALL be delegated to `hooks` events `WorktreeCreate` and `WorktreeRemove`. The hook SHALL return `{ path }`. Without such hooks, `isolation: "worktree"` SHALL fail with `Worktrees require git or WorktreeCreate hooks`.

#### Scenario: Jujutsu via hook
- **WHEN** a `WorktreeCreate` hook runs `jj workspace add` and returns a path
- **THEN** the subagent runs in that path

### Requirement: Worktree events
(P1) The system SHALL publish `worktree.created.1`, `worktree.removed.1` and `worktree.kept.1` with `{ name, path, branch, session_id? }`.

#### Scenario: Kept worktree announced
- **WHEN** a dirty worktree is kept on Session exit
- **THEN** `worktree.kept.1` is published with its path and branch

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

### Requirement: Explicit child setup recovery
(P1) Clients SHALL inspect a pending isolated child's setup journal through its direct parent and explicitly authorize retry of a command with an acknowledged nonzero or signal exit or a verified preparation failure before process launch against the reviewed recipe digest and journal revision. Successful commands SHALL not rerun. Recovery SHALL verify current native ownership, hold child execution and checkout leases, retain source trust and current deny/sandbox ceilings, and clear setup-pending only after complete setup. Pending unknown outcomes and recorded execution errors without an acknowledged exit SHALL not authorize retry or settlement by this operation. A dispatch revision SHALL prevent a stale acknowledgement from settling a newer attempt.

#### Scenario: Retry a reviewed settled failure
- **GIVEN** an isolated child is setup-pending after an acknowledged failed exit
- **WHEN** its parent client submits the reviewed recipe digest, journal revision, failed index and a nonempty reason
- **THEN** a durable retry authorization preserves the prior result and permits one fresh attempt at that failed step
- **AND** prior successful commands replay without side effects and subsequent commands run in order
- **AND** completion clears the exact child's setup gate without starting inference

#### Scenario: Retry a command that failed before launch
- **WHEN** sandbox command preparation fails before the setup process launch boundary
- **THEN** the journal SHALL record a distinct not-dispatched result with its diagnostic
- **AND** an explicit retry against the reviewed recipe and revision SHALL preserve earlier successes and authorize one new attempt
- **AND** errors from process launch or later execution and historical undifferentiated failures SHALL remain fenced as uncertain outcomes

#### Scenario: Refuse stale or unsafe recovery
- **WHEN** the reviewed recipe or revision changes, the child is not owned by the parent, a command outcome is unknown, or native ownership/activity is uncertain
- **THEN** recovery refuses before command dispatch or readiness acknowledgement
- **AND** user files, earlier results and setup-pending state remain intact

### Requirement: Worktree Location stream continuity
(P1) When a durable worktree rebound changes a Session's Location, its Session stream SHALL continue using the same aggregate cursor and SHALL label replay/live envelopes with the Location at each event's sequence. Instance streams SHALL deliver the rebound to subscribers at both previous and new Locations, then route subsequent events to the new Location. Stream gaps and reconnects SHALL recover Location from durable history. TUI actions SHALL use the current Session Location, including immediately after an observed rebound.

#### Scenario: Attached client observes relocation
- **WHEN** an isolated child rebounds to a recreated checkout at a different path
- **THEN** old and new Location subscribers each receive the same rebound identity and its new Location
- **AND** subsequent events use the new Location and do not leak to the old Location stream
- **AND** the Session stream keeps its aggregate sequence without duplicate durable events

#### Scenario: Reconnect across a Location change
- **WHEN** a client reconnects before or after a rebound event using its last durable cursor
- **THEN** replay labels earlier events with their original Location and later events with the rebound Location
- **AND** historical payloads remain unchanged

#### Scenario: TUI acts after rebinding
- **WHEN** the active Session receives a valid worktree rebound
- **THEN** its directory updates immediately and a server refresh is requested
- **AND** subsequent located requests and local attachment paths use the new directory

#### Scenario: Uncached deletion preserves other subscriptions
- **WHEN** an all-scope instance stream first observes a Session through its deletion notification
- **THEN** it delivers deletion with an unknown Location omitted
- **AND** unrelated Session events continue on that connection

#### Scenario: Cached canonical Location remains stable
- **WHEN** a Session's source directory alias disappears after its instance stream has resolved that Location
- **THEN** cached routing remains at the resolved canonical Location until a durable binding change
- **AND** ordinary notifications do not recanonicalize the path on every event
