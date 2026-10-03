# configuration Specification

## Purpose
Configuration is the set of authored `cyber.json`/`cyber.jsonc` documents and `.cyber/` directories that Cyber Code reads when a Location opens. It decides models, agents, permissions, extensions and feature settings. Layering follows OpenCode (global → walk-up project → `.cyber/` directories), with Codex-style named profiles and Claude Code-style managed scopes that users cannot widen. Each domain capability interprets the keys it owns; this spec defines discovery, format, layering, validation and reload.

## Requirements

### Requirement: Document format
(P0) The system SHALL read config documents named `cyber.jsonc` and `cyber.json` as JSONC (comments and trailing commas allowed) and SHALL validate them against the published JSON Schema `https://cyber-code.dev/schema/config.json`. A document that fails to parse SHALL produce a `ConfigParseError` naming the file, line and column. A known key with an invalid value SHALL produce a `ConfigInvalidError` listing each issue path. Unknown top-level keys SHALL produce a warning, not an error.

#### Scenario: Parse error reported with position
- **WHEN** `/repo/cyber.jsonc` contains a missing comma on line 7
- **THEN** opening the Location fails with `ConfigParseError: /repo/cyber.jsonc:7:3 expected ','`

#### Scenario: Unknown key warning
- **WHEN** a document contains a top-level key `colour`
- **THEN** a warning `unknown config key "colour"` is logged and shown by `cyber doctor`, and loading continues

### Requirement: Top-level keys
(P0) The schema SHALL accept the top-level keys `$schema`, `model`, `small_model`, `model_roles`, `providers`, `agents`, `default_agent`, `mode`, `permissions`, `sandbox`, `hooks`, `plugins`, `mcp`, `skills`, `commands`, `instructions`, `memory`, `compaction`, `snapshots`, `worktrees`, `lsp`, `formatters`, `tool_output`, `workflows`, `goals`, `loops`, `messaging`, `remote`, `runners`, `channels`, `account`, `share`, `telemetry`, `policy`, `profiles`, `default_profile`, `shell`, `attachments`, `background`, `tools`, `git`, `ide`, `autofix`, `network`, `storage`, `compat`, `services` and `experimental`. Collection keys SHALL use plural names, and entries SHALL use `disabled: true` to turn off an inherited entry.

#### Scenario: Disabling an inherited MCP server
- **WHEN** the global config defines `mcp.github` and the project config sets `mcp.github.disabled = true`
- **THEN** the `github` MCP server is not started for that project

### Requirement: Model roles
(P0) `model_roles` SHALL map the roles `default`, `small`, `advisor`, `evaluator`, `compaction` and `title` to model refs `provider/model[#variant]`. `model` SHALL be shorthand for `model_roles.default`, and `small_model` for `model_roles.small`. When a role is unset, `title` and `compaction` SHALL fall back to `small`, then `default`; `small` SHALL fall back to `default`; `evaluator` SHALL fall back to `small`, then `default`; `advisor` SHALL be disabled.

#### Scenario: Role fallback
- **WHEN** config sets only `model = "anthropic/claude-sonnet"` and `small_model = "openai/gpt-6-mini"`
- **THEN** session titles and goal evaluation use `openai/gpt-6-mini`, and the advisor is off

### Requirement: Global config directory
(P0) The system SHALL use `$XDG_CONFIG_HOME/cyber` (default `~/.config/cyber`), or `CYBER_CONFIG_DIR` when set, as the global config directory, holding `cyber.json`, `cyber.jsonc`, `tui.jsonc`, `AGENTS.md` and the supplemental subdirectories. On first run, when no global document exists, the system SHALL create `cyber.jsonc` containing only `{"$schema": "https://cyber-code.dev/schema/config.json"}`.

#### Scenario: First run bootstrap
- **WHEN** `~/.config/cyber` does not exist and the user runs `cyber`
- **THEN** the directory and a `cyber.jsonc` with only `$schema` are created

### Requirement: Project discovery
(P0) When a Location opens, the system SHALL walk from the Location directory up to the project root (the git worktree root, or the Location directory itself outside git) and collect `cyber.json`, `cyber.jsonc` and `.cyber/` directories. `CYBER_DISABLE_PROJECT_CONFIG=1` SHALL skip project discovery.

