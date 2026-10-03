# compat-import Specification

## Purpose
Lets users adopt Cyber Code without rewriting their setup. Cyber Code reads the instruction, skill and MCP files of Claude Code, Codex and OpenCode in place, and `cyber import` converts their config, agents, commands, permissions, hooks and sessions into Cyber Code's native format with a reviewable dry-run diff. It mirrors Codex's `/import` of Claude Code/Cursor setups and OpenCode v2's in-memory migration of v1 config documents.

## Requirements

### Requirement: Import command
(P1) `cyber import <claude|codex|opencode|auto> [--scope project|global] [--dry-run] [--yes] [--include sessions]` SHALL detect the source tool's files for the current project and user, convert them, and show a unified diff of the files to be created or changed. `auto` SHALL import from every detected source in the order opencode, codex, claude, with later sources only filling keys not yet set. Without `--yes`, the CLI SHALL ask for confirmation before writing. `--dry-run` SHALL never write.

#### Scenario: Dry run
- **WHEN** the user runs `cyber import claude --dry-run` in a repo with `.claude/settings.json`
- **THEN** the proposed `cyber.jsonc` and `.cyber/` changes are printed as a diff, and no file is written

#### Scenario: Nothing to import
- **WHEN** no source files are found
- **THEN** the CLI prints `no claude configuration found in /repo or ~/.claude` and exits 0

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

### Requirement: Read-time instruction compatibility
(P0) Without any import, the system SHALL load these instruction files in the system context:
- `AGENTS.md` (global `~/.config/cyber/AGENTS.md`, plus walk-up project files)
- `CLAUDE.md` and `.claude/CLAUDE.md` (unless `compat.claude = false` or `CYBER_DISABLE_CLAUDE_COMPAT=1`)
- `~/.claude/CLAUDE.md` as a global fallback only when `~/.config/cyber/AGENTS.md` does not exist

Within one directory, `AGENTS.md` SHALL take precedence and `CLAUDE.md` SHALL be included only when no `AGENTS.md` exists in that directory.

#### Scenario: Both files in one directory
- **WHEN** `/repo` contains both `AGENTS.md` and `CLAUDE.md`
- **THEN** only `/repo/AGENTS.md` is loaded for that directory

### Requirement: Read-time skill compatibility
(P1) Without any import, the system SHALL discover `SKILL.md` folders under `.cyber/skills`, `.claude/skills`, `.agents/skills` and `.opencode/skills` (walking up to the project root), and under `~/.config/cyber/skills`, `~/.claude/skills`, `~/.agents/skills` and `~/.codex/skills`. Each compat location SHALL be switchable via `compat.<tool> = false`. On duplicate skill names, the precedence SHALL be `.cyber` > `.agents` > `.claude` > `.opencode` > `.codex`.

#### Scenario: Duplicate skill name
- **WHEN** both `.cyber/skills/deploy/SKILL.md` and `.claude/skills/deploy/SKILL.md` declare `name: deploy`
- **THEN** the `.cyber` skill is used, and a `duplicate skill` diagnostic lists both paths

### Requirement: Read-time MCP compatibility
(P1) When `compat.mcp_files = true` (default), the system SHALL load project `.mcp.json` servers as if declared under `mcp`, at a priority below `cyber.jsonc`. The first time each new project server is used, it SHALL require one-time user approval, recorded per project in the database.

#### Scenario: New project MCP server approval
- **WHEN** a freshly cloned repo contains `.mcp.json` with server `db-tools`
- **THEN** the TUI asks `Enable MCP server "db-tools" from .mcp.json?` before starting it

### Requirement: Session import from other tools
(P2) `cyber import <tool> --include sessions [--since <date>]` SHALL convert Claude Code JSONL transcripts (`~/.claude/projects/<dir>/*.jsonl`), Codex rollouts (`~/.codex/sessions/**/*.jsonl`) and OpenCode sessions (its SQLite database via `opencode export`) into Cyber Sessions. It SHALL keep user and assistant text, tool calls with inputs and outputs, timestamps and model IDs, and mark them `imported_from = <tool>`. Imported Sessions SHALL be resumable, with the original model mapped through the catalog.

#### Scenario: Resume an imported Claude session
- **WHEN** a Claude Code transcript is imported and the user runs `cyber -r <imported-id>`
- **THEN** the TUI shows the prior conversation, and the next prompt continues with the full history

### Requirement: Mapping report
(P1) Every import SHALL end with a report of `imported`, `merged` (an existing key was kept and the source value was ignored), `requires manual port` and `not imported` items, each with the source path, and SHALL write it to `.cyber/import-report-<timestamp>.md` when files were written.

#### Scenario: Existing key kept
- **WHEN** `cyber.jsonc` already sets `model` and the Codex config sets a different `model`
- **THEN** the existing value is kept and the report lists `model` under `merged` with both values

### Requirement: Secret safety during import
(P1) Literal secrets found in source configs (API keys, tokens, header values) SHALL NOT be copied into `cyber.jsonc`. The importer SHALL move each one into the OS keyring as a provider credential, or replace it with an `{env:NAME}` reference, naming the variable the user must set in the report.

#### Scenario: Literal API key in a Codex config
- **WHEN** `config.toml` contains `api_key = "sk-..."` for a provider
- **THEN** the key is stored as a keyring credential for that provider and `cyber.jsonc` contains no secret

### Requirement: Source detection
(P1) `cyber import --detect [--format json]` SHALL list each detected source tool with the files found, counts of agents, commands, skills, MCP servers, hooks and sessions, and whether read-time compatibility already covers each item. On the first interactive run in a project that contains any source files, the TUI SHALL offer `cyber import auto --dry-run` once per project.

#### Scenario: Detection summary
- **WHEN** a repo has `.claude/agents/` with 3 agents and `~/.codex/config.toml` with 2 MCP servers
- **THEN** `cyber import --detect` lists `claude: 3 agents` and `codex: 2 mcp servers`, with `read-time: no` for each

### Requirement: Idempotent re-import
(P1) Running the same import twice without source changes SHALL produce no file changes and report `nothing to do`. Imported entries SHALL carry an `x-imported-from` annotation (a JSONC comment) so later imports can update them without duplicating.

#### Scenario: Second import
- **WHEN** `cyber import codex --yes` runs twice in a row
- **THEN** the second run writes nothing and prints `nothing to do`
