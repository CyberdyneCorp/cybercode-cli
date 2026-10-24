## MODIFIED Requirements

### Requirement: Top-level keys
(P0) The schema SHALL accept the top-level keys `$schema`, `model`, `small_model`, `model_roles`, `fallback_models`, `providers`, `agents`, `default_agent`, `mode`, `output_style` (`system-context` → Output style selection), `permissions`, `sandbox`, `hooks`, `plugins`, `mcp`, `mcp_serve` (MCP serve; global configuration only), `skills`, `commands`, `instructions`, `references`, `memory`, `browser`, `compaction`, `snapshots`, `worktrees`, `lsp`, `formatters`, `tool_output`, `workflows`, `goals`, `teams`, `loops`, `messaging`, `peers`, `remote`, `runners`, `channels`, `account`, `share`, `telemetry`, `budgets`, `policy`, `profiles`, `default_profile`, `shell`, `attachments`, `background`, `tools`, `git`, `ide`, `autofix`, `network`, `storage`, `compat`, `services`, `features`, `experimental` (deprecated alias of `features`) and `autoupdate` (`installation-upgrade`). Collection keys SHALL use plural names, and entries SHALL use `disabled: true` to turn off an inherited entry.

#### Scenario: Disabling an inherited MCP server
- **WHEN** the global config defines `mcp.github` and the project config sets `mcp.github.disabled = true`
- **THEN** the `github` MCP server is not started for that project

#### Scenario: Invalid output style name
- **WHEN** `cyber.jsonc` sets `output_style = "Bad Style"`
- **THEN** validation fails with `ConfigInvalidError` naming `output_style`

### Requirement: Supplemental directory content
(P0) Each `.cyber/` directory and the global config directory SHALL contribute:
- agents from `agents/**/*.md`
- commands from `commands/**/*.md`
- skills from `skills/**/SKILL.md`
- workflows from `workflows/*.{js,ts}`
- hooks from `hooks.jsonc`
- local plugins from `plugins/*/` (each with a `cyber-plugin.json` manifest)
- output styles from `output-styles/*.md` (`system-context` → Output style definitions)

Files SHALL be processed in sorted path order. An invalid file SHALL be skipped with a diagnostic and SHALL NOT abort loading.

#### Scenario: Invalid agent file skipped
- **WHEN** `.cyber/agents/reviewer.md` has malformed YAML frontmatter
- **THEN** the agent is skipped, a diagnostic names the file, and other agents load

#### Scenario: Output style directory
- **WHEN** `~/.config/cyber/output-styles/terse.md` and `~/.config/cyber/styles/old.md` exist
- **THEN** `terse` is an available output style and `old` is not
