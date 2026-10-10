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

#### Scenario: Authenticated pinned memory recovery API
- **WHEN** an authenticated caller reads `GET /api/v1/memory/recovery/{scope}` for project or global memory
- **THEN** the response SHALL carry Location and matching current storage/database reviews without automatic recovery, creating a scope or exposing execution nonces
- **WHEN** the caller posts both review fingerprints to the same route
- **THEN** the server SHALL enforce fresh mutation settings and retain scope and shutdown ownership through reconciliation
- **AND** stale, malformed, foreign, unbound or unverifiable reviews SHALL refuse before effects
- **AND** the SDK SHALL expose typed recovery inspection and confirmation methods
- **AND** ordinary response-cache replay SHALL return the acknowledged result without duplicate effects, while unkeyed cache loss SHALL preserve stale-review refusal

#### Scenario: Durable keyed recovery receipt lookup
- **WHEN** recovery uses a retained Idempotency-Key
- **THEN** durable takeover SHALL atomically bind its request digest and key identity to the original memory mutation
- **AND** identical completed retries after response-cache loss SHALL return that mutation's receipt without additional file effects or events
- **AND** acknowledged receipt replay SHALL remain available while the original HTTP response is pending
- **AND** conflicting body, endpoint or Location reuse SHALL refuse even after response-cache loss
- **WHEN** an authenticated caller reads `GET /api/v1/memory/recovery/requests?scope=project|global&key=<retained-key>`
- **THEN** the response SHALL carry Location and report an acknowledged result or unresolved evidence without reconstructing execution ownership
- **AND** absent keys SHALL return null without creating storage

#### Scenario: CLI reconciliation through a running server
- **WHEN** the user runs `cyber memory recovery --server [--global]`
- **THEN** the CLI SHALL use the existing registered server to report the paired storage and admission review without starting a server or model work
- **WHEN** the user explicitly confirms with `cyber memory recover --server --review <storage> --admission-review <database> --key <retained-key>`
- **THEN** the CLI SHALL submit those exact fingerprints and retained key once through the authenticated recovery API and report the original change receipt
- **AND** settings, scope ownership, stale review refusal and shutdown admission SHALL remain enforced by the server
- **WHEN** the user runs `cyber memory recovery-request --server --key <retained-key>`
- **THEN** the CLI SHALL report the Location-bound durable receipt or unresolved evidence without replaying a mutation
- **AND** missing registration, invalid fingerprints and missing confirmation identity SHALL refuse without creating a database, starting a server or acquiring memory storage

#### Scenario: TUI memory review and explicit reconciliation
- **WHEN** the user opens `/memory` or `/memory global`
- **THEN** the TUI SHALL list validated note metadata and safe invalid-file diagnostics through the Location-scoped API and allow reading selected notes
- **AND** deletion SHALL require review of the selected note and a separate explicit confirmation bound to its scope and Location
- **WHEN** the user inspects a retained recovery journal
- **THEN** the TUI SHALL display the proposed content, paired fingerprints and completion state before allowing explicit recovery confirmation
- **AND** confirmed recovery SHALL retain a request key and offer read-only durable receipt lookup after an uncertain response without automatic effect replay
- **AND** stale generations, changed Location/Session, closed panels and scope changes SHALL prevent applying unrelated responses or confirmations
- **AND** the embedded client SHALL preserve explicit request headers and return actual response statuses for single raw requests

#### Scenario: Reviewed memory edit API
- **WHEN** an authenticated user requests `GET /api/v1/memory/edit/{scope}/{name}`
- **THEN** the API SHALL return bounded original Markdown and a fingerprint bound to the existing scope, note, index and catalog without creating memory storage
- **WHEN** PUT supplies `review_fingerprint` or DELETE supplies `X-Cyber-Memory-Review`
- **THEN** mutation SHALL verify the current fingerprint under retained scope ownership before durable admission and again before journal preparation
- **AND** changed note/index content, replaced scope or files and foreign note/scope reviews SHALL refuse without overwriting user edits
- **AND** valid edits SHALL retain ordinary settings, validation, durable acknowledgement, notifications and identical completed request replay
- **AND** the SDK SHALL expose typed edit review and optional conditional PUT


