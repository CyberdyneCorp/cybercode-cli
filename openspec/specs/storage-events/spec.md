# storage-events Specification

## Purpose
Defines where Cyber Code keeps state and how that state is made durable. One SQLite database holds Sessions, inbox rows, projections, workflow runs, goals, schedules, credentials metadata and projects. It is fronted by an append-only, per-aggregate event store whose projectors run in the same transaction. Project identity is derived from git. The design takes OpenCode v2's durable event store and portable paths, OpenCode v1's XDG layout, export/import and `db` command, and adds retention and garbage collection so long-lived installs stay bounded.

## Requirements

### Requirement: XDG directory layout
(P0) The system SHALL use XDG base directories with app name `cyber`:
- data `$XDG_DATA_HOME/cyber` (default `~/.local/share/cyber`)
- config `$XDG_CONFIG_HOME/cyber`
- state `$XDG_STATE_HOME/cyber`
- cache `$XDG_CACHE_HOME/cyber`

Derived directories SHALL be `<data>/log`, `<data>/tool-output`, `<data>/snapshot`, `<data>/memory`, `<data>/worktrees`, `<cache>/bin`, `<cache>/models`, `<cache>/plugins` and `<os tmpdir>/cyber`, and SHALL be created at startup. On macOS and Windows the same XDG defaults under the home directory SHALL be used unless the XDG variables are set. `CYBER_HOME` SHALL relocate all of them under one root.

#### Scenario: CYBER_HOME relocation
- **WHEN** `CYBER_HOME=/opt/cyber-home` is set
- **THEN** data, config, state and cache live under `/opt/cyber-home/{data,config,state,cache}`

### Requirement: Database location and connection
(P0) The database SHALL be `<data>/cyber.db`, or `<data>/cyber-<channel>.db` for non-stable channels, unless `CYBER_DB` is set (`:memory:`, an absolute path, or a name relative to data). On open the system SHALL set `journal_mode=WAL`, `synchronous=FULL`, `busy_timeout=5000`, `cache_size=-64000` and `foreign_keys=ON`, and run a passive WAL checkpoint before migrations.

#### Scenario: Beta channel database
- **WHEN** a `beta` build runs without `CYBER_DB`
- **THEN** it uses `<data>/cyber-beta.db`

### Requirement: Idempotent migrations
(P0) Schema migrations SHALL be identified by timestamped IDs, recorded in a `migration` table (`id`, `time_completed`), applied in ID order, each in its own transaction together with its journal row, and never re-run. An empty database SHALL receive the full current schema in one transaction, with all known migrations marked complete. A database written by a newer version with unknown migration IDs SHALL be opened read-only, with the error `DatabaseTooNewError` and a hint to upgrade.

#### Scenario: Downgrade protection
- **WHEN** a 1.2 binary opens a database that contains migration `20270101000000_add_teams` unknown to it
- **THEN** the server refuses writes and reports `DatabaseTooNewError: upgrade cyber to >= the version that wrote this database`

### Requirement: Durable event store
(P0) The system SHALL append durable events per aggregate (Session, Workflow Run, Goal, Routine, Team) in an IMMEDIATE transaction. Sequence numbers SHALL start at 0 and increase by 1 per aggregate. `event_sequence` SHALL hold the latest `seq`. Each `event` row SHALL store `id`, `aggregate_id`, `seq`, `type` (versioned `<domain>.<name>.<version>`), JSON `data`, `time` and an optional `causation_id`. All registered projectors SHALL run inside the same transaction, so a projection never diverges from its events.

#### Scenario: Projector failure rolls back
- **WHEN** a projector throws while applying `session.prompt.promoted.1`
- **THEN** neither the event row nor any projection change is committed, and the caller receives the error

#### Scenario: Gapless sequence
- **WHEN** two writers append to Session `ses_A` concurrently
- **THEN** the stored events have consecutive `seq` values with no duplicates or gaps

### Requirement: Event replay and subscription
(P0) The store SHALL support reading the events of one aggregate after an exclusive `seq` with a limit (1 to 500, default 100) and a `has_more` flag, and tailing newly committed events without gaps by combining a replay read with a commit notification. Live-only fragments (text and reasoning deltas, progress) SHALL NOT be persisted as durable events.

