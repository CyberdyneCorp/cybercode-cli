# system-context Specification

## Purpose
System Context builds the privileged instructions shown to the model from typed, independently observed Context Sources and keeps them byte-stable across Turns so provider prompt caches stay warm. It adopts the OpenCode v2 Context Epoch model and the instruction discovery of OpenCode v1, Claude Code (`CLAUDE.md`, `@path` imports, output styles), and Codex (`AGENTS.md`). Changes after the baseline arrive as durable mid-conversation system messages, never as rewrites of the cached prefix.

## Requirements

### Requirement: Typed Context Sources
(P0) The system SHALL model each Context Source as a stable key matching `^[a-z0-9]+(/[a-z0-9._-]+)+$`, a JSON codec for its value, an infallible loader, a pure baseline renderer, a pure update renderer, and an optional removal renderer. Built-in keys SHALL include `core/environment`, `core/date`, `core/instructions`, `core/skills`, `core/references`, `core/memory`, `core/mcp-instructions`, `core/messaging` (cross-session-messaging guidance, present only while the built-in messaging tools are advertised), `core/mode`, `core/goal`, and `core/output-style`, and `core/task-state` (compaction).

#### Scenario: Invalid key rejected
- **WHEN** a plugin registers a source with key `Bad Key`
- **THEN** registration fails with `ContextSourceKeyError`

#### Scenario: Messaging guidance follows the tools
- **WHEN** messaging is disabled for a Session's Location after its baseline was rendered
- **THEN** `send_message`, `list_sessions` and `watch_session` are no longer advertised and the next reconciliation withdraws `core/messaging`

### Requirement: Context Source registry
(P0) The system SHALL provide a Location-scoped registry where producers (built-ins, plugins, MCP) register scoped sources, load all sources concurrently, and compose them in ascending key order joined by blank lines. Registering a duplicate key SHALL fail, and closing a producer's scope SHALL remove its sources at the next observation.

#### Scenario: Plugin source removed
- **WHEN** a plugin that contributed `acme/conventions` is disabled
- **THEN** the next reconciliation renders the source's removal text

### Requirement: Environment and date sources
(P0) The system SHALL render `core/environment` as an `<env>` block with working directory, project root, whether it is a git repository, current branch, platform, shell, and active sandbox profile, and `core/date` as `Today's date: <YYYY-MM-DD>` in host-local time. Their update renderings SHALL state the newly effective values.

#### Scenario: Date changes overnight
- **WHEN** a Session continues past midnight
- **THEN** a mid-conversation system message states `Today's date is now: <new date>`

### Requirement: Instruction file discovery
(P0) The system SHALL load `core/instructions` from the global `~/.config/cyber/AGENTS.md`, then the global `~/.config/cyber/CYBER.md`. It SHALL then walk the directories from the project root down to the Location directory and load, in each one and in this order: `AGENTS.md`; `CLAUDE.md`, unless disabled and only when that directory has no `AGENTS.md`; `CONTEXT.md`; and `CYBER.md`, Cyber Code's own instruction file. More specific directories SHALL come later, and every file SHALL be deduplicated by canonical path. `CYBER_DISABLE_CLAUDE_MD=1` (or `compat.claude = false` in the resolved config of the Location, project layers included) SHALL disable `CLAUDE.md` loading and SHALL NOT disable `CYBER.md`; the runtime and `cyber doctor` SHALL read `compat.claude` from that same per-Location config. `CYBER_DISABLE_PROJECT_CONFIG=1` SHALL skip all project files, including `CYBER.md`, while the global files still load. The baseline, nested on-read discovery and `cyber doctor` SHALL derive their files from one ordered list of file names.

#### Scenario: Monorepo package rules
- **WHEN** the Location is `/repo/packages/api` and both `/repo/AGENTS.md` and `/repo/packages/api/AGENTS.md` exist
- **THEN** both are loaded, root first

#### Scenario: Claude Code project compatibility
- **WHEN** a project has only `CLAUDE.md`
- **THEN** its content is loaded as project instructions

#### Scenario: CYBER.md after the other files of its directory
- **WHEN** `/repo` contains `AGENTS.md`, `CONTEXT.md` and `CYBER.md`, and `/repo/packages/api` contains `AGENTS.md`
- **THEN** the order is `/repo/AGENTS.md`, `/repo/CONTEXT.md`, `/repo/CYBER.md`, then `/repo/packages/api/AGENTS.md`

#### Scenario: AGENTS.md, CLAUDE.md and CYBER.md in one directory
- **WHEN** `/repo` contains `AGENTS.md`, `CLAUDE.md` and `CYBER.md`
- **THEN** the order is `/repo/AGENTS.md` then `/repo/CYBER.md`, and `/repo/CLAUDE.md` is not loaded
- **AND** when `/repo/AGENTS.md` is removed, the order is `/repo/CLAUDE.md` then `/repo/CYBER.md`