#### Scenario: Reviewed TUI memory draft
- **WHEN** the user opens a note and requests editing in `/memory`
- **THEN** the TUI SHALL fetch the bounded original Markdown and conditional edit fingerprint and provide multiline Unicode typing and paste
- **WHEN** the user reviews a save and separately confirms it
- **THEN** the TUI SHALL send one conditional PUT with a retained request key and accept only a matching Location/scope/note write receipt
- **AND** validation, stale-review, transport or response errors SHALL retain the draft and refuse a new save until fresh review
- **AND** refreshing the review SHALL display the current original while preserving draft changes
- **AND** dismissal or Session/Location/scope changes SHALL retain the process-local draft without foreign save authority; discarding SHALL require explicit confirmation
- **AND** terminal controls SHALL be escaped, input SHALL be bounded, and confirmation controls SHALL remain visible while scrolling


#### Scenario: Private memory client checkpoints
- **WHEN** client draft/request retention opens existing state
- **THEN** absent storage SHALL remain absent and unsafe file aliases or non-private existing storage SHALL refuse without permission repair
- **WHEN** a client owns checkpoint storage
- **THEN** another owner SHALL refuse; checkpoint bytes SHALL be bounded, written through a private exclusive temporary file, synced, atomically installed and directory-synced
- **AND** changed checkpoint bytes or replaced checkpoint/lock/state directory identities SHALL refuse without overwriting user edits
- **WHEN** the owner dies abruptly and another client opens storage
- **THEN** only the installed checkpoint SHALL load; staged files SHALL remain evidence and SHALL NOT be automatically adopted
- **AND** restored bytes SHALL grant no memory mutation or recovery authority; callers SHALL validate versions and scoped identities and obtain fresh reviews before new effects
- **AND** unsupported native privacy/durability SHALL refuse before creating client storage

#### Scenario: Native Windows memory handle evidence
- **WHEN** native memory verifies a Windows file handle
- **THEN** regular-file admission SHALL refuse directories, reparse points and multiple file links before note access
- **AND** object identity SHALL preserve the full volume and 128-bit file identifiers without a truncated fallback or path-based reopening
- **WHEN** native private-object verification reviews an existing handle
- **THEN** it SHALL require the current process user as owner and a protected non-null valid DACL granting only that user or SYSTEM
- **AND** foreign grants, unsupported ACE types, malformed or out-of-bounds token/SID evidence and unprotected or null DACLs SHALL refuse without changing permissions
- **AND** these checks SHALL NOT enable mutation or checkpoints until private creation, identity binding, durable updates and native acceptance are implemented

#### Scenario: Native Windows private child creation
- **WHEN** native memory creates a directory under a retained caller-selected directory handle
- **THEN** it SHALL resolve one bounded safe child component relative to that handle and atomically install a protected current-user/SYSTEM-only security descriptor
- **AND** regular-file creation SHALL additionally require a private parent directory before native creation
- **AND** create-only admission SHALL refuse existing children without opening, truncating or repairing them; returned handles SHALL be verified for private security and expected type before caller content writes
- **AND** traversal, separators, alternate streams, reserved device names, trailing-dot/space aliases, unsupported names and reparse parents SHALL refuse without child effects
- **WHEN** the retained parent is moved and its original path is replaced
- **THEN** native creation SHALL remain under the retained parent, without reopening the replacement path
- **AND** caller path binding, durable installation and lifecycle integration SHALL remain required before enabling Windows mutation/checkpoint workflows


#### Scenario: Native Windows existing private child opening
- **WHEN** native memory opens an existing private child under a retained directory handle
- **THEN** it SHALL use open-only disposition without a creation security descriptor, creation, truncation or permission repair
- **AND** it SHALL validate the returned object's private owner/DACL, expected type, reparse status and regular-file link count before returning it
- **AND** regular-file opening SHALL require a private parent; read access SHALL grant no data writes, and write access SHALL preserve existing bytes until the caller explicitly writes
- **WHEN** the parent moves and its original path is replaced
- **THEN** opening SHALL resolve under the retained parent rather than the replacement path
- **AND** missing children, unsafe names, broad permissions, wrong types and file aliases SHALL refuse without changing child bytes or creating missing children
- **AND** durable installation, caller path binding and storage/checkpoint integration SHALL remain required before enabling Windows writes

