## MODIFIED Requirements

### Requirement: Claude Code sources
(P1) The `claude` importer SHALL read:
- `~/.claude/settings.json`, `.claude/settings.json`, `.claude/settings.local.json`
- `~/.claude.json` and `.mcp.json` (MCP servers)
- `.claude/agents/*.md` and `~/.claude/agents/*.md`
- `.claude/commands/**/*.md`
- `.claude/skills/**/SKILL.md`
- `.claude/rules/*.md` and `~/.claude/rules/*.md`
- `CLAUDE.md` files
- hook definitions under `hooks` in settings

It SHALL map them as follows:
- `permissions.allow`/`deny`/`ask` → `permissions` rules, emitted in the order `allow`, `ask`, `deny` so that last-match-wins reproduces Claude Code's deny-over-ask-over-allow precedence
- `defaultMode` → `mode` (`acceptEdits` → `accept-edits`, `bypassPermissions` → `bypass`, `dontAsk` → `dont-ask`)
- `model` aliases → `anthropic/<model>`; `fallbackModel` → `fallback_models`
- `apiKeyHelper` → a `command` credential; `env` → a profile `env` block
- `.claude/rules/*.md` → `.cyber/rules/*.md`, keeping `paths` frontmatter
- hooks → `.cyber/hooks.jsonc`, with event names mapped one-to-one where an equivalent exists (PascalCase tool matchers lower-cased, `permissionDecision`/`updatedInput`/`additionalContext` renamed), and `Setup`, `PermissionDenied`, `InstructionsLoaded`, `PreModelSwitch`/`PostModelSwitch`, `Elicitation`, `DirectoryAdded` and `PostToolBatch` mapped to their Cyber events

#### Scenario: Permission rule conversion
- **WHEN** `.claude/settings.json` has `"permissions": {"allow": ["Bash(npm run test:*)"], "deny": ["Read(./.env)"]}`
- **THEN** the import produces rules `{ "action": "bash", "resource": "npm run test *", "effect": "allow" }` followed by `{ "action": "read", "resource": ".env", "effect": "deny" }`, in that order

#### Scenario: Unmappable hook event
- **WHEN** a Claude hook uses an event with no Cyber equivalent
- **THEN** the hook is skipped and the import report lists it under `not imported` with the reason

### Requirement: Codex sources
(P1) The `codex` importer SHALL read `~/.codex/config.toml` (and `$CODEX_HOME`), `~/.codex/<name>.config.toml` profile files, `.codex/config.toml` project layers, `AGENTS.md` and `AGENTS.override.md` files, `~/.codex/skills` and `~/.agents/skills`, `[mcp_servers.*]` tables, `[agents]` and custom agent TOML files, `[profiles.*]`, `[model_providers.*]`, `[permissions.*]` profiles, `rules/*.rules` execpolicy files, `hooks.json`, `[shell_environment_policy]`, `notify` and `requirements.toml`. It SHALL map:
- `model` and `model_provider` → `model`
- `approval_policy` + `sandbox_mode` → `mode` + `sandbox` (`on-request` → `default`, `never` + `workspace-write` → `dont-ask` with the sandbox enabled, `danger-full-access` → `bypass` with the sandbox disabled; the retired `untrusted` → `default` with a deprecation note; `granular` approvals → equivalent `ask`/`deny` rules)
- `[permissions.*]` → `sandbox.profiles.*`; `[shell_environment_policy]` → `sandbox.env`
- `rules/*.rules` `prefix_rule` entries → `bash` permission rules (`allow`/`prompt`/`forbidden` → `allow`/`ask`/`deny`)
- `hooks.json` → `.cyber/hooks.jsonc`; `notify` → a `Notification` command hook
- `model_providers` → `providers` in the `provider-catalog` shape: `base_url` → `api.url`, `wire_api` `chat` → `api.type: "openai-compatible"` and `responses` → `api.type: "openai-responses"`, `env_key` → `api.settings.api_key: "{env:NAME}"`
- profiles → `profiles`
- `requirements.toml` → a report of equivalent `org-policy` settings (not written, because policy files are admin-owned)

#### Scenario: Custom OpenAI-compatible provider
- **WHEN** `config.toml` has `[model_providers.local] base_url = "http://localhost:8000/v1" env_key = "LOCAL_KEY" wire_api = "chat"`
- **THEN** the import creates `providers.local = { "api": { "type": "openai-compatible", "url": "http://localhost:8000/v1", "settings": { "api_key": "{env:LOCAL_KEY}" } } }`

#### Scenario: Execpolicy rule converted
- **WHEN** `rules/git.rules` contains `prefix_rule(pattern=["git","push"], decision="forbidden")`
- **THEN** the import produces `{ "action": "bash", "resource": "git push *", "effect": "deny" }`
