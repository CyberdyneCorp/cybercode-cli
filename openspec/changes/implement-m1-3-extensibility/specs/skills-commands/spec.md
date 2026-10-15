## MODIFIED Requirements

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

### Requirement: Skill-scoped tool approvals and model
(P1) While a skill loaded in the current Turn declares `allowed-tools`, matching tool calls SHALL be allowed without prompting, unless a permission rule denies them. A skill `model` SHALL apply only when the skill is run as a user command or as a subtask.

#### Scenario: Pre-approved git commands
- **WHEN** skill `release` declares `allowed-tools: ["bash:git tag *"]` and the model runs `git tag v1.2.0`
- **THEN** no permission prompt is shown

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

### Requirement: Bundled skills
(P1) The system SHALL bundle skills `review` (single-agent code review of a diff producing the structured findings defined by `vcs-integration`), `batch` (split a large change into 5-30 worktree-isolated subagents), `simplify` (review changed code for reuse and simplification), `security-review` (review pending changes for vulnerabilities) and `customize-cyber` (how to edit Cyber Code config, agents, skills, hooks and MCP). Each SHALL be replaceable by a discovered skill of the same name. The bundled `review` skill SHALL receive a host-resolved, permission-filtered diff in a data block instead of fetching it itself, and its child SHALL be created with the findings structured-result schema (`vcs-integration` → Local review command). Because only `/review` and `cyber review` resolve that target, the bundled `review` skill SHALL set `disable-model-invocation` so the model cannot invoke it through the skill tool without a diff; the other bundled skills remain model-invocable. An overriding `review` skill or command SHALL replace these behaviors.

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