#### Scenario: Native Windows retained create-only rename
- **WHEN** native memory retains a private file or directory for namespace installation
- **THEN** it SHALL acquire a synchronous handle with exclusive write/delete sharing, verify the expected full source identity and retain it for caller content review
- **AND** competing source writes, deletion or renaming SHALL refuse while that handle is retained; stale source identity, unsafe permissions, reparse points or regular-file aliases SHALL refuse before namespace effects
- **WHEN** the retained object is renamed under a private destination directory handle
- **THEN** the operation SHALL use one validated component and create-only rename through the retained source and destination handles, without path reopening or replacement flags
- **AND** existing destinations SHALL preserve source and destination objects and bytes; successful installation SHALL verify private security and the same full source identity at the destination
- **AND** source/destination evidence SHALL remain retained on unknown or failed postconditions without automatic rollback, deletion or permission repair
- **AND** caller content review, journal recovery, namespace durability and complete storage/checkpoint integration SHALL remain required before Windows writes are enabled

#### Scenario: Native Windows private memory storage admission
- **WHEN** startup ensures the Windows memory root or MemoryStore explicitly admits a project/global scope
- **THEN** missing directories SHALL be created through retained parent handles with private security installed atomically, and existing directories SHALL be opened without repair and verified as private non-reparse objects
- **AND** existing-review admission SHALL NOT create missing directories or lock files
- **WHEN** a Windows scope is claimed
- **THEN** its lock SHALL be created privately or an existing private regular lock SHALL be opened without truncation/repair, with exclusive cross-process ownership retained through the scope guard
- **WHEN** a Windows memory note or index is read
- **THEN** handle-relative read-only opening SHALL verify private owner/DACL, regular type, no reparse points and a single link before returning any bytes
- **AND** broad or inherited-unprotected directories, locks, notes and indexes SHALL refuse without permission changes or user-byte loss; unsafe catalog diagnostics SHALL NOT expose note contents
- **AND** startup SHALL NOT create a default-public memory root before private admission, while missing project/global scopes remain uncreated
- **AND** Windows mutation/checkpoint workflows SHALL remain gated until durable namespace installation and complete lifecycle integration are implemented and natively verified

#### Scenario: Native Windows private flush and durable create-only rename
- **WHEN** native memory synchronizes a private retained file or directory
- **THEN** it SHALL request normal native flushing of data, metadata and underlying storage cache with no data-only/no-sync flags, and verify the retained private identity before and after the synchronous call
- **AND** read-only handles, unsafe objects, unsupported filesystem/storage flushing and non-success native settlement SHALL refuse without a silent fallback or durability acknowledgement
- **WHEN** a retained private object is installed durably
- **THEN** its source directory handle SHALL remain retained; file/source-directory/destination-directory flushing SHALL be preflighted before create-only rename
- **AND** successful rename SHALL be followed by source-object and both directory flushes before acknowledgement, preserving exact identity and existing-destination refusal
- **AND** preflight refusal SHALL leave the namespace unchanged; a failure after rename SHALL retain source/journal evidence without rollback or inferred durable completion
- **AND** native flush acceptance SHALL NOT by itself enable mutation/checkpoints before journal/client integration, caller path/content binding and complete native lifecycle acceptance

#### Scenario: Native identity-bound memory reviews and checkpoints
- **WHEN** a memory review or client binding captures native object identity
- **THEN** Unix SHALL retain device/inode identity and its existing review serialization shape; Windows SHALL retain the full volume/128-bit file identity without a missing or truncated fallback
- **AND** unsupported native identity SHALL refuse rather than treat two absent identities as a match
- **WHEN** a memory review reads a note/index
- **THEN** its digest and identity SHALL come from the same bounded verified regular-file handle, and the current named object SHALL still match that handle before review return
- **WHEN** a reviewed target/index is replaced with an object containing identical bytes
- **THEN** fresh review identity SHALL differ and the earlier review SHALL refuse without file mutation
- **AND** fresh scope directory identity SHALL participate in the review fingerprint, while missing notes/indexes remain missing
- **AND** client identity checks SHALL compare actual native handles; Windows journal/checkpoint activation SHALL still require complete durability/lifecycle integration and native acceptance

