# skills-commands Specification

## Purpose
Skills are reusable instruction packages (`SKILL.md` plus supporting files) that the model loads on demand. Commands are prompt templates that users run as `/name args`. Together they are the lightweight way to teach Cyber Code project workflows without code. The `SKILL.md` format is shared with Claude Code, OpenCode and Codex, and those tools' skill folders are discovered for compatibility. Command templating follows OpenCode v1 (`$ARGUMENTS`, positional args, shell injection, `@` references, agent/model/subtask overrides), and Codex's Record & Replay inspires the bundled `record-to-skill` skill.

## Requirements

### Requirement: SKILL.md format
(P0) A skill SHALL be a directory containing `SKILL.md` with YAML frontmatter fields `name` (required, `^[a-z0-9][a-z0-9-]{0,63}$`), `description` (required, at most 1024 characters), and optional `allowed-tools` (list of tool permission patterns pre-approved while the skill is active), `model` (`provider/model`), `disable-model-invocation` (boolean), `user-invocable` (boolean, default true) and `argument-hint`. The Markdown body SHALL be the skill content. `SKILL.md` SHALL be read without following links as a regular file of at most 1 MiB. A linked `SKILL.md` or skill directory, a linked skill root, an oversized file, or a `model` that is not `provider/model[#variant]` SHALL skip the skill with a diagnostic, and links SHALL NOT be read.

#### Scenario: Valid skill
- **WHEN** `.cyber/skills/release/SKILL.md` has `name: release` and `description: Cut a release`
- **THEN** skill `release` is registered with its directory as base path

#### Scenario: Invalid frontmatter
- **WHEN** a `SKILL.md` frontmatter is not valid YAML
- **THEN** the skill is skipped and a diagnostic naming the file and parse error is shown in `/skills` and `cyber doctor`

#### Scenario: Linked skill refused
- **WHEN** `.cyber/skills/ops` is a symbolic link to a directory elsewhere
- **THEN** no skill is loaded from it and a diagnostic names the link

### Requirement: Skill discovery locations
(P0) The system SHALL discover skills in this precedence order (later wins on duplicate names): built-in, plugin skills, `~/.config/cyber/skills/*/SKILL.md`, the compatibility folders `~/.claude/skills`, `~/.agents/skills` and `~/.codex/skills`, then for each directory from the git root down to the Location, `.claude/skills`, `.agents/skills`, `.codex/skills` and `.cyber/skills`, and finally `skills` config entries (paths or URLs). `skills.compat: false` SHALL disable the `.claude`, `.agents` and `.codex` locations.

#### Scenario: Project overrides global
- **WHEN** both `~/.config/cyber/skills/deploy` and `.cyber/skills/deploy` exist
- **THEN** the project skill is used and a duplicate notice lists both paths

#### Scenario: Compat disabled
- **WHEN** `skills.compat` is false
- **THEN** skills under `.claude/skills` are not loaded

### Requirement: Remote skill sources
(P1) A `skills` entry that is an `https://` URL SHALL be treated as an index (`<url>/index.json` with `{ skills: [{ name, files, version? }] }`), and each listed file SHALL be fetched from `<url>/<name>/<file>`. Listed files SHALL be downloaded to `~/.cache/cyber/skills/<host>/<source-id>/<generation>/<name>/`, where `<host>` carries the port when one is given and `<source-id>` is derived from the source URL so two sources on one host never share a directory; a refresh writes a new generation and then switches the source's `.marker.json` to it, so readers never see a half-written set, refreshed at most every 24 h, and used from cache when offline. Fetch failures SHALL be logged and SHALL NOT block startup: refreshes SHALL run in the background, failures SHALL be reported as skill diagnostics (`GET /api/v1/skills`, `cyber skills list`, `cyber doctor`), and discovery SHALL use whatever cache exists at the entry's position in the precedence order. Fetches SHALL go through the policy-checked client (owned proxy, effective network mode and allowlist, no redirects), and only `https://` URLs without credentials, query or fragment SHALL be accepted; the fetcher SHALL be injectable so tests and embedders can serve sources in process. The index SHALL be at most 256 KiB and list at most 64 skills with valid, unique names, each listing `SKILL.md` and at most 64 files. A file path SHALL be relative, `/`-separated and made of letters, digits, `.`, `_` and `-`, with no `..`, hidden or dot-terminated component and no Windows device name (`CON`, `NUL`, ...). Each file SHALL be at most 1 MiB, each skill 4 MiB and each source 16 MiB, and each `SKILL.md` SHALL declare its index name. A failed or invalid refresh SHALL keep the previous cache, and the files SHALL be written owner-only with a marker recording the fetch time, generation and content digest. Skills from a cache older than 24 h SHALL carry a stale-cache warning. A URL from a config layer other than the user's own (global config, environment or command line), such as project config, SHALL be refreshed under the sandbox network mode and allowlist and SHALL stay inactive until the checkout approves the digest of its URL and cached content (`cyber skills trust <url>`), so a changed remote set needs a new approval. Cached skill directories SHALL receive the ordinary skill directory read allowance and no other permission.

