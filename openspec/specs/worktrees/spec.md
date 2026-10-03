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
(P2) Workflow agents launched with `{ isolation: "worktree" }` SHALL each receive their own managed worktree named `run_<id>-<index>`. Worktrees from completed runs SHALL be retained until the run is archived or `cyber worktree prune` runs.

#### Scenario: Parallel migration agents
- **WHEN** a workflow fans out 10 agents with worktree isolation
- **THEN** 10 distinct worktrees exist and no two agents write to the same checkout

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
