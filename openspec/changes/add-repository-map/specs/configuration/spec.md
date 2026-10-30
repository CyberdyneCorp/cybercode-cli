## MODIFIED Requirements

### Requirement: Top-level keys
(P0) The schema SHALL accept the top-level keys `$schema`, `model`, `small_model`, `model_roles`, `fallback_models`, `providers`, `agents`, `default_agent`, `mode`, `output_style` (`system-context` → Output style selection), `permissions`, `sandbox`, `hooks`, `plugins`, `mcp`, `mcp_serve` (MCP serve; global configuration only), `skills`, `commands`, `instructions`, `references`, `memory`, `browser`, `computer`, `compaction`, `snapshots`, `worktrees`, `lsp`, `repo_map` (`code-intelligence` → Repository map configuration), `formatters`, `tool_output`, `workflows`, `goals`, `teams`, `loops`, `messaging`, `peers`, `remote`, `runners`, `channels`, `account`, `share`, `telemetry`, `budgets`, `policy`, `profiles`, `default_profile`, `shell`, `attachments`, `background`, `tools`, `git`, `ide`, `autofix`, `network`, `storage`, `compat`, `services`, `features`, `experimental` (deprecated alias of `features`) and `autoupdate` (`installation-upgrade`). Collection keys SHALL use plural names, and entries SHALL use `disabled: true` to turn off an inherited entry.

#### Scenario: Disabling an inherited MCP server
- **WHEN** the global config defines `mcp.github` and the project config sets `mcp.github.disabled = true`
- **THEN** the `github` MCP server is not started for that project

#### Scenario: Invalid output style name
- **WHEN** `cyber.jsonc` sets `output_style = "Bad Style"`
- **THEN** validation fails with `ConfigInvalidError` naming `output_style`