#### Scenario: Nested package config
- **WHEN** the Location is `/repo/packages/api` and both `/repo/cyber.jsonc` and `/repo/packages/api/.cyber/cyber.jsonc` exist
- **THEN** both are loaded, and the nested one has higher priority

### Requirement: Layering order
(P0) The system SHALL merge layers from lowest to highest priority as follows:
1. built-in defaults
2. global `cyber.json` then `cyber.jsonc`
3. project documents from the farthest ancestor to the nearest (`.json` before `.jsonc` in the same directory)
4. `.cyber/` directories from farthest to nearest
5. `CYBER_CONFIG` file
6. `CYBER_CONFIG_CONTENT` inline JSON
7. the selected profile
8. CLI flags
9. org-managed policy layers, which are always last (see `org-policy`)

Project layers SHALL pass workspace-trust before activating security-sensitive values or substitutions. Objects SHALL deep-merge. Scalars and arrays SHALL be replaced, except `instructions`, `plugins` and `skills`, which SHALL be concatenated and de-duplicated.

#### Scenario: Nearest wins for scalars
- **WHEN** the global config sets `mode = "default"` and `/repo/cyber.jsonc` sets `mode = "accept-edits"`
- **THEN** sessions in `/repo` start in mode `accept-edits`

#### Scenario: Instructions concatenate
- **WHEN** the global config sets `instructions = ["~/rules.md"]` and the project sets `instructions = ["docs/style.md"]`
- **THEN** both files are loaded, global first

### Requirement: Profiles
(P0) The `profiles` key SHALL map names to partial config objects. The active profile (`--profile`, `CYBER_PROFILE`, else `default_profile`) SHALL be merged at layer 7. Profiles SHALL NOT contain `profiles` or `policy` keys.

#### Scenario: Profile cannot contain policy
- **WHEN** `profiles.fast.policy` is set
- **THEN** validation fails with `ConfigInvalidError: profiles.fast.policy is not allowed`

### Requirement: Supplemental directory content
(P0) Each `.cyber/` directory and the global config directory SHALL contribute:
- agents from `agents/**/*.md`
- commands from `commands/**/*.md`
- skills from `skills/**/SKILL.md`
- workflows from `workflows/*.{js,ts}`
- hooks from `hooks.jsonc`
- local plugins from `plugins/*/` (each with a `cyber-plugin.json` manifest)
- output styles from `styles/*.md`

Files SHALL be processed in sorted path order. An invalid file SHALL be skipped with a diagnostic and SHALL NOT abort loading.

#### Scenario: Invalid agent file skipped
- **WHEN** `.cyber/agents/reviewer.md` has malformed YAML frontmatter
- **THEN** the agent is skipped, a diagnostic names the file, and other agents load

### Requirement: Variable substitution
(P0) String values SHALL support `{env:NAME}` (empty string when unset, or the `{env:NAME:-default}` fallback form) and `{file:path}` (trimmed file contents, path relative to the declaring document; `~/` and absolute paths allowed). A missing `{file:...}` target SHALL produce `ConfigInvalidError`. Substitution SHALL NOT be applied to keys.

#### Scenario: Env fallback
- **WHEN** a provider sets `base_url = "{env:LLM_URL:-http://localhost:11434/v1}"` and `LLM_URL` is unset
- **THEN** the base URL is `http://localhost:11434/v1`

### Requirement: Secrets handling
(P0) Values under keys matching `api_key`, `token`, `secret`, `password`, `authorization` or `cookie`, and every value inside a `headers` object, SHALL be treated as secrets. They SHALL be redacted as `***` in `cyber debug config`, logs, exports and telemetry. The system SHALL warn when a secret literal (not `{env:}` or `{file:}`) appears in a document inside a git worktree that is not gitignored.

#### Scenario: Committed secret warning
- **WHEN** `/repo/cyber.jsonc` (tracked by git) contains `"api_key": "sk-live-123"`
- **THEN** `cyber doctor` warns `secret literal in tracked file /repo/cyber.jsonc at providers.openai.api_key; use {env:...}`

### Requirement: TUI config document
(P0) TUI-only settings (theme, keybinds, statusline, scroll, vim mode, notifications) SHALL live in `tui.jsonc`/`tui.json` in the global directory and in `.cyber/` directories, layered like the main config and validated against `https://cyber-code.dev/schema/tui.json`. TUI keys found in `cyber.jsonc` SHALL be ignored with a warning.

