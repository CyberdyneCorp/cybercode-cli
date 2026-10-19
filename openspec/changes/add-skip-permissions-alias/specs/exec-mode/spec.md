## MODIFIED Requirements

### Requirement: Model, agent and mode selection
(P0) The system SHALL accept `--model provider/model[#variant]`, `--agent <name>` and `--mode <mode>`. The default mode for exec Sessions SHALL be `dont-ask`: actions allowed by rules run, and anything that would prompt is denied and logged. `--auto` SHALL select `auto` mode. `--yolo` SHALL select `bypass` and print a warning to stderr, and it SHALL be refused when org policy disables bypass. `--dangerously-skip-permissions` SHALL select `bypass` together with the override of `--dangerously-bypass-permissions` and print a warning to stderr; like that flag it SHALL require `--embedded` or `--ephemeral`, failing otherwise with exit code 2 and a message naming them, and it SHALL fail with exit code 2 when combined with `--auto` or a `--mode` other than `bypass`. It SHALL be refused when org policy disables bypass. An unknown agent or a subagent-only agent SHALL fail with exit code 2.

#### Scenario: Default dont-ask
- **WHEN** an exec Session with no mode flag hits an `edit` that would require `ask`
- **THEN** the edit is denied without blocking, a `permission_denied` event is emitted, and the model receives the denial as a tool error

#### Scenario: Bypass disabled by policy
- **WHEN** org policy disables bypass and a user runs `cyber exec --yolo "deploy"`
- **THEN** the command prints `bypass mode is disabled by your organization` and exits with code 2

#### Scenario: Claude Code's skip flag
- **WHEN** a user runs `cyber exec --embedded --dangerously-skip-permissions "fix it"` on a host with no sandbox or container
- **THEN** the Session runs in `bypass`

#### Scenario: Skip flag on the shared server
- **WHEN** a user runs `cyber exec --dangerously-skip-permissions "fix it"` without `--embedded` or `--ephemeral`
- **THEN** the command prints `--dangerously-skip-permissions needs --embedded or --ephemeral` and exits with code 2 without creating a Session
