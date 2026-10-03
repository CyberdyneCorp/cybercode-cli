# builtin-tools Specification

## Purpose
Defines the built-in tools Cyber Code exposes to models: parameters, limits, permission action/resource, and output format. The baseline is the twelve tools shared by OpenCode v1 and v2 (`read`, `write`, `edit`, `apply_patch`, `bash`, `glob`, `grep`, `webfetch`, `websearch`, `todo`, `question`, `skill`). It adds Claude Code's notebook, LSP, monitor and task-list tools, Codex's `apply_patch` preference for GPT-family models, and an `advisor` tool that consults a stronger model.

## Requirements

### Requirement: Built-in tool set
(P0) The system SHALL register these built-in tools in every Location: `read`, `write`, `edit`, `apply_patch`, `glob`, `grep`, `list`, `bash`, `webfetch`, `websearch`, `todo`, `question`, `skill`, `notebook_edit`, `history_search` (compaction), and the hidden `invalid`. It SHALL add `powershell` on Windows, `lsp` when code intelligence is enabled (P1), `monitor` (P1), and `advisor` when `model_roles.advisor` is configured (P2). The registry SHALL offer `apply_patch` instead of `edit`/`write` to models whose catalog entry sets `capabilities.prefers_apply_patch: true`, and `edit`/`write` to all other models.

#### Scenario: GPT-family model gets apply_patch
- **WHEN** the Turn model's catalog entry has `prefers_apply_patch: true`
- **THEN** the advertised tools include `apply_patch` and exclude `edit` and `write`

### Requirement: Capability-owned tool catalog
(P1) Besides the tools above, the registry SHALL register the following model-facing tools. Each is specified by the capability in parentheses and is advertised only when that capability is enabled and its phase is implemented:
- `agent`, `return_result` (agents-subagents)
- `workflow` (workflows)
- `schedule_wakeup`, `cron_create`, `cron_list`, `cron_delete` (loops-scheduling)
- `monitor`, `task_stop`, `pty_start`, `pty_write`, `pty_read`, `notify`, `send_file` (background-tasks)
- `team_spawn`, `team_merge`, `task_create`, `task_update`, `task_list`, `task_get` (agent-teams)
- `list_sessions`, `send_message`, `watch_session` (cross-session-messaging)
- `channel_reply` (channels)
- `memory` (memory)
- `wait_for_mcp`, `tool_search`, `mcp_list_resources`, `mcp_list_resource_templates`, `mcp_read_resource` (mcp and tool-registry)
- `publish_artifact` (session-sharing)
- `advisor` (provider-catalog)

All of these SHALL use the shared tool-registry contract: schema validation, output budget, permission assertion with the tool name as the action unless the owning spec states otherwise, and hiding when fully denied.

#### Scenario: Phase-gated tool hidden
- **WHEN** the agent-teams capability is disabled (`experimental.teams` unset)
- **THEN** `team_spawn`, `team_merge` and the `task_*` tools are not advertised to the model, and a call to them settles as `Unknown tool: team_spawn`

#### Scenario: Cross-capability tool obeys permissions
- **WHEN** config denies `send_message` for resource `*` in agent `explore`
- **THEN** `send_message`, `list_sessions` and `watch_session` remain subject to their own rules, and `send_message` is omitted from the `explore` agent's advertised tools

### Requirement: Path resolution and external directories
(P0) Path-taking tools SHALL resolve relative paths against the Location directory. A relative path that escapes the Location, or a symlink whose canonical target escapes it, SHALL require an `external_directory` permission on `<canonical dir>/*` before the tool's own permission. Cyber's tool-output, temp, skill and worktree directories SHALL be allowed by default.

#### Scenario: Absolute path outside project
- **WHEN** `read` targets `/etc/hosts` from Location `/repo`
- **THEN** an `external_directory` request for `/etc/*` is evaluated before the `read` permission

