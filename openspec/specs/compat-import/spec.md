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
- `CLAUDE.md` and `.claude/CLAUDE.md` (unless `compat.claude = false`, `CYBER_DISABLE_CLAUDE_COMPAT=1` or `CYBER_DISABLE_CLAUDE_MD=1`)
- `~/.claude/CLAUDE.md` as a global fallback only when `~/.config/cyber/AGENTS.md` does not exist

Within one directory, `AGENTS.md` SHALL take precedence and `CLAUDE.md` SHALL be included only when no `AGENTS.md` exists in that directory. `CYBER.md` is Cyber Code's native instruction file, not a compatibility file. No compatibility switch SHALL disable it, and it SHALL load after the compatibility files of its directory.

#### Scenario: Both files in one directory
- **WHEN** `/repo` contains both `AGENTS.md` and `CLAUDE.md`
- **THEN** only `/repo/AGENTS.md` is loaded for that directory

#### Scenario: Compatibility off keeps CYBER.md
- **WHEN** `compat.claude = false` and `/repo` contains `CLAUDE.md` and `CYBER.md`
- **THEN** only `/repo/CYBER.md` is loaded for that directory

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

### Requirement: Pure Claude permission conversion
(P1) Import planning SHALL convert Claude permissions allow, ask and deny arrays to typed native rules in that group order, preserving order within each group. It SHALL convert the canonical final Bash prefix selector `:*` to ` *`, strip leading project-relative `./` selectors, map known native actions explicitly and map supported defaultMode aliases. Conversion SHALL perform no filesystem or process operations and SHALL NOT grant trust. A malformed or unsupported rule/mode SHALL refuse the batch with a bounded field/index and static reason, without echoing the source value or returning a permissive partial result. This converter alone SHALL NOT constitute complete import command or milestone acceptance.

#### Scenario: Deny wins over broad allow
- **WHEN** Claude permits Bash generally, asks for a specific command and denies that command
- **THEN** converted rules SHALL cause the existing last-match-wins engine to deny the command
- **AND** unrelated permitted commands SHALL remain permitted

#### Scenario: Invalid source selector
- **WHEN** a source permission is malformed or uses an unmapped action
- **THEN** conversion SHALL return no rule batch
- **AND** its error SHALL identify the source field/index without repeating source contents

### Requirement: Imported edit-tool permission granularity
(P1) Ordered permission rules SHALL support an optional exact native `tool` selector in addition to action/resource/effect. Rules without this selector SHALL preserve their existing meaning. Claude Write and NotebookEdit selectors SHALL map to the shared edit action scoped to write and notebook_edit respectively; Claude Edit/MultiEdit SHALL map to edit scoped to edit. It SHALL NOT infer an apply_patch grant: patch operations can create/remove files and are not the same source tool. The host SHALL bind the actual invocation identity before permission, protected-path, ancestor-mode or auto-review decisions. A scoped allow SHALL NOT match another tool or an evaluation without a tool identity. Scoped deny ceilings SHALL survive bypass, saved approvals and later lower-priority grants. Model tool catalogs SHALL hide fully denied scoped tools without hiding unrelated edit tools. Invalid tool selector types or empty/control-containing selectors SHALL fail closed rather than become unscoped allows.

#### Scenario: Write allow does not grant all edits
- **WHEN** imported permissions allow Write but do not allow Edit
- **THEN** an ordinary write SHALL be allowed under that scoped rule
- **AND** an edit or patch SHALL retain its own permission decision

#### Scenario: Scoped denial survives bypass
- **WHEN** imported permissions deny Write for a path while allowing Edit
- **THEN** write SHALL be denied in default and bypass modes
- **AND** edit SHALL remain independently governed

### Requirement: Pure OpenCode permission conversion
(P1) Import planning SHALL convert explicit OpenCode permission/permissions strings, ordered action/resource/effect arrays, ordered action/pattern maps and legacy tools booleans into typed native rules without executing code or accessing files. Legacy tool rules SHALL precede explicit permission rules. It SHALL map shell to bash, task/subagent to agent and write/patch to edit. Map and array ordering SHALL survive conversion. A source literal glob ending in space-star SHALL NOT acquire Cyber's additional bare-command match. Ambiguous simultaneous permission and permissions fields, malformed shapes, unknown actions/effects or resource patterns requiring unresolved home expansion SHALL refuse the whole batch with bounded static errors that do not echo source values. This explicit converter SHALL NOT imply complete source discovery, implicit source defaults or full migration acceptance.

