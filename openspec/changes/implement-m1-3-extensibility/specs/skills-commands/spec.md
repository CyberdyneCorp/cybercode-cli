## MODIFIED Requirements

### Requirement: Remote skill sources
(P1) A `skills` entry that is an `https://` URL SHALL be treated as an index (`<url>/index.json` with `{ skills: [{ name, files, version? }] }`). Listed files SHALL be downloaded to `~/.cache/cyber/skills/<host>/<name>/`, refreshed at most every 24 h, and used from cache when offline. Fetch failures SHALL be logged and SHALL NOT block startup.

#### Scenario: Offline use of cached remote skill
- **WHEN** the network is unavailable and a remote skill was cached yesterday
- **THEN** the cached skill is loaded and a warning notes the stale cache

### Requirement: Skill-scoped tool approvals and model
(P1) While a skill loaded in the current Turn declares `allowed-tools`, matching tool calls SHALL be allowed without prompting, unless a permission rule denies them. A skill `model` SHALL apply only when the skill is run as a user command or as a subtask.

#### Scenario: Pre-approved git commands
- **WHEN** skill `release` declares `allowed-tools: ["bash:git tag *"]` and the model runs `git tag v1.2.0`
- **THEN** no permission prompt is shown

### Requirement: Shell output injection
(P1) The system SHALL replace each `` !`cmd` `` in a template with the stdout of running `cmd` in the sandbox. It SHALL first evaluate `bash` permission for the command text in the current mode, and an `ask` effect SHALL prompt before the command runs. Commands SHALL run concurrently with a 30 s timeout each, and output SHALL be capped at 20 KiB per command.

#### Scenario: Denied shell injection
- **WHEN** a template contains `` !`cat ~/.aws/credentials` `` and the rules deny it
- **THEN** the command is not run and the substitution is `[denied: cat ~/.aws/credentials]`

### Requirement: Bundled skills
(P1) The system SHALL bundle skills `review` (single-agent code review of a diff producing the structured findings defined by `vcs-integration`), `batch` (split a large change into 5-30 worktree-isolated subagents), `simplify` (review changed code for reuse and simplification), `security-review` (review pending changes for vulnerabilities) and `customize-cyber` (how to edit Cyber Code config, agents, skills, hooks and MCP). Each SHALL be replaceable by a discovered skill of the same name.

#### Scenario: Override bundled skill
- **WHEN** a project defines `.cyber/skills/simplify/SKILL.md`
- **THEN** the project version replaces the bundled one

### Requirement: Path-triggered and forked skills
(P1) `SKILL.md` frontmatter MAY declare `paths` (globs relative to the project root), `context` (`inline`, default, or `fork`) and `disallowed-tools`. When a file matching `paths` is first read, edited or patched in a Context Epoch, the skill's name and description SHALL be appended to the tool output in a `<system-reminder>` block suggesting `skill` be called. A skill with `context: fork` run as a user command or by the model SHALL execute in a forked background subagent (`agents-subagents` → Forked subagents) and return only its summary. `disallowed-tools` patterns SHALL be denied while the skill is active, taking precedence over `allowed-tools`.

#### Scenario: Skill suggested by path
- **WHEN** skill `migrations` declares `paths: ["db/migrations/**"]` and the model edits `db/migrations/0042.sql`
- **THEN** the edit result ends with a reminder naming the `migrations` skill, once for the epoch

#### Scenario: Forked skill
- **WHEN** the user runs `/deep-audit` and that skill has `context: fork`
- **THEN** a forked background subagent runs the skill and the parent receives its summary as a queued handback