#### Scenario: Global CYBER.md
- **WHEN** both `~/.config/cyber/AGENTS.md` and `~/.config/cyber/CYBER.md` exist
- **THEN** the global `CYBER.md` loads right after the global `AGENTS.md` and before any project file

#### Scenario: CLAUDE.md disabled
- **WHEN** `CYBER_DISABLE_CLAUDE_MD=1` and `/repo` contains `CLAUDE.md` and `CYBER.md`
- **THEN** only `/repo/CYBER.md` is loaded

#### Scenario: Project files disabled
- **WHEN** `CYBER_DISABLE_PROJECT_CONFIG=1` and both the global and the project `CYBER.md` exist
- **THEN** only the global `CYBER.md` is loaded

#### Scenario: CYBER.md symlinked to AGENTS.md
- **WHEN** `/repo/CYBER.md` is a symlink to `/repo/AGENTS.md`
- **THEN** the content is loaded once, as `/repo/AGENTS.md`

#### Scenario: Project compat.claude off
- **WHEN** `/repo/cyber.json` sets `compat.claude` to `false`, `/repo/CLAUDE.md` exists, and the server was started in another directory
- **THEN** neither the Session's provider request nor `cyber doctor` at `/repo` includes `/repo/CLAUDE.md`

### Requirement: Instruction imports
(P1) The system SHALL expand `@path` references on their own line inside instruction files, including `CYBER.md`. A path SHALL resolve relative to the containing file, or to home for `~/`. Expansion SHALL go up to a nesting depth of 5, skip paths inside code fences, detect cycles, and deduplicate by canonical path. Unresolved imports SHALL be recorded as warnings rather than failures. A project instruction file, or an import from one, that resolves canonically outside the project root SHALL NOT be loaded and SHALL be recorded as a warning; a symlink that stays inside the project SHALL keep working. A project instruction file or an import from one SHALL load only when the `read` permission rules of the Location (defaults plus configured rules) explicitly allow its discovered and canonical paths; a file they ask about (the default `*.env` rule) or deny SHALL NOT be loaded, raises no prompt, and SHALL be recorded as a warning. When the Location's config fails to load, no project instruction file SHALL be loaded and `CLAUDE.md` SHALL be treated as disabled. Only regular files SHALL be read: a FIFO, device or directory in place of an instruction file SHALL NOT be opened for reading and SHALL make the baseline unavailable (a nested read skips it with a warning), and no read SHALL go past the byte limit that applies to the file.

#### Scenario: Import cycle
- **WHEN** `AGENTS.md` imports `a.md` which imports `AGENTS.md`
- **THEN** the cycle is broken after the first inclusion and a warning names both files

#### Scenario: Import cycle through CYBER.md
- **WHEN** `CYBER.md` imports `docs/a.md` which imports `../CYBER.md`
- **THEN** each file is loaded once and a warning names both files

#### Scenario: Import outside the project
- **WHEN** a project `CYBER.md` imports `../secrets.md` outside the project root
- **THEN** the file is not loaded and a warning names the import

#### Scenario: Instruction file symlinked outside the project
- **WHEN** `/repo/CYBER.md` or `/repo/web/CYBER.md` is a symlink to a file outside `/repo`
- **THEN** that file is loaded neither into the baseline nor into a nested read reminder, and `cyber doctor` warns that it resolves outside the project

#### Scenario: Import of a protected file
- **WHEN** a project `CYBER.md` imports `@.env`, or `/repo/web/CYBER.md` is a symlink to `../.env` (or to a file a configured `read` deny covers), and the model reads `/repo/web/x.ts`
- **THEN** the secret reaches neither the baseline nor the nested read reminder, no permission prompt is raised, and `cyber doctor` warns that the import is not allowed by the read permission rules

#### Scenario: Malformed project config
- **WHEN** the global config sets `compat.claude` to `false` and `/repo/cyber.json` does not parse
- **THEN** `/repo/CLAUDE.md` is not loaded, and no other project instruction file is either

#### Scenario: Instruction file linked to a device
- **WHEN** `/repo/web/CYBER.md` is a symlink to `/dev/zero`, or a FIFO, and the model reads `/repo/web/x.ts`
- **THEN** the read settles normally, nothing is read from that file, and loading does not block; the same file in the baseline makes `core/instructions` unavailable without being read

### Requirement: Configured instructions
(P0) The system SHALL load extra entries from `instructions` config: file paths and globs resolved against the declaring config file's directory, and `http(s)` URLs fetched with a 5-second timeout and cached for the Context Epoch. Failed URLs SHALL be reported as Unavailable Context.

