# memory Specification

## Purpose
Memory lets Cyber Code keep durable, file-based notes about the user, their preferences, and their projects across Sessions, so corrections and context don't have to be repeated. It follows Claude Code's auto-memory (one fact per file plus a `MEMORY.md` index loaded every session) and Codex's `/memories` toggles. Memory reaches the model through the `core/memory` Context Source, which keeps prompt caching intact.

## Requirements

### Requirement: Memory locations
(P1) The system SHALL keep project memory in `~/.local/share/cyber/memory/<project_id>/` and global memory in `~/.local/share/cyber/memory/global/`, each containing one Markdown file per memory and a `MEMORY.md` index. The `global` project ID SHALL map to the global directory.

#### Scenario: Project memory directory
- **WHEN** a Session runs in a repository with project ID `prj_7f3a`
- **THEN** project memories are read from and written to `~/.local/share/cyber/memory/prj_7f3a/`

### Requirement: Memory file format
(P1) The system SHALL store each memory as a Markdown file with YAML frontmatter `name` (kebab-case slug, unique per directory), `description` (one line), and `type` (`user`, `feedback`, `project`, or `reference`), followed by the fact. `feedback` and `project` memories SHALL include `**Why:**` and `**How to apply:**` lines. Files with invalid frontmatter SHALL be skipped with a warning.

#### Scenario: Invalid frontmatter
- **WHEN** a memory file lacks `type`
- **THEN** it is not loaded and `cyber debug memory` lists it as invalid

### Requirement: Index loading
(P1) The system SHALL load each directory's `MEMORY.md` into the `core/memory` Context Source at baseline initialization, truncated to the first 200 lines or 25,000 bytes, whichever is smaller, with a truncation notice. Individual memory files SHALL be read on demand through the memory tool, not loaded in full.

#### Scenario: Oversized index
- **WHEN** `MEMORY.md` has 340 lines
- **THEN** the first 200 lines are loaded followed by `[memory index truncated; read MEMORY.md for more]`

### Requirement: Memory tool
(P1) The system SHALL provide a `memory` tool with operations `list`, `read`, `write`, `update`, and `delete`, gated by the `memory` permission (default `allow` for the memory directories). `write` and `update` SHALL keep the `MEMORY.md` index in sync with one line per memory (`- [Title](file.md) — hook`).

#### Scenario: Write keeps the index in sync
- **WHEN** the model writes a new `feedback` memory `prefers-small-prs`
- **THEN** `prefers-small-prs.md` is created and a line linking it is appended to `MEMORY.md`

### Requirement: Automatic memory generation
(P1) When `memory.generate` is true (default true), the system SHALL instruct the model, through the `core/memory` source, to save durable user preferences, corrections, and non-obvious project facts, and SHALL NOT save content derivable from the repository, git history, or instruction files, or facts that matter only to the current conversation.

#### Scenario: Correction becomes feedback memory
- **WHEN** the user says "never use mocks for the database in tests" and explains why
- **THEN** the model may write a `feedback` memory containing the rule, the reason, and when to apply it

### Requirement: Memory toggles
(P1) The system SHALL honor `memory.enabled` (default true; when false, no memory is loaded and the tool is not offered) and `memory.generate` (when false, memory is loaded read-only and `write`, `update`, and `delete` are not offered), exposed through `/memory on|off|readonly` and the `CYBER_DISABLE_MEMORY` environment variable.

#### Scenario: Read-only memory
- **WHEN** the user runs `/memory readonly`
- **THEN** the index is still loaded but the memory tool offers only `list` and `read`

### Requirement: Secret redaction
(P1) The system SHALL reject a memory write whose content matches secret patterns (API keys, tokens, private keys, passwords, high-entropy strings over 32 characters) with `Memory rejected: content looks like a secret`, and SHALL never write credentials into memory.

#### Scenario: API key in a memory
- **WHEN** the model tries to save `OPENAI_API_KEY=sk-...` as a reference memory
- **THEN** the write fails and nothing is stored

### Requirement: Deduplication and updates
(P1) The system SHALL require the model to check existing memories (by name and description) before writing, update an existing file instead of creating a duplicate when the `name` matches, and delete memories the user identifies as wrong.

#### Scenario: Duplicate name
- **WHEN** the model writes memory `name: test-db-policy` and that name already exists
- **THEN** the existing file is updated and no second file is created

### Requirement: Staleness notice
(P1) The system SHALL present loaded memories as background context reflecting what was true when written, and SHALL instruct the model to verify any file, function, or flag a memory names before recommending it.

#### Scenario: Memory names a removed function
- **WHEN** a memory references `utils/legacyAuth.ts` which no longer exists
- **THEN** the model is expected to check the file before relying on it and update or delete the memory

### Requirement: Memory command
(P1) The system SHALL provide `/memory` in the TUI and `cyber memory list|show|edit|delete|path [--global]` on the CLI, where `edit` opens the file in `$EDITOR` and `path` prints the directory.

#### Scenario: Open project memory folder
- **WHEN** the user runs `cyber memory path`
- **THEN** the project memory directory path is printed

### Requirement: Memory changes during a Session
(P1) The system SHALL treat memory index changes as Context Source changes reconciled at the next Safe Boundary, so a memory written in one Session reaches other active Sessions of the same project as a mid-conversation system message.

#### Scenario: Two sessions in one project
- **WHEN** Session A writes a project memory while Session B is active in the same project
- **THEN** Session B receives the updated index at its next Safe Boundary

### Requirement: Explicit remember requests
(P1) The system SHALL treat user requests such as "remember that ..." as a request to write or update a memory of the appropriate type, and requests to forget as a request to delete the matching memory, confirming the result in the reply.

#### Scenario: User asks to forget
- **WHEN** the user says "forget the note about using pnpm"
- **THEN** the matching memory file and its index line are deleted

### Requirement: Memory HTTP API
(P1) The server SHALL expose `GET /api/v1/memory?scope=project|global`, `GET|PUT|DELETE /api/v1/memory/{scope}/{name}`, and publish `memory.updated` after each change.

#### Scenario: Web client edits a memory
- **WHEN** a web client sends `PUT /api/v1/memory/project/prefers-small-prs` with new content
- **THEN** the file and index are updated and `memory.updated` is published

### Requirement: Memory in sandboxed and remote runs
(P3) The system SHALL load memory read-only in Sessions running on a remote Runner unless `memory.sync` is enabled for that Runner, and SHALL never upload memory to a Relay or share service.

#### Scenario: Hosted runner session
- **WHEN** a Session runs on a Cyber Cloud Runner without `memory.sync`
- **THEN** no local memory content is sent to the Runner