#### Scenario: Explicit permission overrides legacy boolean
- **WHEN** legacy tools disable bash and a later permission rule allows one command
- **THEN** other commands SHALL stay denied and that command SHALL be allowed

#### Scenario: Command prefix retains its literal space
- **WHEN** a source rule allows git followed by a space and wildcard
- **THEN** git status SHALL match
- **AND** bare git SHALL retain its separate decision

### Requirement: Pure Codex argv-prefix conversion
(P1) Import planning SHALL convert parsed constant Codex prefix rules to native bash rules. It SHALL support nonempty literal argv-prefix lists and nonempty alternatives per position, default decision allow and allow/prompt/forbidden effects. It SHALL order output allow, ask, deny, preserving source order within each group, so the most restrictive matching decision survives native last-match-wins evaluation. Expansion SHALL be bounded at 4096 rules and one MiB of command text per batch. Native rules SHALL retain an optional argv_prefix and compare literal Bash argv with exact case, preserving quotes/whitespace normalization rather than matching only source text. Unresolved argv SHALL not receive an imported allow; matching ask/deny rules SHALL conservatively retain their refusal/review boundary. Tokens that cannot be safely represented as literal native command patterns, malformed fields and unknown decisions SHALL refuse the entire batch with indexed static errors without echoing source values. Conversion SHALL neither execute source Starlark nor grant sandbox escalation. Starlark parsing, examples/metadata, source discovery and full Codex configuration/profile/sandbox migration SHALL remain separate required work.

#### Scenario: Later allow cannot erase a forbidden prefix
- **WHEN** a parsed batch forbids git push and later allows git generally
- **THEN** native evaluation SHALL deny git push and allow unrelated git commands

#### Scenario: Literal argv cannot become a wildcard grant
- **WHEN** a prefix token contains a native wildcard or whitespace
- **THEN** conversion SHALL refuse the batch instead of widening the command grant

### Requirement: Literal Codex rules source parsing
(P1) Import planning SHALL parse bounded constant prefix_rule calls from Codex rules text without evaluating Starlark or executing commands. It SHALL accept comments, single/double quoted strings, whitespace, trailing commas, literal argv alternatives, decisions, justification and match/not_match examples. It SHALL retain source metadata and validate each example against its source argv prefix, then apply the shared strongest-match converter to the complete batch. Input SHALL be bounded at one MiB, 4096 calls and two list levels. Unknown statements, executable expressions, duplicate/unknown fields, malformed strings and contradictory examples SHALL refuse the whole batch with static indexed/offset errors without echoing source values. Unsupported dynamic Starlark constructs SHALL remain reportable as not imported, without silently dropping their potentially protective rules. Source file discovery, config/profile/sandbox conversion and reviewed import writing SHALL remain required.

#### Scenario: Source examples are checked before import
- **WHEN** a prefix rule declares git push but includes git status in its match examples
- **THEN** no converted batch SHALL be returned
- **AND** the failure SHALL identify the example index without repeating source text

#### Scenario: Source code cannot run during migration
- **WHEN** source text contains load, assignments or function calls instead of constant prefix_rule fields
- **THEN** parsing SHALL refuse without executing that source

### Requirement: Read-only migration file inventory
(P1) Migration discovery SHALL accept explicit canonical project-root, current-directory, home and optional CODEX_HOME inputs and inventory static Claude/Codex/OpenCode project/global source locations without reading configuration contents, executing commands or writing files. It SHALL preserve source tool, global/project/custom-home layer, file kind and path, walk project layers from root to current directory, and order source groups opencode, codex, claude. It SHALL include settings/config/profile files, instructions, MCP files, agents, commands, skills/assets, rules, hooks/policy files and OpenCode manual-port files. Symlinks and inspection failures SHALL be reported explicitly without following linked directory trees. Traversal SHALL be deterministically ordered and bounded at 4096 directory-entry/metadata inspections and 32 tree levels; limit failures SHALL return no incomplete inventory as a complete result. Referenced custom-agent/config paths, parsed item counts/read-time status, sessions/SQLite counts, CLI detection and import review/writes SHALL remain mandatory subsequent delivery.

