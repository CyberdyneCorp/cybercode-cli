## MODIFIED Requirements

### Requirement: Top-level keys
(P0) The schema SHALL accept the top-level keys `$schema`, `model`, `small_model`, `model_roles`, `fallback_models`, `providers`, `agents`, `default_agent`, `mode`, `permissions`, `sandbox`, `hooks`, `plugins`, `mcp`, `skills`, `commands`, `instructions`, `references`, `memory`, `compaction`, `snapshots`, `worktrees`, `lsp`, `formatters`, `tool_output`, `workflows`, `goals`, `loops`, `messaging`, `peers`, `remote`, `runners`, `channels`, `account`, `share`, `telemetry`, `budgets`, `policy`, `profiles`, `default_profile`, `shell`, `attachments`, `background`, `tools`, `git`, `ide`, `autofix`, `network`, `storage`, `compat`, `services`, `features`, `experimental` (deprecated alias of `features`) and `autoupdate` (`installation-upgrade`). Collection keys SHALL use plural names, and entries SHALL use `disabled: true` to turn off an inherited entry.

#### Scenario: Disabling an inherited MCP server
- **WHEN** the global config defines `mcp.github` and the project config sets `mcp.github.disabled = true`
- **THEN** the `github` MCP server is not started for that project

### Requirement: Command-line overrides
(P0) `--config <key=value>` (long form only; `-c` is `--continue`) SHALL set one config value for the process, where `key` is a dotted path and `value` is parsed as JSON5 when it parses and as a string otherwise. Overrides SHALL merge as a layer above the selected profile and below other CLI flags, SHALL be validated against the schema, and SHALL NOT be able to set `policy` or `profiles`. `cyber debug config` SHALL show the source `cli:--config` for overridden values.

#### Scenario: One-off override
- **WHEN** the user runs `cyber exec --config compaction.auto=false --config 'sandbox.allowed_domains=["example.org"]' "..."`
- **THEN** the Session runs without automatic compaction, with `example.org` allowed, and nothing is written to any config file
