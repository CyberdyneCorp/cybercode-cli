## ADDED Requirements

### Requirement: Read-only shell classification
(P1) A shell command SHALL count as read-only (for the auto classifier and taint-aware approvals) only when every simple command is a read-only program without an environment assignment or wrapper (such as `GIT_EXTERNAL_DIFF=…`, `LD_PRELOAD=…`, `sudo`, `env`, `xargs`), without an option that writes a file or runs a command (`--output`, `--ext-diff`, `--textconv`, `--pre`, `--pre-glob`), and without a write redirection.

#### Scenario: Environment-driven diff tool
- **WHEN** a tainted run proposes `GIT_EXTERNAL_DIFF=./x git diff`
- **THEN** the command is not read-only and still asks

## MODIFIED Requirements

### Requirement: Protected paths
(P0) The system SHALL treat these as protected for mutation: `.git/` (the repository metadata directory, not the working tree); the configuration documents `cyber.json`, `cyber.jsonc`, `.cyber/cyber.jsonc`, `.cyber/cyber.local.jsonc`, `.cyber/hooks.jsonc`, `.cyber/mcp.json`, `.cyber/plugins.json`, `.cyber/plugins.local.json`, `.cyber/plugins.lock` and `.cyber/plugins/`; shell and Git startup files (`~/.bashrc`, `~/.bash_profile`, `~/.bash_login`, `~/.zshrc`, `~/.zshenv`, `~/.zprofile`, `~/.zlogin`, `~/.profile`, `~/.config/fish/config.fish`, `~/.gitconfig`, `~/.config/git/config`) and credential files (`~/.netrc`, `~/.npmrc`, `~/.aws/credentials`); `~/.ssh/`, `~/.gnupg/`; and Cyber Code's own configuration, state and data (`~/.cyber/`, `~/.config/cyber/`, `~/.local/state/cyber/`, `~/.local/share/cyber/`, and the same directories when `CYBER_HOME` or `XDG_CONFIG_HOME`/`XDG_STATE_HOME`/`XDG_DATA_HOME` move them), except the managed worktrees under `<data>/worktrees/`, which hold the user's code. On macOS and Windows, whose filesystems ignore case, the comparison SHALL ignore case. The `.cyber/` content directories `plans/`, `workflows/`, `agents/`, `skills/`, `commands/`, `teams/`, `output-styles/` and files `.cyber/import-report-*.md` SHALL NOT be protected; they follow ordinary `edit` rules so the system can write plans, saved workflows, generated agents and recorded skills. Mutating a protected path SHALL require `ask` in every Mode, including `bypass` and `auto`, unless an explicit config rule names the exact path with `allow`. The `sandbox` capability SHALL enforce the same partition for OS-level writes.

#### Scenario: Bypass still asks for .ssh
- **WHEN** a Session in `bypass` mode attempts to write `~/.ssh/config`
- **THEN** a permission request is raised

#### Scenario: Plan file is ordinary content
- **WHEN** a `plan`-mode Session writes `.cyber/plans/auth-refactor.md`
- **THEN** no protected-path prompt is raised and the write succeeds under plan-mode rules

#### Scenario: Hook definition stays protected
- **WHEN** an `accept-edits` Session edits `.cyber/hooks.jsonc`
- **THEN** a permission request is raised

#### Scenario: Case variant on macOS
- **WHEN** an `accept-edits` Session on macOS writes `.cyber/CYBER.jsonc`
- **THEN** a permission request is raised, as for `.cyber/cyber.jsonc`

#### Scenario: Managed worktree stays writable
- **WHEN** an `accept-edits` Session whose Location is a managed worktree under `<data>/worktrees/` writes `src/main.rs`
- **THEN** the write is not treated as protected, while `<data>/cyber.db` still is