#### Scenario: Offline use of cached remote skill
- **WHEN** the network is unavailable and a remote skill was cached yesterday
- **THEN** the cached skill is loaded and a warning notes the stale cache

#### Scenario: Project remote source needs approval
- **WHEN** project config lists `https://skills.example.com/team` and its skills are cached
- **THEN** they are not loaded and a diagnostic names `cyber skills trust https://skills.example.com/team` until the checkout approves that content, and a changed remote set needs a new approval

#### Scenario: Unsafe index path refused
- **WHEN** `index.json` lists the file `../../.bashrc`
- **THEN** the refresh fails with a diagnostic, nothing is written and the previous cache is kept

### Requirement: Skill listing in system context
(P0) The system SHALL add a `core/skills` Context Source listing, for the selected agent, the name and description of each permitted, model-invocable skill, sorted by name, inside `<available_skills>`. The list SHALL be bounded by `skills.listing_budget_tokens` (default 2000). When over budget, descriptions SHALL be truncated to 120 characters and then remaining names listed without descriptions. When even names do not fit, the list SHALL end with a `... N more` marker, so the block never exceeds the budget (four characters per token). Permissions SHALL be evaluated with the selected agent's profile and the Session ruleset.

#### Scenario: Denied skill not listed
- **WHEN** an agent's permission denies `skill` for `deploy`
- **THEN** `deploy` does not appear in that agent's `<available_skills>`

#### Scenario: Listing over budget
- **WHEN** 200 skills with long descriptions are permitted and the budget is 2000 tokens
- **THEN** the block stays within 8000 characters and ends its list with `... N more`

### Requirement: Skill tool
(P0) The `skill` tool SHALL take `name` and optional `arguments`, check the `skill` permission with resource equal to the name, and return the skill body wrapped in `<skill name="..." base="...">` plus a listing of up to 20 sibling files (relative paths). Unknown names SHALL fail with `Skill "<name>" not found. Available: <names>`, where `<names>` lists only model-invocable skills the agent may load.

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
(P0) Each skill with `user-invocable` not false SHALL be invocable as `/<name> [args]`. The skill body SHALL be treated as a command template: `$ARGUMENTS` and `$1..$N` SHALL be substituted exactly as for custom commands, and when the body has no placeholders and arguments are non-empty, the arguments SHALL be appended after a blank line as `ARGUMENTS: <args>`. A skill with `disable-model-invocation: true` SHALL be omitted from `<available_skills>` and loadable only by the user.

#### Scenario: User runs a skill
- **WHEN** the user types `/release minor` and the skill body contains `Cut a $1 release`
- **THEN** the prompt submitted is the body with `Cut a minor release`

#### Scenario: User runs a skill without placeholders
- **WHEN** the user types `/release minor` and the body has no placeholders
- **THEN** the body is submitted followed by a blank line and `ARGUMENTS: minor`

### Requirement: Custom command definitions
(P0) The system SHALL load commands from `commands` config entries (`{ template, description?, agent?, model?, variant?, subtask?, argument_hint? }`) and from Markdown files matching `{command,commands}/**/*.md` in `~/.config/cyber`, every `.cyber/` directory, and the compat folders `.claude/commands`. Frontmatter SHALL supply fields and the body SHALL become the template. A command's name SHALL be its path relative to the commands folder without extension, using `/` for nesting. A definition whose `model` is not `provider/model[#variant]`, whose `variant` is empty or repeats the model's `#variant`, whose `agent` names no configured agent, or whose `subtask` agent cannot run as a subagent SHALL be unavailable without falling back to a shadowed definition, and SHALL be reported as a command issue (`command_issues` in `GET /api/v1/skills`, `cyber doctor`).

