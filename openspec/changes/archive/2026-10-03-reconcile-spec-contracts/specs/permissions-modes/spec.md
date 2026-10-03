## MODIFIED Requirements

### Requirement: Default rules
(P0) Without configuration the system SHALL apply: `* ask`; `read`, `glob`, `grep` and `list` allow within the Location; `edit`, `bash` and external mutations ask; `external_directory ask` except the tool-output, jobs, temp, skill and worktree directories; `read` and `edit` `ask` for `*.env` and `*.env.*` but `allow` for `*.env.example`; `question`, `plan_enter` and `plan_exit` allow for primary agents (`mode` `primary` or `all`) and deny for subagent-only and hidden agents; `doom_loop ask`; `message.send ask` for cross-machine targets; `workflow.run ask`; `remote.attach ask`.

#### Scenario: Reading .env asks
- **WHEN** the model reads `.env.local` with default rules
- **THEN** a permission request is raised

#### Scenario: Subagent cannot ask questions
- **WHEN** an `explore` subagent calls `question`
- **THEN** the call is denied and the subagent is told to return its best answer instead

### Requirement: Protected paths
(P0) The system SHALL treat these as protected for mutation: `.git/` (the repository metadata directory, not the working tree); the configuration documents `cyber.json`, `cyber.jsonc`, `.cyber/cyber.jsonc`, `.cyber/cyber.local.jsonc`, `.cyber/hooks.jsonc`, `.cyber/mcp.json`, `.cyber/plugins.json`, `.cyber/plugins.local.json`, `.cyber/plugins.lock` and `.cyber/plugins/`; shell startup files (`~/.bashrc`, `~/.zshrc`, `~/.profile`, `~/.config/fish/config.fish`); `~/.ssh/`, `~/.gnupg/`, `~/.aws/credentials` and `~/.config/cyber/`. The `.cyber/` content directories `plans/`, `workflows/`, `agents/`, `skills/`, `commands/`, `teams/`, `output-styles/` and files `.cyber/import-report-*.md` SHALL NOT be protected; they follow ordinary `edit` rules so the system can write plans, saved workflows, generated agents and recorded skills. Mutating a protected path SHALL require `ask` in every Mode, including `bypass` and `auto`, unless an explicit config rule names the exact path with `allow`. The `sandbox` capability SHALL enforce the same partition for OS-level writes.

#### Scenario: Bypass still asks for .ssh
- **WHEN** a Session in `bypass` mode attempts to write `~/.ssh/config`
- **THEN** a permission request is raised

#### Scenario: Plan file is ordinary content
- **WHEN** a `plan`-mode Session writes `.cyber/plans/auth-refactor.md`
- **THEN** no protected-path prompt is raised and the write succeeds under plan-mode rules

#### Scenario: Hook definition stays protected
- **WHEN** an `accept-edits` Session edits `.cyber/hooks.jsonc`
- **THEN** a permission request is raised

### Requirement: Permission modes
(P0) The system SHALL support Modes `default` and `plan` in P0, adding `accept-edits`, `auto`, `dont-ask` and `bypass` in P1, selectable per Session (`--mode`, `mode` config, agent `permission_mode`, `/mode <name>`, `POST /api/v1/sessions/:id/mode`). The TUI SHALL cycle Modes with Shift+Tab through `default → accept-edits → plan → auto → default` on P1 builds (`default → plan → default` on P0 builds); `bypass` and `dont-ask` SHALL be reachable only through `/mode`, flags or config. A Mode change SHALL apply at the next Turn and publish `session.mode.switched.1`, as specified by session-runtime. The UI SHALL show a requested change as pending until effective; interrupt SHALL be offered when immediate cancellation is needed.

#### Scenario: Cycling modes
- **WHEN** the user presses Shift+Tab twice from `default` on a P1 build
- **THEN** the Session mode becomes `plan`

### Requirement: plan mode
(P0) In `plan`, the system SHALL allow only tools annotated `read_only`, plus writing the plan file `<location>/.cyber/plans/<session-slug>.md`, and SHALL deny every other mutating action with `Plan mode is read-only. Present the plan with plan_exit.` The system SHALL provide the tools `plan_enter {}` (switch the Session to `plan` at the next Turn) and `plan_exit { summary }` (present the plan file for approval). `plan_exit` SHALL route a question to the user with the choices `Approve and build (default)`, `Approve with accept-edits`, `Approve with auto` (only when `auto` is available) and `Request changes`. An approval SHALL switch the Mode to the chosen one at the next Turn; `Request changes` SHALL return the user's feedback as the tool result and keep `plan`. In non-interactive Sessions both tools SHALL be denied. Planning is a Mode, not an agent: there is no built-in `plan` agent.

#### Scenario: Write blocked in plan mode
- **WHEN** a `plan` Session calls `edit` on `src/lib.rs`
- **THEN** the call is denied with `Plan mode is read-only. Present the plan with plan_exit.`

#### Scenario: Plan approved with accept-edits
- **WHEN** the model calls `plan_exit` and the user chooses `Approve with accept-edits`
- **THEN** the Session's Mode becomes `accept-edits` at the next Turn and `session.mode.switched.1` is published

#### Scenario: Changes requested
- **WHEN** the user chooses `Request changes` and types `split the migration into two steps`
- **THEN** the tool returns that feedback to the model and the Session stays in `plan`

## REMOVED Requirements

### Requirement: Subagent permission inheritance
**Reason**: Duplicate of the requirement with the same name in `agents-subagents`, which owns the contract (P1, strict Mode ceiling). The two copies disagreed on phase and on whether a child could exceed the parent's Mode.
**Migration**: Reference `agents-subagents` → "Subagent permission inheritance".
