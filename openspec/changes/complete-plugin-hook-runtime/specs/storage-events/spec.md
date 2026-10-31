## MODIFIED Requirements

### Requirement: Durability boundary and storage failure
(P0) The server SHALL acknowledge admission and dispatch tool execution only after the associated transaction commits. File-backed SQLite SHALL use WAL and FULL synchronous on every writer connection, assuming a filesystem and device that honor synchronization. This guarantee SHALL cover acknowledged database facts, not unrecorded stream deltas or external side effects. In-memory mode SHALL report `durability: ephemeral`. Disk-full, I/O and corruption errors SHALL stop new mutations and tool dispatch with `StorageUnavailableError`; the system SHALL NOT silently replace the database or fall back to memory. When COMMIT reports an error after the transaction has ended, the writer SHALL preserve the uncertain outcome, return `StorageUnavailableError` and stop later mutations regardless of the SQLite error code. It SHALL NOT manufacture a success acknowledgement or blindly retry that transaction. A failed COMMIT that leaves the transaction active SHALL retain rollback and existing error classification. Known network filesystems SHALL be rejected for the live database; unsupported or undetectable filesystem guarantees SHALL be documented by doctor.

#### Scenario: Failed admission commit
- **WHEN** the disk fills before a prompt transaction commits
- **THEN** the prompt receives no success receipt and no provider or tool work starts for that admission

#### Scenario: Commit completed without a successful acknowledgement
- **WHEN** COMMIT returns an error with no active transaction remaining, including a WAL callback error after persistence
- **THEN** the writer reports an uncertain storage outcome and rejects later mutations until recovery rather than treating a generic error code as proof of rollback
- **AND** any persisted facts remain available to replay without manufacturing a successful live observation or replaying an external handler for a receipt

#### Scenario: Rejected commit retains known rollback
- **WHEN** a deferred constraint rejects COMMIT while the transaction remains active
- **THEN** the failed transaction rolls back and retains its existing error classification
- **AND** a nonfatal rejection does not stop an otherwise healthy writer from committing later valid work