#### Scenario: Remote style guide
- **WHEN** `instructions` contains `https://example.com/style.md` and the fetch succeeds
- **THEN** its content appears as `Instructions from: https://example.com/style.md`

### Requirement: Nested rule files on read
(P1) The system SHALL, when the `read` tool successfully reads a file inside a subdirectory that contains instruction files from the ordered list (`AGENTS.md`, `CLAUDE.md`, `CONTEXT.md`, `CYBER.md`, or `AGENTS.override.md` in their place), append those files to the tool output inside a `<system-reminder>` block, once per file per Context Epoch. Files that are already in the baseline or already attached in uncompacted history, compared by canonical path, SHALL be skipped. Denied or failed reads SHALL load no files, and the files SHALL be read without holding the Session lock. One reminder SHALL carry at most 32768 bytes of instruction text; files past that SHALL be named by path instead. A reminder that does not fit the tool output budget SHALL be replaced by a list of the file paths, and when that does not fit either the read SHALL keep its own output; instruction files SHALL NOT turn a successful read into an error. The attached or named files SHALL be recorded durably with the tool settlement, and SHALL fire `InstructionsLoaded` with `load_reason: "nested_read"` and their `file_paths`.

#### Scenario: First read in a subpackage
- **WHEN** the model reads `/repo/web/src/app.ts` and `/repo/web/AGENTS.md` is not loaded yet
- **THEN** the read output ends with `/repo/web/AGENTS.md` inside a `<system-reminder>` block

#### Scenario: Nested CYBER.md attached once
- **WHEN** the model reads `/repo/web/x.ts` twice in one Context Epoch and `/repo/web/CYBER.md` exists
- **THEN** only the first read's output contains `/repo/web/CYBER.md` in a `<system-reminder>` block

#### Scenario: Two full-size nested files
- **WHEN** `/repo/web/AGENTS.md` and `/repo/web/CYBER.md` are each 32768 bytes and the model reads `/repo/web/x.ts` under the default tool output budget
- **THEN** the read settles successfully with its own content, `AGENTS.md` attached and `CYBER.md` named by path, and a later read under `/repo/web/` attaches neither again

#### Scenario: Nested files are observable
- **WHEN** a read attaches `/repo/web/CYBER.md`
- **THEN** an `InstructionsLoaded` hook runs with `load_reason` `nested_read` and `file_paths` `["/repo/web/CYBER.md"]`

### Requirement: Skills, references, and MCP instruction sources
(P1) The system SHALL render `core/skills` as an `<available_skills>` block of name and description for skills the selected agent may use, `core/references` as an `<available_references>` block for described project references, and `core/mcp-instructions` as an `<mcp_instructions>` block with one entry per connected server whose tools are not all denied. Each source SHALL be omitted when empty.

#### Scenario: Skill hidden by permission
- **WHEN** the agent's `skill` permission denies `deploy-*`
- **THEN** `deploy-prod` is not listed in `<available_skills>`

### Requirement: Mode and goal sources
(P2) The system SHALL render `core/mode` with the active permission mode's reminder (for example the read-only plan reminder in `plan` mode) and `core/goal` with the active goal condition and status, so mode and goal changes reach the model as mid-conversation updates.

#### Scenario: Entering plan mode
- **WHEN** the user switches to `plan` mode mid-session
- **THEN** the next Turn includes a system message stating that edits are not allowed until the plan is approved

### Requirement: Provider base prompts and output styles
(P0) The system SHALL start the system prompt with the agent's `system` text when set, otherwise with a base prompt selected by model family (OpenAI, Anthropic, Gemini, local/open-weights, default), and SHALL append the selected output style (`default`, `concise`, `explanatory`, `learning`, or a custom style from `~/.config/cyber/output-styles/*.md` or `.cyber/output-styles/*.md`).

#### Scenario: Custom output style
- **WHEN** the user selects output style `reviewer` defined in `.cyber/output-styles/reviewer.md`
- **THEN** its body is appended to the baseline under the next Context Epoch

### Requirement: Baseline initialization
(P0) The system SHALL initialize a Session's Context Epoch on its first Turn by observing all sources once and durably storing the rendered baseline, a model-hidden Context Snapshot, and the baseline sequence. If any source reports Unavailable Context, the Turn SHALL fail with `ContextInitializationBlocked` naming the source keys and the prompt SHALL stay retryable in the inbox.

#### Scenario: Unreadable AGENTS.md at start
- **WHEN** the project `AGENTS.md` cannot be read due to a transient I/O error
- **THEN** the first Turn fails with `ContextInitializationBlocked: core/instructions` and the prompt stays in the inbox

### Requirement: Baseline reuse within an epoch
(P0) The system SHALL reuse the stored baseline text verbatim for every Turn of the active Context Epoch, including across process restarts, so that changes to source values never rewrite the cached prefix within an epoch.