### Requirement: read tool
(P0) `read` SHALL accept `{ path, offset? (1-based line), limit? (default 2000) }`, check the `read` permission on the Location-relative path, and return text as `<n>: <line>` lines. Pages SHALL hold at most 2000 lines and 50 KiB, lines longer than 2000 characters SHALL be cut with `…[truncated]`, and when more remains the output SHALL include `next_offset`. A directory SHALL be listed (directories first, `/` suffix). PNG, JPEG, GIF and WebP images and PDFs (≤ 20 pages per call via `pages`) SHALL be returned as media parts, and `.ipynb` as cells with outputs. Detected binaries SHALL be refused with `Binary file: <mime>`. A missing path SHALL fail with `File not found` plus up to 3 similar sibling names.

#### Scenario: Large file paged
- **WHEN** `read` targets a 10000-line file without offset
- **THEN** lines 1–2000 are returned with `next_offset: 2001`

#### Scenario: Nested instructions attached
- **WHEN** `read` targets a file under a directory containing an unloaded `AGENTS.md`
- **THEN** that file's content is appended in a `<system-reminder>` block once per Context Epoch

### Requirement: write tool
(P0) `write` SHALL accept `{ path, content }` and check the `edit` permission (shown with a diff against the current content). It SHALL create parent directories, preserve an existing UTF-8 BOM and line-ending style, and return `Created <path>` or `Wrote <path> (<n> lines)`. Overwriting an existing file the Session has not read SHALL fail with `Read the file before overwriting it.`

#### Scenario: Blind overwrite refused
- **WHEN** the model writes to an existing file it never read in this Session
- **THEN** the call fails with `Read the file before overwriting it.`

### Requirement: edit tool
(P0) `edit` SHALL accept `{ path, old_string, new_string, replace_all? = false }` and check the `edit` permission with a unified diff. `old_string` SHALL match exactly and uniquely unless `replace_all` is set; identical or empty `old_string` SHALL be rejected. The tool SHALL preserve EOL style and BOM. The write SHALL commit only if the file bytes still equal those read before approval, and otherwise fail with `File changed after permission approval. Read it again before editing.` Output SHALL include the diff and the replacement count.

#### Scenario: Ambiguous match
- **WHEN** `old_string` occurs 3 times and `replace_all` is false
- **THEN** the call fails with `Found 3 matches; provide more context or set replace_all`

#### Scenario: Concurrent external change
- **WHEN** the file is modified by the user while the edit approval prompt is open
- **THEN** after approval the edit fails with `File changed after permission approval. Read it again before editing.`

### Requirement: apply_patch tool
(P0) `apply_patch` SHALL accept `{ patch }` in the `*** Begin Patch` / `*** End Patch` envelope with `*** Add File:`, `*** Update File:` (optional `*** Move to:`) and `*** Delete File:` sections. It SHALL resolve and approve external directories and one `edit` batch covering all targets, validate every hunk before writing, and apply operations in order. A later failure SHALL report `Patch partially applied before failing at <path>. Applied: <list>`. Success SHALL return `A|M|D <path>` lines.

#### Scenario: Hunk validation before write
- **WHEN** a patch's second hunk does not match the file
- **THEN** no file is modified and the error names the failing hunk

### Requirement: Serialized file mutation
(P0) The system SHALL serialize writes, edits, patches and deletions per canonical path within the server process. Conditional writes SHALL compare and write under the same lock.

#### Scenario: Two subagents edit one file
- **WHEN** two subagents edit the same file concurrently
- **THEN** the edits apply one after the other and the second sees the first's content

### Requirement: glob, grep and list tools
(P0) `glob` SHALL accept `{ pattern, path? }` and return up to 100 absolute paths sorted by modification time, newest first. `grep` SHALL accept `{ pattern (regex), path?, include?, context_lines? (0–5), limit? (default 100) }` and return matches grouped by file as `Line <n>: <text>` under `Found <n> matches`. `list` SHALL accept `{ path, depth? (default 1, max 3) }`. All three SHALL use ripgrep (from PATH, else the copy bundled in the binary), respect `.gitignore`/`.ignore`, exclude `.git/`, and check their own permission action with the pattern or path as resource.

