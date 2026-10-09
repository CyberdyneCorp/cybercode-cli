## MODIFIED Requirements

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

### Requirement: Secret redaction
(P1) The system SHALL reject a memory write whose content matches secret patterns (API keys, tokens, private keys, passwords, high-entropy strings over 32 characters) with `Memory rejected: content looks like a secret`, and SHALL never write credentials into memory.

#### Scenario: API key in a memory
- **WHEN** the model tries to save `OPENAI_API_KEY=sk-...` as a reference memory
- **THEN** the write fails and nothing is stored


#### Scenario: Quoted credential text refused
- **WHEN** a proposed memory body or frontmatter contains a quoted JSON or YAML credential assignment
- **THEN** shared write validation SHALL refuse it with `Memory rejected: content looks like a secret` without echoing the credential

### Requirement: Memory locations
(P1) The system SHALL keep project memory in `~/.local/share/cyber/memory/<project_id>/` and global memory in `~/.local/share/cyber/memory/global/`, each containing one Markdown file per memory and a `MEMORY.md` index. The `global` project ID SHALL map to the global directory.

#### Scenario: Project memory directory
- **WHEN** a Session runs in a repository with project ID `prj_7f3a`
- **THEN** project memories are read from and written to `~/.local/share/cyber/memory/prj_7f3a/`

#### Scenario: Bound storage scope reads
- **WHEN** a memory scope or note path is a symlink alias, or a read observes pending transaction evidence
- **THEN** the system SHALL refuse the read without following the alias or removing recovery evidence
- **AND** baseline index reads SHALL preserve the UTF-8 200-line/25,000-byte prefix without loading individual note bodies

#### Scenario: Interrupted note and index mutation
- **WHEN** the storage owner exits after preparing or partially installing a note/index mutation
- **THEN** ordinary reads SHALL remain fenced by retained transaction evidence
- **AND** explicit recovery under exclusive scope ownership SHALL finish only fingerprint-matched effects without adopting the previous execution owner
- **AND** the acknowledged result SHALL have matching note and generated index content

#### Scenario: User edits conflict with recovery
- **WHEN** a target, index, other valid note or archived original changes after preparation
- **THEN** recovery SHALL preserve the changed files and transaction evidence and refuse acknowledgement
- **AND** create-only installation SHALL refuse an unexpected destination instead of overwriting it

#### Scenario: Interrupted staging link
- **WHEN** interruption leaves an exact two-link staged/destination pair with the expected inode and content
- **THEN** recovery SHALL remove only its staging alias before completing the mutation
- **AND** an additional external hard-link alias SHALL cause refusal without deleting the alias

### Requirement: Memory toggles
(P1) The system SHALL honor `memory.enabled` (default true; when false, no memory is loaded and the tool is not offered) and `memory.generate` (when false, memory is loaded read-only and `write`, `update`, and `delete` are not offered), exposed through `/memory on|off|readonly` and the `CYBER_DISABLE_MEMORY` environment variable.

#### Scenario: Read-only memory
- **WHEN** the user runs `/memory readonly`
- **THEN** the index is still loaded but the memory tool offers only `list` and `read`

#### Scenario: Invalid memory toggle type
- **WHEN** loaded configuration supplies nonboolean `memory.enabled` or `memory.generate`, or a nonobject `memory` value
- **THEN** configuration loading SHALL fail with a memory validation error without echoing supplied values

### Requirement: Memory tool
(P1) The system SHALL provide a `memory` tool with operations `list`, `read`, `write`, `update`, and `delete`, gated by the `memory` permission (default `allow` for the memory directories). `write` and `update` SHALL keep the `MEMORY.md` index in sync with one line per memory (`- [Title](file.md) — hook`).

#### Scenario: Write keeps the index in sync
- **WHEN** the model writes a new `feedback` memory `prefers-small-prs`
- **THEN** `prefers-small-prs.md` is created and a line linking it is appended to `MEMORY.md`

#### Scenario: Disabled or read-only direct dispatch
- **WHEN** a caller invokes memory despite disabled settings, or invokes a mutation while generation is disabled or Plan Mode is active
- **THEN** dispatch SHALL refuse storage effects even if the caller retained an earlier tool definition

#### Scenario: Project fallback retains global scope permissions
- **WHEN** project memory resolves to the global directory outside Git
- **THEN** tool dispatch SHALL also require global scope authorization before storage effects
- **AND** baseline observation SHALL not load the global index through a permitted project alias when global permission is denied or requires approval

#### Scenario: Scope authorization before storage effects
- **WHEN** a memory operation is denied or its admission is cancelled
- **THEN** the tool SHALL NOT create memory directories, journals or note files
- **AND** a started mutation SHALL retain its owner until the actual storage outcome is acknowledged

### Requirement: Automatic memory generation
(P1) When `memory.generate` is true (default true), the system SHALL instruct the model, through the `core/memory` source, to save durable user preferences, corrections, and non-obvious project facts, and SHALL NOT save content derivable from the repository, git history, or instruction files, or facts that matter only to the current conversation.

#### Scenario: Correction becomes feedback memory
- **WHEN** the user says "never use mocks for the database in tests" and explains why
- **THEN** the model may write a `feedback` memory containing the rule, the reason, and when to apply it

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

### Requirement: Memory changes during a Session
(P1) The system SHALL treat memory index changes as Context Source changes reconciled at the next Safe Boundary, so a memory written in one Session reaches other active Sessions of the same project as a mid-conversation system message.

#### Scenario: Two sessions in one project
- **WHEN** Session A writes a project memory while Session B is active in the same project
- **THEN** Session B receives the updated index at its next Safe Boundary

