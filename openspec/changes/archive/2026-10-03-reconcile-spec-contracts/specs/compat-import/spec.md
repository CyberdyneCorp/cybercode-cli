## MODIFIED Requirements

### Requirement: Codex sources
(P1) The `codex` importer SHALL read `~/.codex/config.toml` (and `$CODEX_HOME`), `AGENTS.md` files, `~/.codex/skills` and `~/.agents/skills`, `[mcp_servers.*]` tables, `[agents]` and custom agent TOML files, `[profiles.*]` and `[model_providers.*]`. It SHALL map:
- `model` and `model_provider` → `model`
- `approval_policy` + `sandbox_mode` → `mode` + `sandbox` (`untrusted` → `default`, `on-request` → `default`, `never` + `workspace-write` → `dont-ask` with the sandbox enabled, `danger-full-access` → `bypass` with the sandbox disabled)
- `model_providers` → `providers` in the `provider-catalog` shape: `base_url` → `api.url`, `wire_api` `chat` → `api.type: "openai-compatible"` and `responses` → `api.type: "openai-responses"`, `env_key` → `api.settings.api_key: "{env:NAME}"`
- profiles → `profiles`

#### Scenario: Custom OpenAI-compatible provider
- **WHEN** `config.toml` has `[model_providers.local] base_url = "http://localhost:8000/v1" env_key = "LOCAL_KEY" wire_api = "chat"`
- **THEN** the import creates `providers.local = { "api": { "type": "openai-compatible", "url": "http://localhost:8000/v1", "settings": { "api_key": "{env:LOCAL_KEY}" } } }`

### Requirement: OpenCode sources
(P1) The `opencode` importer SHALL read `opencode.json`/`opencode.jsonc` (v1 and v2 shapes), `.opencode/` directories (agents, commands, modes, plugins, skills, tools), and the global `~/.config/opencode` directory. It SHALL map:
- `agent`/`agents` → `agents`
- `command`/`commands` → `commands`
- `permission`/`permissions` (v1 map and v2 ordered array) and legacy `tools` booleans → `permissions` (`write` and `patch` → `edit`, `task` and `subagent` → `agent`, `shell` → `bash`)
- `provider`/`providers` → `providers`
- `mcp` (v1 flat map and v2 `mcp.servers`) → `mcp`, with `enabled: false` → `disabled: true`
- `compaction.preserve_recent_tokens` → `compaction.keep.tokens`
- `instructions` → `instructions`

JS/TS plugins and custom tool files SHALL be listed as `requires manual port`, because the plugin protocols differ.

#### Scenario: OpenCode plugin not auto-ported
- **WHEN** `.opencode/plugins/notify.ts` exists
- **THEN** the report lists it under `requires manual port`, with a link to the plugin kit migration guide
