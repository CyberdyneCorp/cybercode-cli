## MODIFIED Requirements

### Requirement: Top-level keys
(P0) The schema SHALL accept the top-level keys `$schema`, `model`, `small_model`, `model_roles`, `fallback_models`, `fallback` (`provider-catalog` → Model fallback chain), `providers`, `agents`, `default_agent`, `mode`, `output_style` (`system-context` → Output style selection), `language` (`localization`), `permissions`, `sandbox`, `hooks`, `plugins`, `mcp`, `mcp_serve` (MCP serve; global configuration only), `skills`, `commands`, `instructions`, `instruction_limits`, `references`, `memory`, `browser`, `computer`, `compaction`, `snapshots`, `worktrees`, `lsp`, `formatters`, `tool_output`, `workflows`, `goals`, `teams`, `loops`, `messaging`, `peers`, `remote`, `runners`, `channels`, `account`, `share`, `telemetry`, `budgets`, `policy`, `profiles`, `default_profile`, `shell`, `attachments`, `background`, `tools`, `git`, `ide`, `autofix`, `network`, `storage`, `compat`, `services`, `features`, `experimental` (deprecated alias of `features`) and `autoupdate` (`installation-upgrade`). Collection keys SHALL use plural names, and entries SHALL use `disabled: true` to turn off an inherited entry.

#### Scenario: Disabling an inherited MCP server
- **WHEN** the global config defines `mcp.github` and the project config sets `mcp.github.disabled = true`
- **THEN** the `github` MCP server is not started for that project

#### Scenario: Invalid output style name
- **WHEN** `cyber.jsonc` sets `output_style = "Bad Style"`
- **THEN** validation fails with `ConfigInvalidError` naming `output_style`

#### Scenario: Layered instruction budgets preserve instruction lists
- **WHEN** a global configuration sets both instruction limits, a project overrides the total, and a selected profile or command-line override changes the file limit
- **THEN** normal member precedence SHALL determine the effective limits and `instructions` arrays SHALL retain their existing concatenation semantics
- **AND** null, noninteger, nonpositive, unknown or incompatible limit values SHALL fail validation before the configuration is usable

#### Scenario: Project budgets require workspace trust
- **WHEN** an untrusted project defines `instruction_limits`, directly or inside a profile
- **THEN** that definition SHALL remain withheld under the existing workspace-trust digest until approved, without changing trusted global limits