#### Scenario: No matches
- **WHEN** `grep` finds nothing
- **THEN** it returns `No matches found`

### Requirement: bash tool
(P0) `bash` SHALL accept `{ command, timeout_ms? (default 120000, max 600000), workdir?, description?, background? }`. It SHALL run in the configured `shell`, else `$SHELL` when acceptable (bash, zsh, sh, dash), else `/bin/sh`. Stdin SHALL be closed and stdout/stderr combined. The environment SHALL carry `CYBER=1`, `CYBER_SESSION_ID` and `CYBER_PID`. The tool SHALL run inside the sandbox per the `sandbox` capability. Capture SHALL be limited to 1 MiB, keeping the tail, with the full output in a managed file. The exit code SHALL be reported as `exit_code` metadata, and empty output SHALL be returned as `(no output)`. `background: true` SHALL start a background task and return its `job_` ID immediately.

#### Scenario: Timeout
- **WHEN** a command exceeds `timeout_ms`
- **THEN** its process tree is killed and the result ends with `Command timed out after <n> ms`

#### Scenario: Background command
- **WHEN** the model calls `bash` with `background: true` for `npm run dev`
- **THEN** the call returns `job_<id>` immediately and the output streams into the background task

### Requirement: Bash permission analysis
(P0) The system SHALL parse every command with tree-sitter (bash, or PowerShell for `powershell`). It SHALL evaluate the `bash` permission once per simple command, using its source text as resource, and offer an "always" pattern of the command's arity prefix plus ` *` (for example `git commit *`, `npm run *`). Path arguments of file-mutating commands (`rm`, `mv`, `cp`, `mkdir`, `touch`, `chmod`, `chown`, `ln`, `tee`, redirections) that resolve outside the Location SHALL first raise `external_directory`. Unparseable commands SHALL be evaluated as one resource and never auto-allowed by prefix rules.

#### Scenario: Compound command evaluated per part
- **WHEN** the command is `npm test && git push`
- **THEN** `npm test` and `git push` are evaluated separately and a deny on `git push *` blocks the call

### Requirement: powershell tool
(P1) On Windows, `powershell` SHALL run commands natively in `pwsh`, falling back to `powershell.exe`, with the same parameters, limits, sandboxing and permission analysis as `bash`. The permission action SHALL be `bash` so a single ruleset covers both.

#### Scenario: PowerShell on Windows
- **WHEN** the server runs on Windows and `pwsh` is installed
- **THEN** `powershell` is advertised and its calls are checked against `bash` rules

### Requirement: webfetch tool
(P0) `webfetch` SHALL accept `{ url, format? = "markdown" | "text" | "html", timeout_s? (default 30, max 120), prompt? }` for `http`/`https` URLs only. It SHALL check the `webfetch` permission on the URL (and `network` when sandboxed offline), reject bodies over 5 MiB, return images as media parts, convert HTML per `format`, and retry once with user agent `cyber` on a Cloudflare challenge. When `prompt` is given, the content SHALL be summarized by `model_roles.small` and the summary returned.

#### Scenario: Non-HTTP scheme
- **WHEN** the URL is `file:///etc/passwd`
- **THEN** the call fails with `Only http and https URLs are supported`

### Requirement: websearch tool
(P0) `websearch` SHALL accept `{ query, max_results? (default 8, max 20), allowed_domains?, blocked_domains? }` and check the `websearch` permission on the query. It SHALL use the model provider's native search when the Turn model's catalog entry declares `capabilities.native_web_search`, and otherwise the configured backend `tools.websearch.backend` (`exa`, `brave`, `searxng`, `parallel`), with credentials from `provider-credentials`. Requests SHALL time out after 25 s and responses SHALL be limited to 256 KiB. Results SHALL be returned as `title`, `url`, `snippet`. Without any backend, the tool SHALL be hidden.