#### Scenario: Explicit custom Codex home
- **WHEN** an explicit CODEX_HOME differs from the default user directory
- **THEN** discovered sources SHALL retain which home supplied each config/profile/rule
- **AND** discovery SHALL not change process-global environment

#### Scenario: Linked source directory
- **WHEN** an agents or skills source points through a symbolic link
- **THEN** discovery SHALL report that source without scanning its target
- **AND** no files SHALL be written or executed

### Requirement: Verified migration source snapshots
(P1) Migration planning SHALL read only inventoried files under their declared explicit source roots. It SHALL open each path component without following links, retain native directory/file identities, reject reparse points/nonregular/hard-linked source files and bound source contents at one MiB. It SHALL revalidate directory bindings, file identity, modification metadata and content before returning a snapshot and provide the same verification for later reviewed writing. Source bytes SHALL not appear in snapshot debugging or error messages. Changed/replaced/linked/missing sources SHALL refuse instead of authorizing stale review. Snapshot verification alone SHALL NOT grant trust or implement destination transaction/confirmation guarantees; full reviewed import writing remains required.

#### Scenario: Source changed after review
- **WHEN** an inventoried source is edited or replaced after its snapshot is read
- **THEN** snapshot verification SHALL fail
- **AND** it SHALL not expose source contents in its error

#### Scenario: Source directory replaced
- **WHEN** a source ancestor is renamed/replaced or linked to a different tree
- **THEN** verification SHALL refuse even when a same-named file remains readable

#### Scenario: Windows directory changes during review
- **WHEN** a source snapshot has returned and the user renames its project/source directory
- **THEN** retained review identities SHALL permit namespace deletion/renaming
- **AND** later verification SHALL refuse changed/replaced source bindings
- **AND** capability path operations SHALL continue to use fresh no-follow lookup handles

### Requirement: Read-only static-source detection command
(P1) The implemented `cyber import --detect [--format json]` increment SHALL inventory known static project/global sources, parse JSON/JSONC/TOML definitions with bounded verified reads and return raw agent/command/skill/MCP/hook counts without source configuration values. It SHALL avoid bootstrap, database, logging or source execution. Incomplete session counts, referenced sources and effective read-time coverage SHALL remain explicitly unknown/incomplete; this increment SHALL NOT imply acceptance of canonical complete Source detection or reviewed import writing.

#### Scenario: Detect without bootstrap writes
- **WHEN** a user detects a project containing supported source definitions
- **THEN** the command SHALL report raw counts without creating Cyber configuration or runtime storage
- **AND** unresolved coverage SHALL remain explicit

#### Scenario: Malformed source contains secrets
- **WHEN** a source document cannot be parsed
- **THEN** detection SHALL report a static issue without echoing source values

### Requirement: Secret-safe Codex provider/model planning
(P1) The provider/model adapter SHALL parse bounded TOML and convert supported Codex model/provider declarations into native provider-catalog configuration. It SHALL map explicit chat and Responses wire protocols, preserve the documented Responses default and qualify/register selected custom models without inventing pricing. Literal API keys/bearer tokens and static HTTP header values SHALL become deterministic environment references with value-free setup records; existing environment names SHALL remain references. Unsupported fields SHALL remain explicit indexed pending mappings. Ambiguous credentials/headers, unsafe endpoint forms and unsupported account authentication SHALL refuse the batch with value-free errors. This pure adapter SHALL NOT imply complete source/profile/sandbox migration, keyring persistence, reviewed writing or import acceptance.

#### Scenario: Provider resolves through native loader
- **WHEN** a converted custom provider and selected model are loaded with the reported environment variables set
- **THEN** the native catalog SHALL resolve the intended protocol, endpoint, credential and headers
- **AND** custom pricing SHALL remain unknown

#### Scenario: Literal credentials remain out of plans
- **WHEN** a source contains literal API keys or static header values
- **THEN** serialized/debug plan output SHALL omit those values
- **AND** the plan SHALL name the environment variables the user must populate
