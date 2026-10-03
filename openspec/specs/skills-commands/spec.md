# skills-commands Specification

## Purpose
Skills are reusable instruction packages (`SKILL.md` plus supporting files) that the model loads on demand. Commands are prompt templates that users run as `/name args`. Together they are the lightweight way to teach Cyber Code project workflows without code. The `SKILL.md` format is shared with Claude Code, OpenCode and Codex, and those tools' skill folders are discovered for compatibility. Command templating follows OpenCode v1 (`$ARGUMENTS`, positional args, shell injection, `@` references, agent/model/subtask overrides), and Codex's Record & Replay inspires the bundled `record-to-skill` skill.

## Requirements

### Requirement: SKILL.md format
(P0) A skill SHALL be a directory containing `SKILL.md` with YAML frontmatter fields `name` (required, `^[a-z0-9][a-z0-9-]{0,63}$`), `description` (required, at most 1024 characters), and optional `allowed-tools` (list of tool permission patterns pre-approved while the skill is active), `model` (`provider/model`), `disable-model-invocation` (boolean), `user-invocable` (boolean, default true) and `argument-hint`. The Markdown body SHALL be the skill content.

#### Scenario: Valid skill
- **WHEN** `.cyber/skills/release/SKILL.md` has `name: release` and `description: Cut a release`
- **THEN** skill `release` is registered with its directory as base path

#### Scenario: Invalid frontmatter
- **WHEN** a `SKILL.md` frontmatter is not valid YAML
- **THEN** the skill is skipped and a diagnostic naming the file and parse error is shown in `/skills` and `cyber doctor`

### Requirement: Skill discovery locations
(P0) The system SHALL discover skills in this precedence order (later wins on duplicate names): built-in, plugin skills, `~/.config/cyber/skills/*/SKILL.md`, the compatibility folders `~/.claude/skills`, `~/.agents/skills` and `~/.codex/skills`, then for each directory from the git root down to the Location, `.claude/skills`, `.agents/skills`, `.codex/skills` and `.cyber/skills`, and finally `skills` config entries (paths or URLs). `skills.compat: false` SHALL disable the `.claude`, `.agents` and `.codex` locations.

#### Scenario: Project overrides global
- **WHEN** both `~/.config/cyber/skills/deploy` and `.cyber/skills/deploy` exist
- **THEN** the project skill is used and a duplicate notice lists both paths

#### Scenario: Compat disabled
- **WHEN** `skills.compat` is false
- **THEN** skills under `.claude/skills` are not loaded

### Requirement: Remote skill sources
(P1) A `skills` entry that is an `https://` URL SHALL be treated as an index (`<url>/index.json` with `{ skills: [{ name, files, version? }] }`). Listed files SHALL be downloaded to `~/.cache/cyber/skills/<host>/<name>/`, refreshed at most every 24 h, and used from cache when offline. Fetch failures SHALL be logged and SHALL NOT block startup.

#### Scenario: Offline use of cached remote skill
- **WHEN** the network is unavailable and a remote skill was cached yesterday
- **THEN** the cached skill is loaded and a warning notes the stale cache

### Requirement: Skill listing in system context
(P0) The system SHALL add a `core/skills` Context Source listing, for the selected agent, the name and description of each permitted, model-invocable skill, sorted by name, inside `<available_skills>`. The list SHALL be bounded by `skills.listing_budget_tokens` (default 2000). When over budget, descriptions SHALL be truncated to 120 characters and then remaining names listed without descriptions.

#### Scenario: Denied skill not listed
- **WHEN** an agent's permission denies `skill` for `deploy`
- **THEN** `deploy` does not appear in that agent's `<available_skills>`

### Requirement: Skill tool
(P0) The `skill` tool SHALL take `name` and optional `arguments`, check the `skill` permission with resource equal to the name, and return the skill body wrapped in `<skill name="..." base="...">` plus a listing of up to 20 sibling files (relative paths). Unknown names SHALL fail with `Skill "<name>" not found. Available: <names>`.

#### Scenario: Skill loaded by model
- **WHEN** the model calls `skill` with `name: "release"`
- **THEN** the body, base directory and file listing are returned as tool output

### Requirement: Skill-scoped tool approvals and model
(P1) While a skill loaded in the current Turn declares `allowed-tools`, matching tool calls SHALL be allowed without prompting, unless a permission rule denies them. A skill `model` SHALL apply only when the skill is run as a user command or as a subtask.

#### Scenario: Pre-approved git commands
- **WHEN** skill `release` declares `allowed-tools: ["bash:git tag *"]` and the model runs `git tag v1.2.0`
- **THEN** no permission prompt is shown

### Requirement: Skill directories readable
(P0) Every discovered skill directory SHALL be added to the `external_directory` allow list as `<dir>/*` for read access, so agents can read skill resources without prompting.

#### Scenario: Read a skill script
- **WHEN** the model reads `~/.config/cyber/skills/release/notes.md`
- **THEN** no external-directory prompt is shown

### Requirement: Skills as user commands
(P0) Each skill with `user-invocable` not false SHALL be invocable as `/<name> [args]`, which admits the skill body followed by `ARGUMENTS: <args>` as the user prompt. A skill with `disable-model-invocation: true` SHALL be omitted from `<available_skills>` and loadable only by the user.

#### Scenario: User runs a skill
- **WHEN** the user types `/release minor`
- **THEN** the release skill body is submitted with arguments `minor`

### Requirement: Custom command definitions
(P0) The system SHALL load commands from `commands` config entries (`{ template, description?, agent?, model?, variant?, subtask?, argument_hint? }`) and from Markdown files matching `{command,commands}/**/*.md` in `~/.config/cyber`, every `.cyber/` directory, and the compat folders `.claude/commands`. Frontmatter SHALL supply fields and the body SHALL become the template. A command's name SHALL be its path relative to the commands folder without extension, using `/` for nesting.