#### Scenario: Reconnect without gaps
- **WHEN** a client last saw `seq=41` of `ses_A` and reconnects while events 42 to 45 were committed
- **THEN** the subscription first delivers 42 to 45 in order, then live events

### Requirement: Event schema versioning
(P0) Every durable event type SHALL carry an integer version suffix. Readers SHALL upcast older versions to the current shape through registered upcasters. Writing an event type with no registered schema SHALL be a defect that fails the transaction.

#### Scenario: Upcasting an old event
- **WHEN** the store holds `session.prompt.admitted.1` and the current schema is version 2 with a new `delivery` field defaulting to `steer`
- **THEN** readers see version 2 data with `delivery = "steer"`

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

### Requirement: Portable stored paths
(P0) Absolute paths SHALL be stored with forward slashes, and non-absolute values SHALL be rejected for absolute-path columns. Windows drive and UNC paths SHALL be converted back to native form when read on Windows. Exports SHALL store paths relative to the project root.

#### Scenario: Windows path round-trip
- **WHEN** Location `C:\work\repo` is stored and read back on Windows
- **THEN** the stored value is `C:/work/repo` and the returned value is `C:\work\repo`

### Requirement: Project identity from git
(P0) For a Location directory the system SHALL find the enclosing git repository and derive the project ID, in order, from:
1. the SHA-256 of the normalized `origin` URL (scheme, credentials and `.git` suffix stripped, lowercase host)
2. an ID cached in `<git-common-dir>/cyber-project-id`
3. the first root commit hash

The worktree root SHALL be the project directory. Directories outside git SHALL map to project `global`. All worktrees of one repository SHALL share one project ID.

#### Scenario: Same repo, two clones
- **WHEN** `/a/repo` and `/b/repo` are clones of `git@github.com:acme/app.git` and `https://github.com/acme/app`
- **THEN** both resolve to the same project ID

#### Scenario: Outside git
- **WHEN** the Location is `/tmp/scratch` with no repository
- **THEN** the project is `global` with directory `/`

### Requirement: Session export
(P0) `cyber sessions export <id> [--sanitize] [--include-children] [--format json|md]` SHALL write the Session info, messages, parts, goals and workflow run summaries. `--sanitize` SHALL replace prompt and response text, file paths, URLs, tool inputs and outputs, and metadata with `[redacted:<kind>:<n>]` markers while keeping structure, token counts and timings.

#### Scenario: Sanitized export
- **WHEN** the user exports with `--sanitize`
- **THEN** no file path or prompt text from the session appears in the output, and token totals are preserved

### Requirement: Session import
(P0) `cyber sessions import <file|share-url>` SHALL re-home the Session to the current project and directory with fresh IDs, insert messages and parts without overwriting existing rows, and print `Imported session: <id>`. Importing an export produced by a newer schema version SHALL fail with `ExportTooNewError`.

#### Scenario: Import twice
- **WHEN** the same export file is imported twice
- **THEN** two distinct Sessions with different IDs exist

### Requirement: Database command
(P0) `cyber db [query] [--format tsv|json]` SHALL run a read-only SQL query against the database (the connection opened with `query_only=ON`) and print the rows. Without a query, it SHALL launch `sqlite3` on the file in read-only mode. `cyber db path` SHALL print the path, and `cyber db vacuum` SHALL run `VACUUM` while the server is stopped.

#### Scenario: Write attempt rejected
- **WHEN** the user runs `cyber db "DELETE FROM session"`
- **THEN** the command fails with `attempt to write a readonly database` and nothing is deleted

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

### Requirement: Encryption at rest option
(P3) When `storage.encrypt = true`, the database SHALL be encrypted with SQLCipher using a 256-bit key stored in the OS keyring under service `cyber-code`, account `db-key`. Enabling or disabling encryption SHALL migrate the file in place via `cyber db encrypt|decrypt`.

#### Scenario: Missing key
- **WHEN** `storage.encrypt = true` and the keyring entry is missing
- **THEN** the server fails to start with `DatabaseKeyMissingError` and a hint to restore the key or run `cyber db decrypt --from-backup`

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