#### Scenario: Native search preferred
- **WHEN** the Turn model declares `native_web_search`
- **THEN** the search is executed by the provider and no third-party backend is contacted

### Requirement: todo tool
(P0) `todo` SHALL manage the Session task list with operations `create { subject, description?, blocked_by?: id[] }`, `update { id, status?: pending|in_progress|completed|deleted, ... }`, `list` and `get { id }`. It SHALL check the `todo` permission and return the full list after each mutation. At most one task SHALL be `in_progress` per agent. Task lists SHALL persist with the Session and be visible to agent-team members per `agent-teams`.

#### Scenario: Blocked task
- **WHEN** a task is created with `blocked_by: ["t1"]` and `t1` is pending
- **THEN** `list` shows it as blocked until `t1` is completed

### Requirement: question tool
(P0) `question` SHALL accept 1–4 questions, each with `{ question, header (≤12 chars), options (2–4 with label, description), multi_select?, allow_custom? = true }`. It SHALL check the `question` permission and block until answered, returning one array of selected labels or custom text per question. Dismissal SHALL halt the Drain, and in `exec` mode without a TTY the tool SHALL be denied by default.

#### Scenario: User answers with custom text
- **WHEN** the user picks "Other" and types a value
- **THEN** that text is returned as the answer for that question

### Requirement: skill tool
(P0) `skill` SHALL accept `{ name, args? }`, check the `skill` permission on the name, and return the skill body wrapped in `<skill name="...">`, with its base directory and up to 10 sampled sibling file paths. An unknown name SHALL fail with `Skill "<name>" not found. Available: <names>`.

#### Scenario: Load a skill
- **WHEN** the model calls `skill` with `name: "release-notes"`
- **THEN** the body of that skill's `SKILL.md` and its base directory are returned

### Requirement: notebook_edit tool
(P1) `notebook_edit` SHALL accept `{ path, cell_id?, cell_index?, new_source, cell_type?, mode: replace|insert|delete }` for `.ipynb` files. It SHALL check the `edit` permission, preserve notebook metadata and outputs of untouched cells, and clear the outputs of edited code cells.

#### Scenario: Insert markdown cell
- **WHEN** `mode` is `insert` with `cell_type: "markdown"` at index 0
- **THEN** a new markdown cell becomes the first cell and the other cells are unchanged

### Requirement: monitor tool
(P1) `monitor` SHALL accept `{ command | websocket_url, pattern?, max_lines? (default 200), until? }` and start a background watcher whose matching output lines are admitted into the Session as Mid-Conversation System Messages at Safe Boundaries. It SHALL return the `job_` ID and check the `bash` (or `network`) permission.

#### Scenario: React to a log line
- **WHEN** the model monitors `tail -f app.log` with pattern `ERROR`
- **THEN** each new `ERROR` line is delivered to the model at the next Safe Boundary

### Requirement: advisor tool
(P2) When `model_roles.advisor` is set, `advisor` SHALL accept `{ question, context_files?: string[] }` and send the question plus a compacted transcript summary and the listed files to the advisor model. It SHALL return the advisor's answer, record the advisor's token cost on the Session, and check the `advisor` permission.

#### Scenario: Consulting a stronger model
- **WHEN** the session model calls `advisor` on a design decision
- **THEN** the answer from the advisor model is returned and its cost is added to the Session usage

### Requirement: Tool output formats are stable contracts
(P0) Each built-in tool SHALL document its output format and annotation flags in the generated OpenAPI/tool schema bundle at `GET /api/v1/tools/schema`. Changes to the parameter names of built-in tools SHALL be versioned under the `tools.v<N>` contract with a 1-minor-release deprecation window.

#### Scenario: Schema bundle available
- **WHEN** a client fetches `GET /api/v1/tools/schema`
- **THEN** it receives each built-in tool's input schema, output description and annotations