#### Scenario: Nested command name
- **WHEN** `.cyber/commands/db/migrate.md` exists
- **THEN** it is invocable as `/db/migrate`

### Requirement: Command source precedence
(P0) Command names SHALL resolve in order: built-in commands, plugin commands, user commands, project commands, MCP prompts (`/<server>:<prompt>`), then skills, with later sources replacing earlier ones of the same name, except that built-ins listed as reserved (`/help`, `/exit`, `/goal`, `/loop`, `/workflows`, `/mode`) SHALL NOT be overridable.

#### Scenario: Reserved name protected
- **WHEN** a project defines `.cyber/commands/goal.md`
- **THEN** it is registered as `/project:goal` and `/goal` remains the built-in

### Requirement: Argument substitution
(P0) The system SHALL tokenize arguments (double-quoted, single-quoted, or whitespace-separated, quotes stripped) and substitute `$1..$N` positionally. The highest-numbered placeholder SHALL receive all remaining tokens. `$ARGUMENTS` SHALL receive the raw argument string. When the template has no placeholders and the arguments are non-empty, they SHALL be appended after a blank line.

#### Scenario: Positional args
- **WHEN** template `Fix issue $1 in $2` is run with `42 "auth module"`
- **THEN** the prompt is `Fix issue 42 in auth module`

### Requirement: Shell output injection
(P1) The system SHALL replace each `` !`cmd` `` in a template with the stdout of running `cmd` in the sandbox. It SHALL first evaluate `bash` permission for the command text in the current mode, and an `ask` effect SHALL prompt before the command runs. Commands SHALL run concurrently with a 30 s timeout each, and output SHALL be capped at 20 KiB per command.

#### Scenario: Denied shell injection
- **WHEN** a template contains `` !`cat ~/.aws/credentials` `` and the rules deny it
- **THEN** the command is not run and the substitution is `[denied: cat ~/.aws/credentials]`

### Requirement: File and agent references
(P0) Each `@path` token in an expanded template SHALL be resolved relative to the project root (or home for `~/`) and attached as a file or directory part when it exists. `@agent-name` matching a subagent SHALL become an agent invocation part, and other tokens SHALL remain text.

#### Scenario: File reference attached
- **WHEN** the template contains `@src/auth.rs`
- **THEN** the file is attached to the prompt

### Requirement: Agent, model and subtask overrides
(P0) A command SHALL run with its `agent` when set, otherwise the caller's agent. Its model SHALL resolve as the command `model`, then the command agent's model, then the session model. When `subtask` is true, or the agent's mode is `subagent`, the command SHALL run in a child session and only its summary SHALL be returned to the caller.

#### Scenario: Review as subtask
- **WHEN** `/review` has `subtask: true` and `agent: reviewer`
- **THEN** a child session with agent `reviewer` runs it and the parent receives the summary

### Requirement: Argument hints and autocomplete data
(P0) The system SHALL expose for each command and skill its name, description, source, `argument_hint` (or derived placeholders) and namespace via `GET /api/v1/commands`, which clients use for slash autocomplete.

#### Scenario: Hints listed
- **WHEN** a client requests `GET /api/v1/commands`
- **THEN** `/db/migrate` is listed with `argument_hint: "<direction> [steps]"`

### Requirement: Built-in commands
(P0) The system SHALL provide built-in server commands `/init` (generate or refresh `AGENTS.md`), `/review` (review uncommitted changes, a branch or a PR), `/compact`, `/goal`, `/loop`, `/workflows`, `/tasks`, `/agents`, `/model`, `/mode`, `/memory`, `/rewind`, `/fork`, `/side` (alias `/btw`), `/remote`, `/share`, `/status`, `/cost`, `/doctor`, `/hooks`, `/mcp`, `/skills`, `/plugins` and `/help`. Their behavior SHALL be specified by the owning capability, and TUI-only commands SHALL be owned by the `tui` capability.

#### Scenario: Built-in available in exec mode
- **WHEN** `cyber exec --command init` runs in a repo without `AGENTS.md`
- **THEN** an `AGENTS.md` is generated through the normal permission flow

### Requirement: Bundled skills
(P1) The system SHALL bundle skills `batch` (split a large change into 5-30 worktree-isolated subagents), `simplify` (review changed code for reuse and simplification), `security-review` (review pending changes for vulnerabilities) and `customize-cyber` (how to edit Cyber Code config, agents, skills, hooks and MCP). Each SHALL be replaceable by a discovered skill of the same name.

#### Scenario: Override bundled skill
- **WHEN** a project defines `.cyber/skills/simplify/SKILL.md`
- **THEN** the project version replaces the bundled one

### Requirement: Record to skill
(P2) The system SHALL provide the bundled skill `record-to-skill`, which turns the current session's (or a selected range of) user-approved actions into a new `SKILL.md`. The draft SHALL include a description, ordered steps, the commands used and allowed-tools, be written to `.cyber/skills/<name>/` after the user confirms, and appear in a diff review before saving.

#### Scenario: Recorded deploy steps become a skill
- **WHEN** the user runs `/record-to-skill deploy-staging` after manually guiding a staging deploy
- **THEN** a draft `deploy-staging` skill is shown for review and saved on approval

### Requirement: Skill and command inspection
(P0) The system SHALL provide `/skills` and `cyber skills list|show <name>`, `cyber commands list`, and `GET /api/v1/skills`. These SHALL show each item's source path, scope, permission effect for the current agent, and any diagnostics.

#### Scenario: Diagnose missing skill
- **WHEN** the user runs `cyber skills list`
- **THEN** skipped skills appear with their parse or duplicate diagnostics