#### Scenario: Nested command name
- **WHEN** `.cyber/commands/db/migrate.md` exists
- **THEN** it is invocable as `/db/migrate`

#### Scenario: Unknown command agent
- **WHEN** `.cyber/commands/ghost.md` declares `agent: nobody` and no agent `nobody` is configured
- **THEN** `/ghost` is not listed by `GET /api/v1/commands`, running it fails as an unknown command, and `GET /api/v1/skills` lists a command issue for the file

### Requirement: Command source precedence
(P0) Command names SHALL resolve in order: built-in commands, plugin commands, user commands, project commands, MCP prompts (`/<server>:<prompt>`), then skills, with later sources replacing earlier ones of the same name, except that built-ins listed as reserved (`/help`, `/exit`, `/goal`, `/loop`, `/workflows`, `/mode`) SHALL NOT be overridable.

#### Scenario: Reserved name protected
- **WHEN** a project defines `.cyber/commands/goal.md`
- **THEN** it is registered as `/project:goal` and `/goal` remains the built-in

### Requirement: Argument substitution
(P0) The system SHALL tokenize arguments (double-quoted, single-quoted, or whitespace-separated, quotes stripped) and substitute `$1..$N` positionally. The highest-numbered placeholder SHALL receive all remaining tokens. `$ARGUMENTS` SHALL receive the raw argument string. Substitution SHALL be a single pass over the template, so argument text containing `$1` or `$ARGUMENTS` is never substituted again. When the template has no placeholders and the arguments are non-empty, they SHALL be appended after a blank line: raw for a custom command, and as `ARGUMENTS: <args>` for a skill.

#### Scenario: Positional args
- **WHEN** template `Fix issue $1 in $2` is run with `42 "auth module"`
- **THEN** the prompt is `Fix issue 42 in auth module`

#### Scenario: Argument text is not substituted again
- **WHEN** template `Fix $1 in $2` is run with `"$2" x`
- **THEN** the prompt is `Fix $2 in x`

#### Scenario: Command without placeholders
- **WHEN** a custom command `Review code` is run with `carefully`
- **THEN** the prompt is `Review code`, a blank line, and `carefully`

