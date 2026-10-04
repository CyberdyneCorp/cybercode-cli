## MODIFIED Requirements

### Requirement: Global flags
(P0) Every command SHALL accept `--help`/`-h`, `--version`/`-V`, `--model`/`-m <provider/model[#variant]>`, `--agent <name>`, `--mode <default|accept-edits|plan|auto|dont-ask|bypass>`, `--cwd <path>`, `--add-dir <path>` (repeatable; `permissions-modes` → Additional working directories), `--profile <name>`, `--config <key=value>` (repeatable, long form only because `-c` is `--continue`; `configuration` → Command-line overrides), `--features <name,...>`, `--allow <rule>` and `--deny <rule>` (repeatable; Session ruleset entries `action[:resource]`), `--system-prompt <text|@file>` and `--append-system-prompt <text|@file>` (replace or extend the agent's `system` for the new Session), `--mcp-config <file>` (repeatable; adds MCP servers for the Session only, as `editor-integration` does for ACP clients), `--format <table|json|text|stream-json>` (the only output-format flag), `--print-logs`, `--log-level <trace|debug|info|warn|error>` and `--pure`. `--cwd` SHALL be the only working-directory flag. `--pure` SHALL disable external plugins, hooks, channels and MCP servers that are not managed by org policy. Flags SHALL take precedence over environment variables, and environment variables over config.

#### Scenario: Flag overrides config model
- **WHEN** `cyber.jsonc` sets `model` to `anthropic/claude-sonnet` and the user runs `cyber exec -m openai/gpt-6 "hi"`
- **THEN** the session uses `openai/gpt-6`

#### Scenario: Pure mode
- **WHEN** the user runs `cyber --pure`
- **THEN** no external plugin process is spawned, no user hooks run, and `cyber debug info` reports `external extensions disabled (--pure)`

#### Scenario: Inline allow rule for CI
- **WHEN** CI runs `cyber exec --allow "bash:npm test *" --deny "bash:git push *" "fix the tests"`
- **THEN** `npm test` runs without prompting, `git push` is denied, and both rules appear in the Session ruleset