#### Scenario: Durable TUI memory draft and request retention
- **WHEN** the TUI changes a memory draft on a platform with private native checkpoint support
- **THEN** it SHALL checkpoint bounded draft text and its original Session/Location/scope identity through the private client store
- **WHEN** a memory save, deletion or recovery is separately confirmed
- **THEN** its immutable request intent and exact key SHALL be durably retained before dispatch; stale or foreign-context actions SHALL refuse
- **WHEN** the TUI restarts
- **THEN** version and scoped identities SHALL be validated; draft text and request evidence SHALL restore without review fingerprints, pending actions or confirmations
- **AND** a new save SHALL require fresh review; stored recovery keys SHALL grant only scoped read-only receipt lookup and retained requests SHALL NOT be sent automatically
- **WHEN** checkpoint storage is unsafe, malformed, busy, changed or cannot save
- **THEN** fresh memory effects SHALL remain local and orderly exit SHALL refuse while unsaved work remains; original files and draft text SHALL be preserved
- **AND** unchanged failed checkpoints SHALL NOT create new staged writes on each background tick
- **AND** acknowledged saves SHALL durably clear saved drafts, acknowledged save/delete requests SHALL retire their pending intents, and explicit discard SHALL durably clear draft state; request evidence SHALL remain bounded and complete native/outcome acceptance SHALL still be required

#### Scenario: Read-only retained save and delete outcome lookup
- **WHEN** an authenticated client looks up a retained HTTP save/delete key at its original Location and scope
- **THEN** the API SHALL return durable admitted request metadata, its HTTP fingerprint, pinned journal if available and matching completion if acknowledged, independently of the HTTP response cache
- **AND** an unknown key SHALL return no evidence; a foreign Location/scope SHALL refuse without revealing its evidence
- **AND** inconsistent ledger identities, digests or receipts SHALL refuse rather than imply completion
- **AND** lookup SHALL NOT create memory storage, execute a mutation, publish an update or require enabled writable memory settings

#### Scenario: TUI retained request reconciliation
- **WHEN** the user opens retained requests
- **THEN** the TUI SHALL list bounded immutable request evidence and provide explicit read-only lookup in the original Session/Location/scope
- **AND** save/delete status SHALL match the exact retained HTTP fingerprint, name, operation, journal and completion identities before retiring its intent
- **AND** absent, unresolved, malformed or mismatched status SHALL preserve request evidence and draft text without replay
- **WHEN** a completed save is reconciled
- **THEN** its draft SHALL clear only if text and retained key still match; newer edits SHALL remain and require fresh review
- **WHEN** the user explicitly forgets a selected retained request
- **THEN** confirmation SHALL identify the exact key and warn that forgetting does not resolve unknown effects; it SHALL remove only that record through the normal durable checkpoint gate
- **AND** canceled confirmation and late/foreign responses SHALL preserve other records and edits

#### Scenario: Native Windows journal installation and retained recovery
- **WHEN** Windows prepares a memory journal
- **THEN** journal directories and files SHALL be created privately through retained parent handles, without adopting an existing transaction or repairing unsafe objects
- **WHEN** an original note/index is captured or a prepared replacement is installed
- **THEN** the exact source SHALL be retained and its bounded content hash and full identity SHALL match before a durable create-only rename
- **AND** existing destinations SHALL refuse without replacement, cleanup or rollback
- **WHEN** Windows resumes a partial installation
- **THEN** installed note/index identities SHALL be preserved, original versions SHALL remain journal evidence, and hard-link aliases SHALL refuse without Unix link normalization
- **AND** a failed acknowledgement SHALL leave completed evidence for recovery without replaying installed file effects
- **AND** complete journal/client path binding and native lifecycle acceptance SHALL remain required before public Windows mutation admission

#### Scenario: Retained scope lock and live journal bindings
- **WHEN** an admitted memory store or scope is used
- **THEN** its current data path, named memory root and named scope SHALL match retained native directory identities
- **AND** its named private regular lock SHALL still match the held lock before read/preparation/recovery effects
- **WHEN** a prepared journal commits
- **THEN** its named journal directory and intent object/bytes SHALL match retained evidence before file effects and acknowledgement
- **AND** moved/replaced scopes, roots, locks, journals or intent objects SHALL refuse without repairing or deleting replacement evidence
- **AND** Windows bootstrap directory handles SHALL permit delete sharing and reject final reparse objects, while public mutation admission remains gated pending complete lifecycle acceptance

