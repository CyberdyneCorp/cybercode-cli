## ADDED Requirements

### Requirement: Local writer ownership and backpressure
(P0) The registered server SHALL own application writes through one bounded writer queue (default 1024 transactions), with separate read connections. Embedded mode SHALL acquire the same server ownership lock before writing. CLI mutations SHALL route through that owner; stopped-server maintenance SHALL acquire the lock. Transactions SHALL contain no provider calls, tool execution, user waits or network I/O. Queue admission SHALL time out after 5 seconds with retryable `StorageBusyError` (HTTP 503), without acknowledging an uncommitted prompt. Readers SHALL release transactions between replay pages. Queue depth, commit latency, busy errors and WAL size SHALL be observable.

#### Scenario: Writer saturation
- **WHEN** the writer queue is full for 5 seconds
- **THEN** admission returns StorageBusyError, no receipt is acknowledged, and the client can retry with the same message ID

### Requirement: Durability boundary and storage failure
(P0) The server SHALL acknowledge admission and dispatch tool execution only after the associated transaction commits. File-backed SQLite SHALL use WAL and FULL synchronous on every writer connection, assuming a filesystem and device that honor synchronization. This guarantee SHALL cover acknowledged database facts, not unrecorded stream deltas or external side effects. In-memory mode SHALL report `durability: ephemeral`. Disk-full, I/O and corruption errors SHALL stop new mutations and tool dispatch with `StorageUnavailableError`; the system SHALL NOT silently replace the database or fall back to memory. Known network filesystems SHALL be rejected for the live database; unsupported or undetectable filesystem guarantees SHALL be documented by doctor.

#### Scenario: Failed admission commit
- **WHEN** the disk fills before a prompt transaction commits
- **THEN** the prompt receives no success receipt and no provider or tool work starts for that admission

### Requirement: Database and artifact recovery
(P0) Database backup SHALL declare that it excludes snapshot objects, attachments and overflow files. `cyber db backup <file> --include-artifacts` SHALL create a manifest bundle of a consistent database backup and all referenced managed artifacts, pinning the backup artifact set against GC before references can be deleted, until copying completes, and verifying their hashes. Managed artifacts SHALL be immutable and durably written before database references to them commit. Restore SHALL verify the manifest, database integrity and schema compatibility in a temporary location before replacing existing state under the ownership lock. Failure SHALL preserve existing state. GC SHALL preserve artifacts referenced by active Sessions, active workflows, recovery records or backups in progress.

#### Scenario: Missing backup artifact
- **WHEN** a full restore bundle is missing an attachment listed in its manifest
- **THEN** restore fails before replacing the current database or artifact directories

### Requirement: Hosted storage boundary
(P3) Local Session runtimes SHALL continue to use SQLite. Hosted control-plane services with multiple active instances SHALL use PostgreSQL for ownership leases, scheduling, quotas and service metadata; a single-instance self-hosted service MAY use SQLite. Backends SHALL implement atomic append with expected sequence, projection updates, idempotent admission and ownership checks as transactional repository operations. PostgreSQL SHALL NOT be required for local use. Neither backend SHALL share live SQLite files or replicate them as a mechanism for Session handoff; handoff SHALL use the versioned transfer protocol.

#### Scenario: Two hosted schedulers claim work
- **WHEN** two service instances attempt to claim the same Routine firing
- **THEN** one transaction obtains ownership and only that owner dispatches the run

## MODIFIED Requirements

### Requirement: Database location and connection
(P0) The database SHALL be `<data>/cyber.db`, or `<data>/cyber-<channel>.db` for non-stable channels, unless `CYBER_DB` is set (`:memory:`, an absolute path, or a name relative to data). On open the system SHALL set `journal_mode=WAL`, `synchronous=FULL`, `busy_timeout=5000`, `cache_size=-64000` and `foreign_keys=ON`, and run a passive WAL checkpoint before migrations.

#### Scenario: Beta channel database
- **WHEN** a `beta` build runs without `CYBER_DB`
- **THEN** it uses `<data>/cyber-beta.db`

### Requirement: Core tables
(P0) The schema SHALL include at least:
- `project`, `project_directory`, `workspace`
- `session`, `session_input` (inbox: `delivery`, `admitted_seq`, `promoted_seq`, `held_reason`), `session_message`, `session_part`, `session_context_epoch`
- `event`, `event_sequence`
- `tool_call`, `tool_recovery`, `session_task_state`, `budget_reservation`, `permission_saved`, `credential_ref` (metadata only; secrets live in the keyring)
- `workflow_run`, `workflow_agent`, `goal`, `loop`, `schedule`, `routine`
- `team`, `team_task`, `xsession_message`, `device`, `runner`, `share`, `background_job`
- `memory_entry`, `migration`, `data_migration`

Child rows SHALL be deleted by cascade when their parent Session or Project is explicitly deleted. Conversation rewind SHALL NOT delete unresolved tool recovery records; these records SHALL reference durable operation IDs independently of rewindable message rows.

#### Scenario: Cascade on session delete
- **WHEN** Session `ses_A` is deleted
- **THEN** its messages, parts, inbox rows, tool calls, goals and loops are deleted in the same transaction

### Requirement: Retention and garbage collection
(P1) A background GC SHALL run 2 minutes after server start and then every 6 hours. It SHALL delete:
- unpinned tool-output files older than 7 days
- archived Sessions older than `storage.retention.archived_days` (default 90)
- finished background jobs older than 7 days
- finished workflow run agent transcripts older than 30 days, keeping run summaries

It SHALL also run `git gc --prune=7.days` on snapshot repositories. Setting a retention value to `0` SHALL disable that rule.

#### Scenario: Tool output cleanup
- **WHEN** a file in `<data>/tool-output` is 8 days old during a GC pass
- **THEN** it is deleted

### Requirement: Concurrent process safety
(P0) Multiple `cyber` processes SHALL safely read through WAL and `busy_timeout`; application writes SHALL obey Local writer ownership and backpressure. Only the process holding the server registration lock (`<state>/server.lock`, an advisory exclusive lock) SHALL run Drains. Other processes SHALL route execution requests to the registered server.

#### Scenario: Second server refused
- **WHEN** a server is running and the user starts `cyber serve --register` in another terminal
- **THEN** the second process fails with `another cyber server is registered at <url>` and exits 1

### Requirement: Backup
(P0) `cyber db backup <file>` SHALL produce a consistent copy using the SQLite online backup API while the server runs, and `cyber db restore <file>` SHALL refuse to run while a server is registered.

#### Scenario: Online backup
- **WHEN** a Drain is writing events and the user runs `cyber db backup /tmp/b.db`
- **THEN** `/tmp/b.db` opens cleanly and passes `PRAGMA integrity_check`