### Requirement: Shell output injection
(P1) The system SHALL replace each `` !`cmd` `` in a template with the stdout of running `cmd` in the sandbox. Each command SHALL pass the gates of a `bash` tool call first (the agent's tool list, skill tool restrictions and `PreToolUse` hooks, where a hook deny is a denial), then `bash` permission for the command text in the current mode, and an `ask` effect SHALL prompt before the command runs. Commands SHALL run concurrently with a 30 s timeout each, and output SHALL be capped at 20 KiB per command. Only spans written in the command or skill template SHALL run (argument placeholders inside a span are substituted first, each argument word as one quoted POSIX shell word, so its text is never interpreted by the shell); text supplied as arguments, command output and plain prompts SHALL never run. Expansion SHALL happen once, at admission of a planned command or skill, after `UserPromptSubmit` and before the input is committed; the expanded text SHALL be recorded with the admission, so replay and an exact retry never run a command again. A denied command (by rule, by a mode ceiling such as `plan`, or an unanswered or rejected ask, which `dont-ask` turns into a denial) SHALL be substituted as `[denied: <cmd>]`; when no sandbox can confine the command it SHALL be substituted as `[unavailable: sandbox]` and SHALL NOT run. Credential-shaped output SHALL be redacted. A project-controlled template (project commands and skills) that contains a shell injection SHALL be refused with a visible error until the user approves its exact digest for the checkout (`cyber commands trust <name>`); a changed template SHALL need a new approval. A forked skill SHALL expand before its text is handed to the child.

#### Scenario: Denied shell injection
- **WHEN** a template contains `` !`cat ~/.aws/credentials` `` and the rules deny it
- **THEN** the command is not run and the substitution is `[denied: cat ~/.aws/credentials]`

#### Scenario: Untrusted project template
- **WHEN** `.cyber/commands/status.md` contains `` !`git status` `` and its digest has not been approved for the checkout
- **THEN** `/status` is refused with an error naming `cyber commands trust status`, and nothing runs or is admitted

#### Scenario: Exact retry does not run again
- **WHEN** a client retries `POST /api/v1/sessions/:id/command` with the same message ID after the command's shell injection ran
- **THEN** the original receipt is returned and the command does not run a second time

### Requirement: File and agent references
(P0) Each `@path` token in an expanded template SHALL be resolved relative to the project root (or home for `~/`) and attached as a file or directory part when it exists. `@agent-name` matching a subagent SHALL become an agent invocation part, and other tokens SHALL remain text. A path SHALL be attached only after the same `external_directory` and `read` permission checks as the `read` tool, and a refused path SHALL NOT be attached. Tokens inside shell injection spans and command output SHALL NOT be resolved. `@server:uri` SHALL remain an MCP resource mention, except that a drive-letter path such as `@C:\x` is a path.

#### Scenario: File reference attached
- **WHEN** the template contains `@src/auth.rs`
- **THEN** the file is attached to the prompt

#### Scenario: Read-denied reference not attached
- **WHEN** the template contains `@.env` and a rule denies reading `.env`
- **THEN** nothing is attached for it and `@.env` remains text

### Requirement: Agent, model and subtask overrides
(P0) A command SHALL run with its `agent` when set, otherwise the caller's agent. Its model SHALL resolve as the command `model`, then the command agent's model (with the agent's `variant`), then the session model; a command `variant` SHALL apply to whichever model is selected. When `subtask` is true, or the agent's mode is `subagent`, the command SHALL run in a child session through the forked-skill route (captured with the admission, so an exact retry returns the original receipt and starts no second child) and only its summary SHALL be returned to the caller. An inline command keeps the Session's agent, so a command that names a different agent without running in a child session SHALL be refused with a visible error asking for `subtask: true`.

#### Scenario: Review as subtask
- **WHEN** `/review` has `subtask: true` and `agent: reviewer`
- **THEN** a child session with agent `reviewer` runs it and the parent receives the summary

#### Scenario: Agent model fallback
- **WHEN** `.cyber/commands/review-sub.md` has `subtask: true` and `agent: reviewer`, no `model`, and the `reviewer` profile sets `model: test/agent`
- **THEN** the child session runs on `test/agent`, not on the parent session's model

#### Scenario: Model cannot invoke the bundled review
- **WHEN** the model calls the skill tool with name `review` and no discovered skill overrides the bundled one
- **THEN** the call fails with `Skill "review" not found` and no reviewer child starts

#### Scenario: Bundled review receives a resolved diff
- **WHEN** the bundled `review` skill runs as `/review main..HEAD`
- **THEN** its `reviewer` child receives the host-resolved, permission-filtered diff in a data block and the findings structured-result schema, and does not fetch the diff itself

### Requirement: Argument hints and autocomplete data
(P0) The system SHALL expose for each command and skill its name, description, source, `argument_hint` (or derived placeholders) and namespace via `GET /api/v1/commands`, which clients use for slash autocomplete.

#### Scenario: Hints listed
- **WHEN** a client requests `GET /api/v1/commands`
- **THEN** `/db/migrate` is listed with `argument_hint: "<direction> [steps]"`

#### Scenario: Hint derived from placeholders
- **WHEN** `.cyber/commands/ship.md` has no hint and its template is `Ship $1 to $2`
- **THEN** `/ship` is listed with `argument_hint: "$1 $2"`

### Requirement: Built-in commands
(P0) The system SHALL provide built-in server commands `/init` (generate or refresh `AGENTS.md`), `/review` (review uncommitted changes, a branch or a PR), `/compact`, `/goal`, `/loop`, `/workflows`, `/tasks`, `/agents`, `/model`, `/mode`, `/memory`, `/rewind`, `/fork`, `/side` (alias `/btw`), `/remote`, `/share`, `/status`, `/cost`, `/doctor`, `/hooks`, `/mcp`, `/skills`, `/plugins` and `/help`. Their behavior SHALL be specified by the owning capability, and TUI-only commands SHALL be owned by the `tui` capability.

#### Scenario: Built-in available in exec mode
- **WHEN** `cyber exec --command init` runs in a repo without `AGENTS.md`
- **THEN** an `AGENTS.md` is generated through the normal permission flow

### Requirement: Bundled skills
(P1) The system SHALL bundle skills `review` (single-agent code review of a diff producing the structured findings defined by `vcs-integration`), `batch` (split a large change into 5-30 worktree-isolated subagents), `simplify` (review changed code for reuse and simplification), `security-review` (review pending changes for vulnerabilities) and `customize-cyber` (how to edit Cyber Code config, agents, skills, hooks and MCP). Each SHALL be replaceable by a discovered skill of the same name. The bundled `review` skill SHALL receive a host-resolved, permission-filtered diff in a data block instead of fetching it itself, and its child SHALL be created with the findings structured-result schema (`vcs-integration` → Local review command). Because only `/review` and `cyber review` resolve that target, the bundled `review` skill SHALL set `disable-model-invocation` so the model cannot invoke it through the skill tool without a diff; the other bundled skills remain model-invocable. An overriding `review` skill or command SHALL replace these behaviors.

#### Scenario: Override bundled skill
- **WHEN** a project defines `.cyber/skills/simplify/SKILL.md`
- **THEN** the project version replaces the bundled one

### Requirement: Record to skill
(P2) The system SHALL provide the bundled skill `record-to-skill`, which turns the current session's (or a selected range of) user-approved actions into a new `SKILL.md`. The draft SHALL include a description, ordered steps, the commands used and allowed-tools, be written to `.cyber/skills/<name>/` after the user confirms, and appear in a diff review before saving.

#### Scenario: Recorded deploy steps become a skill
- **WHEN** the user runs `/record-to-skill deploy-staging` after manually guiding a staging deploy
- **THEN** a draft `deploy-staging` skill is shown for review and saved on approval

### Requirement: Skill and command inspection
(P0) The system SHALL provide `/skills` and `cyber skills list|show <name>`, `cyber commands list`, and `GET /api/v1/skills`. These SHALL show each item's source path, scope, permission effect for the current agent, and any diagnostics. `GET /api/v1/skills?agent=<name>` (default `build`) SHALL return every loaded and skipped skill with `name`, `description`, `source`, `scope` (`builtin`, `user`, `project` or `config`), `status` (`loaded` or `invalid`), `user_invocable`, `model_invocable`, `argument_hint`, `permission` (`allow`, `ask` or `deny`; absent for a skipped skill) and `diagnostics` (parse errors and `replaces skill <name> from <path>` notices), plus `command_issues` for command definitions that were skipped and `preload_issues` for names in the agent's `skills` profile field that are not preloaded (`agents-subagents` → Agent definition fields). An unknown agent SHALL be refused with 400. Discovery SHALL run on each request, so edits are visible without a restart.

#### Scenario: Diagnose missing skill
- **WHEN** the user runs `cyber skills list`
- **THEN** skipped skills appear with their parse or duplicate diagnostics

#### Scenario: Permission effect per agent
- **WHEN** agent `ops` denies `skill` for `deploy*` and a client requests `GET /api/v1/skills?agent=ops`
- **THEN** `deploy` is listed with `permission: "deny"` and both the project and global paths in its duplicate notice

### Requirement: Path-triggered and forked skills
(P1) `SKILL.md` frontmatter MAY declare `paths` (globs relative to the project root), `context` (`inline`, default, or `fork`) and `disallowed-tools`. When a file matching `paths` is first read, edited or patched in a Context Epoch, the skill's name and description SHALL be appended to the tool output in a `<system-reminder>` block suggesting `skill` be called. A skill with `context: fork` run as a user command or by the model SHALL execute in a forked background subagent (`agents-subagents` → Forked subagents) and return only its summary. `disallowed-tools` patterns SHALL be denied while the skill is active, taking precedence over `allowed-tools`.

#### Scenario: Skill suggested by path
- **WHEN** skill `migrations` declares `paths: ["db/migrations/**"]` and the model edits `db/migrations/0042.sql`
- **THEN** the edit result ends with a reminder naming the `migrations` skill, once for the epoch

#### Scenario: Forked skill
- **WHEN** the user runs `/deep-audit` and that skill has `context: fork`
- **THEN** a forked background subagent runs the skill and the parent receives its summary as a queued handback