#### Scenario: Theme in wrong file
- **WHEN** `cyber.jsonc` contains `theme = "dracula"`
- **THEN** a warning says `theme belongs in tui.jsonc` and the key is ignored

### Requirement: Read once per Location with live reload
(P0) The system SHALL read config once when a Location's services are built and cache it per Location. It SHALL watch every loaded document and supplemental directory, and on change SHALL rebuild the affected domains within 1 second. The new values SHALL apply at the next Safe Boundary, never mid-Turn. A reload that fails validation SHALL keep the previous config and publish `config.reload.failed.1` with the error.

#### Scenario: Live agent edit
- **WHEN** the user edits `.cyber/agents/reviewer.md` while a session is running
- **THEN** the updated agent is used from the next Turn that selects it

#### Scenario: Broken edit keeps old config
- **WHEN** the user saves a `cyber.jsonc` with a syntax error
- **THEN** the previous config remains active and the TUI shows a toast with the parse error

### Requirement: Location services lifetime
(P0) Location services (config, catalog, agents, tools, MCP clients, LSP clients) SHALL be cached per `{directory, workspace}` and released after 30 minutes with no active Session, subscriber or request for that Location.

#### Scenario: Idle Location release
- **WHEN** no request has touched Location `/tmp/x` for 30 minutes and it has no running Drain
- **THEN** its MCP and LSP child processes are stopped and its services are released

### Requirement: Config HTTP API
(P0) The server SHALL expose:
- `GET /api/v1/config` (resolved config for the request Location, secrets redacted, with each value's source layer when `?sources=true`)
- `PATCH /api/v1/config?scope=project|global`, which edits the target `cyber.jsonc` in place, preserves comments and formatting, and returns the new resolved config
- `GET /api/v1/config/schema`

A PATCH that would change a key locked by managed policy SHALL fail with 403 `PolicyLockedError`.

#### Scenario: Source attribution
- **WHEN** a client calls `GET /api/v1/config?sources=true`
- **THEN** each leaf value includes `source` such as `global:/home/u/.config/cyber/cyber.jsonc` or `policy:org`

#### Scenario: Comment-preserving patch
- **WHEN** a client patches `{"mode":"plan"}` at project scope on a `cyber.jsonc` that has comments
- **THEN** only the `mode` value changes and all comments remain

### Requirement: Environment flags
(P0) The system SHALL read `CYBER_*` environment variables, treating `1`, `true`, `yes` and `on` (case-insensitive) as true. Config-related variables SHALL include `CYBER_CONFIG`, `CYBER_CONFIG_DIR`, `CYBER_CONFIG_CONTENT`, `CYBER_PROFILE`, `CYBER_DISABLE_PROJECT_CONFIG`, `CYBER_MODEL`, `CYBER_MODE`, `CYBER_DB`, `CYBER_LOG_LEVEL`, `CYBER_DISABLE_AUTOUPDATE`, `CYBER_DISABLE_TELEMETRY`, `CYBER_OFFLINE` and `CYBER_ACCOUNT_ISSUER`.

#### Scenario: Offline flag
- **WHEN** `CYBER_OFFLINE=1`
- **THEN** no network request is made to the model catalog, update server, Relay or account issuer; only configured local providers are available

### Requirement: Network proxy and certificates
(P0) All outbound HTTP SHALL honor `HTTPS_PROXY`, `HTTP_PROXY`, `NO_PROXY` and `ALL_PROXY` (including credentials in the URL), additional CA bundles from `SSL_CERT_FILE` and `CYBER_EXTRA_CA_CERTS`, and client certificates from `network.client_cert`/`network.client_key` for mTLS.

#### Scenario: Corporate CA
- **WHEN** `CYBER_EXTRA_CA_CERTS=/etc/ssl/corp.pem` is set and the corporate proxy re-signs TLS
- **THEN** provider requests succeed and validate against the corporate CA

### Requirement: Config inspection by agents
(P1) A built-in skill `customize-cyber` SHALL describe config locations, layering, keys and examples, so the agent can edit configuration on the user's behalf. Edits made by the agent SHALL go through the `edit` permission like any other file.

#### Scenario: Agent adds an MCP server
- **WHEN** the user asks "add the GitHub MCP server for this project"
- **THEN** the agent loads `customize-cyber` and proposes an edit to `/repo/cyber.jsonc` under `mcp`, subject to the `edit` permission
