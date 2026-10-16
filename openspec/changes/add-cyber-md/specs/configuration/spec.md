## MODIFIED Requirements

### Requirement: Global config directory
(P0) The system SHALL use `$XDG_CONFIG_HOME/cyber` (default `~/.config/cyber`), or `CYBER_CONFIG_DIR` when set, as the global config directory, holding `cyber.json`, `cyber.jsonc`, `tui.jsonc`, `AGENTS.md`, `CYBER.md` and the supplemental subdirectories. On first run, when no global document exists, the system SHALL create `cyber.jsonc` containing only `{"$schema": "https://cyber-code.dev/schema/config.json"}`.

#### Scenario: First run bootstrap
- **WHEN** `~/.config/cyber` does not exist and the user runs `cyber`
- **THEN** the directory and a `cyber.jsonc` with only `$schema` are created