#### Scenario: Temporarily unavailable memory index
- **WHEN** a memory index is unsafe, unreadable, busy or fenced by a pending transaction
- **THEN** the context observation SHALL be unavailable instead of absent
- **AND** an initial Epoch SHALL remain retryable while an existing Epoch preserves its previous source

#### Scenario: Memory withdrawal and immutable baseline
- **WHEN** memory is disabled after a Session has loaded its index
- **THEN** the next Safe Boundary SHALL record a withdrawal through Context Source reconciliation
- **AND** the immutable baseline SHALL remain unchanged within its Epoch

### Requirement: Memory command
(P1) The system SHALL provide `/memory` in the TUI and `cyber memory list|show|edit|delete|path [--global]` on the CLI, where `edit` opens the file in `$EDITOR` and `path` prints the directory.

#### Scenario: Open project memory folder
- **WHEN** the user runs `cyber memory path`
- **THEN** the project memory directory path is printed


#### Scenario: Editor review preserves concurrent edits
- **WHEN** an external edit changes a target or its index while the user edits a private draft
- **THEN** CLI commit SHALL refuse memory effects and preserve the draft and changed files

#### Scenario: Editor content requires write validation
- **WHEN** edited content is malformed, contains secrets or changes the requested note identity
- **THEN** no note/index mutation SHALL be admitted
- **AND** diagnostics SHALL omit the supplied content while retaining the draft for the user

### Requirement: Memory HTTP API
(P1) The server SHALL expose `GET /api/v1/memory?scope=project|global`, `GET|PUT|DELETE /api/v1/memory/{scope}/{name}`, and publish `memory.updated` after each change.

#### Scenario: Web client edits a memory
- **WHEN** a web client sends `PUT /api/v1/memory/project/prefers-small-prs` with new content
- **THEN** the file and index are updated and `memory.updated` is published

#### Scenario: Authenticated explicit memory review
- **WHEN** an authenticated user lists or reads project or global memory through HTTP
- **THEN** the server SHALL use Location routing and the shared bounded directory-bound storage under a scope claim
- **AND** absent listing SHALL return an empty catalog without creating a memory directory
- **AND** invalid scope/name, busy or pending storage and unsafe aliases SHALL refuse with tagged errors without echoing note content or automatically recovering transactions
- **AND** explicit user review SHALL remain available when automatic model memory is disabled

#### Scenario: Durable memory mutation replay
- **WHEN** an identical memory mutation is retried with its original request identity after acknowledgement
- **THEN** the server SHALL return the original receipt without another note/index mutation or change notification
- **AND** a different request using that identity or an unresolved admission SHALL refuse without automatically executing retained intent
- **AND** mutation completion SHALL require the retained unpublished owner capability and acknowledged shared storage receipt

#### Scenario: HTTP write admission and shutdown ownership
- **WHEN** an authenticated client puts a complete memory document or deletes an existing note
- **THEN** enabled/generate settings, safe matching note identity and validated bounded content SHALL be checked before mutation
- **AND** the owned worker SHALL retain scope and runtime shutdown ownership through durable acknowledgement, including after handler disposal
- **AND** a completed request identity SHALL remain replayable without repeated effects after response-cache loss, and conflicting endpoint or body reuse SHALL refuse
- **AND** pre-admission contention SHALL return a retryable unavailable error without caching that temporary refusal

#### Scenario: Reviewed storage recovery
- **WHEN** a caller explicitly inspects an interrupted memory journal
- **THEN** inspection SHALL leave files unchanged and return the validated proposed note, mutation identity, completion marker and a fingerprint bound to the scope, journal artifacts and current note/index/catalog
- **WHEN** a caller requests recovery using that review fingerprint
- **THEN** the scope SHALL recapture and compare the reviewed state before any normalization or installation
- **AND** stale reviews, unsafe aliases and conflicting evidence SHALL refuse while preserving files and journal fencing
- **AND** recovery acknowledgement SHALL retain the journal until the caller acknowledges completion, without granting authority over unresolved database admissions

#### Scenario: Explicit CLI storage recovery
- **WHEN** the user runs `cyber memory recovery [--global]`
- **THEN** the CLI SHALL inspect recovery without starting model work or creating a memory scope or database, reporting proposed content and the review fingerprint
- **WHEN** the user runs `cyber memory recover --review <fingerprint> [--global]`
- **THEN** the CLI SHALL retain scope ownership, enforce fresh mutation settings and require an unchanged reviewed state
- **AND** a pending database admission for that scope, unavailable database inspection or an in-memory database SHALL refuse before file effects
- **AND** local file recovery SHALL NOT clear or adopt database admission ownership

#### Scenario: Durable journal identity before memory effects
- **WHEN** an admitted model-tool or HTTP memory write prepares its retained journal
- **THEN** the live admission owner SHALL durably bind its transaction receipt and complete intent digest before any note/index installation
- **AND** completion SHALL require that exact bound receipt and the retained owner capability
- **AND** interrupted binding SHALL retain pending admission and journal evidence without reconstructing authority or automatically replaying retained intent
- **AND** historical acknowledged records SHALL remain readable and replayable without manufacturing a journal binding

#### Scenario: Fresh reviewed database and file reconciliation
- **WHEN** an explicit caller reviews a pending admission and matching physical journal under retained scope ownership
- **THEN** recovery SHALL compare both the current storage review and database review before file effects
- **AND** matching bound intent SHALL receive fresh durable recovery ownership without adopting a persisted execution nonce
- **AND** stale database/file reviews, mismatched journals and legacy unbound admissions SHALL refuse without changing files
- **AND** owner disposal SHALL retain unknown fencing and require a fresh review
- **AND** a durably acknowledged journal awaiting archival SHALL finish archival without duplicate acknowledgement or notification