#### Scenario: Windows scope namespace pins
- **WHEN** Windows claims a memory scope
- **THEN** data/root/scope directories and its lock SHALL have retained handles denying competing delete/rename sharing throughout the claim
- **AND** ordinary writable directory/lock capabilities SHALL exclude delete access so independent reads/claims and native flushes can coexist with those pins
- **WHEN** a live journal is prepared or reopened
- **THEN** its directory SHALL deny delete sharing until exact-source history archival releases the writable journal handle and acquires the identity-verified exclusive source
- **AND** failed competing renames/deletions SHALL leave objects/bytes unchanged; disposal SHALL release pins and allow namespace changes
- **AND** pinning SHALL NOT itself activate public Windows mutations before persisted identity, intent/crash/client checkpoint integration and native acceptance

#### Scenario: Immutable live Windows journal intent
- **WHEN** Windows admits a prepared or recovered journal
- **THEN** it SHALL retain the exact intent object under an exclusive source guard, and revalidate canonical intent bytes under that guard before returning the prepared capability
- **AND** competing content-write/delete/rename access SHALL refuse while the capability owns the guard
- **WHEN** the capability is disposed or archived
- **THEN** the intent guard SHALL release without deleting evidence or replaying effects, and SHALL close before exclusive journal-directory archival
- **AND** a fresh recovery SHALL admit and validate current evidence again rather than inherit disposed authority
- **AND** Unix replacement/content conflict checks and Windows persisted target identity/crash/client checkpoint gates SHALL remain intact

#### Scenario: Persisted native identity through Windows recovery
- **WHEN** Windows seals a memory journal
- **THEN** versioned intent SHALL persist full native identities for its data/root/scope/journal/intent objects, original/staged note/index files and admitted catalog files
- **AND** content and identity SHALL be captured on the same bounded verified handle and original identities SHALL be captured before final review verification
- **WHEN** recovery resumes after disposal or process death
- **THEN** it SHALL require recorded directory/intent identities and exact original/captured/installed slot identities, refusing replacements even with identical bytes before effects or acknowledgement
- **AND** source retention for capture/installation SHALL use the recorded identity rather than a newly observed replacement identity
- **AND** incomplete/legacy Windows intent SHALL refuse without repair; Unix version-1 serialization and recovery SHALL remain supported
- **AND** complete crash acceptance, terminal ownership and client checkpoint integration SHALL remain required before public Windows mutation activation

#### Scenario: Windows terminal ownership through acknowledgement
- **WHEN** Windows verifies installed memory before publishing its receipt
- **THEN** it SHALL retain readonly exact-ID guards denying competing content-write/delete/rename access to installed note/index, catalog and captured-original objects through acknowledgement
- **AND** completed evidence SHALL be guarded and checked without granting it mutation authority
- **AND** a pre-existing writer SHALL refuse acknowledgement while retaining journal evidence
- **WHEN** acknowledgement fails or succeeds
- **THEN** failure SHALL release guards without rollback and keep recovery evidence; success SHALL release journal descendants only for exact-source archival while installed/catalog guards stay owned until commit returns
- **AND** ordinary readonly inspection SHALL remain possible; full crash/checkpoint/canonical acceptance SHALL remain required before public Windows activation

#### Scenario: Windows owner death during terminal acknowledgement
- **WHEN** the owning Windows process dies inside the acknowledgement callback before or after publishing its receipt witness
- **THEN** completed journal evidence SHALL survive, process-held exclusions SHALL release, and reopened recovery SHALL retain the exact installed note/index and captured-original identities
- **AND** recovered acknowledgement SHALL use the original journal/receipt identity and archive that exact journal once without replaying installed objects
- **AND** storage callback witnesses SHALL NOT be treated as proof of database reconciliation or complete crash acceptance

#### Scenario: Windows private checkpoint ownership and reads
- **WHEN** client checkpoint storage is privately admitted on Windows
- **THEN** its state and client directory names and held lock SHALL be pinned to native identities for the owner lifetime, with an independent OS ownership lock
- **AND** creation SHALL atomically apply protected current-user/SYSTEM privacy; unsafe existing objects SHALL refuse without repair
- **AND** existing-only admission SHALL not create missing directories or checkpoints; bounded reads SHALL verify bytes and full object identity on retained native handles
- **AND** native durability SHALL use normal verified file/directory flushes; public checkpoint admission SHALL remain gated until exact-source durable replacement and complete crash acceptance
