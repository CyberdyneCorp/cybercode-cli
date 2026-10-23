## MODIFIED Requirements

### Requirement: Top-level keys
(P0) The schema SHALL accept the top-level keys `$schema`, `model`, `small_model`, `model_roles`, `fallback_models`, `providers`, `agents`, `default_agent`, `mode`, `permissions`, `sandbox`, `hooks`, `plugins`, `mcp`, `mcp_serve` (MCP serve; global configuration only), `skills`, `commands`, `instructions`, `references`, `memory`, `browser`, `compaction`, `snapshots`, `worktrees`, `lsp`, `formatters`, `tool_output`, `workflows`, `goals`, `teams`, `loops`, `messaging`, `peers`, `remote`, `runners`, `channels`, `account`, `share`, `telemetry`, `budgets`, `policy`, `profiles`, `default_profile`, `shell`, `attachments`, `background`, `tools`, `git`, `ide`, `autofix`, `network`, `storage`, `compat`, `services`, `features`, `experimental` (deprecated alias of `features`) and `autoupdate` (`installation-upgrade`). Collection keys SHALL use plural names, and entries SHALL use `disabled: true` to turn off an inherited entry.

#### Scenario: Disabling an inherited MCP server
- **WHEN** the global config defines `mcp.github` and the project config sets `mcp.github.disabled = true`
- **THEN** the `github` MCP server is not started for that project