#### Scenario: Restart keeps the prefix
- **WHEN** the server restarts mid-session
- **THEN** the next Turn sends the same baseline bytes as before the restart

### Requirement: Reconciliation at Safe Boundaries
(P0) The system SHALL, at each Safe Boundary after initialization, observe sources once and compare them with the Context Snapshot, combining every changed, added, and removed source into a single durable `session.context.updated.1` event whose text is sent as a system-role message after newly promoted input, and advancing the snapshot in the same commit. A changed source SHALL never wake an idle Session.

#### Scenario: AGENTS.md edited mid-session
- **WHEN** the user edits `AGENTS.md` while a Session is idle and then sends a prompt
- **THEN** the next Turn contains one mid-conversation system message stating the new instructions replace the previous ones

### Requirement: Unavailable Context during reconciliation
(P0) The system SHALL keep the stored snapshot value and emit no update for a source reporting Unavailable Context during reconciliation, and SHALL keep omitting a source that has never loaded until it first loads.

#### Scenario: Temporarily unreachable URL
- **WHEN** a configured instruction URL times out after the baseline was built
- **THEN** the previously admitted instructions remain in effect and no removal message is sent

### Requirement: Epoch replacement
(P0) The system SHALL render a fresh baseline and start a new Context Epoch when a stored snapshot value no longer decodes, when a source without a removal renderer disappears, after a completed compaction, or after a model switch to a different provider family. Earlier mid-conversation system messages SHALL remain durable but SHALL be excluded from projected history.

#### Scenario: New epoch after compaction
- **WHEN** a compaction completes
- **THEN** the next Turn uses a freshly rendered baseline that includes all current sources

### Requirement: Context inspection
(P1) The system SHALL provide `/context` and `GET /api/v1/sessions/{id}/context` showing the active epoch, each source with its estimated token size, pending updates, and the total context utilization of the next request.

#### Scenario: Diagnose a large prompt
- **WHEN** the user runs `/context`
- **THEN** each source is listed with its token estimate, largest first

### Requirement: Corrupt snapshot handling
(P0) The system SHALL fail a Turn with `ContextSnapshotDecodeError` naming the Session, without contacting the provider, when the stored Context Snapshot itself cannot be decoded, and SHALL offer `cyber sessions repair-context <id>` to start a new epoch.

#### Scenario: Repair a corrupted snapshot
- **WHEN** the user runs `cyber sessions repair-context ses_1`
- **THEN** a new Context Epoch is created from current sources and the Session can continue

### Requirement: Path-scoped rules and overrides
(P1) The system SHALL load rule files from `.cyber/rules/*.md` and, unless `compat.claude` is false, `.claude/rules/*.md` in each config directory. A rule file without frontmatter `paths` SHALL join `core/instructions` in the baseline. A rule file with `paths: [globs]` (relative to its project root) SHALL be attached through the nested-instruction mechanism the first time per Context Epoch that a matching file is read, edited or patched, labelled with its path. An `AGENTS.override.md` in a directory SHALL take the `AGENTS.md` position and replace that directory's `AGENTS.md`, `CLAUDE.md` and `CYBER.md` instead of being appended; that directory's `CONTEXT.md` still loads. Each instruction or rule file, including `CYBER.md`, SHALL be truncated at `instructions.max_bytes` (default 32768) with a notice, and `cyber doctor` SHALL warn about truncated files. The baseline instruction text as a whole, imports included, SHALL be bounded (131072 bytes): the file that crosses the bound SHALL be cut with a notice, later files and imports SHALL be left out with a warning.

#### Scenario: Migration rules only for migrations
- **WHEN** `.cyber/rules/migrations.md` has `paths: ["db/migrations/**"]` and the model edits `db/migrations/0042.sql`
- **THEN** the edit result includes the rule file in a `<system-reminder>` block, once for the epoch, and unrelated edits never load it

#### Scenario: Override file replaces
- **WHEN** `/repo/packages/api/` contains both `AGENTS.md` and `AGENTS.override.md`
- **THEN** only `AGENTS.override.md` is loaded for that directory

#### Scenario: Override file replaces CYBER.md
- **WHEN** `/repo/packages/api/` contains `AGENTS.override.md`, `CYBER.md` and `CONTEXT.md`
- **THEN** `AGENTS.override.md` and then `CONTEXT.md` are loaded for that directory, and `CYBER.md` is not

#### Scenario: Oversized CYBER.md
- **WHEN** `/repo/CYBER.md` is larger than `instructions.max_bytes`
- **THEN** its content is cut at that size with a truncation notice naming the file, and `cyber doctor` warns about it

#### Scenario: Many imports
- **WHEN** `/repo/CYBER.md` imports enough files to exceed the total instruction budget
- **THEN** the baseline stops at the budget with a truncation notice, and a warning names each file left out
