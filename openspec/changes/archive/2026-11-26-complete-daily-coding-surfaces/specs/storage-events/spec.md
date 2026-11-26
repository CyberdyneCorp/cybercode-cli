## ADDED Requirements

### Requirement: Atomic recovery owner settlement
(P1) The Store SHALL support a checked atomic append for exactly two distinct durable owners, each with an exact expected sequence, so a resolution receipt and its native acknowledgement can commit together. Owner IDs SHALL contain 1–256 bytes; the pair SHALL contain at most 64 events and 16 MiB of encoded events. The decision, event validation, sequences and projectors SHALL share one IMMEDIATE transaction. Rejected decisions or commits SHALL roll back both owners and decision writes; notifications SHALL follow only observed commit success. An unknown completed COMMIT SHALL fence writes and require durable reconciliation rather than repeat filesystem effects.

#### Scenario: Late second-owner conflict
- **WHEN** the second owner has changed after the first owner's events and projections have been applied inside the transaction
- **THEN** the entire transaction rolls back, including decision writes and both owner sequences, and neither owner emits a commit notification

#### Scenario: Invalid second-owner event or projection
- **WHEN** validation or a projector rejects the second owner's settlement
- **THEN** no event or projection from either owner commits and the writer remains available for a corrected request

#### Scenario: Rejected database commit
- **WHEN** a deferred constraint rejects COMMIT after both owner batches have been applied
- **THEN** both batches and decision writes roll back, no commit notification is emitted and a corrected request can succeed

#### Scenario: Committed settlement loses its reply
- **WHEN** SQLite persists both owners but reports a COMMIT error
- **THEN** the Store emits no success notification and fences new writes
- **AND** reopening preserves both original records for reconciliation without duplicating either owner

#### Scenario: Bounded decision refuses
- **WHEN** a checked pair has duplicate, empty or oversized owner IDs, an invalid expected sequence, too many events or an oversized encoding
- **THEN** it returns an explicit nonfatal refusal and commits no decision or event changes

#### Scenario: Older binary cannot ignore new snapshot fences
- **WHEN** a database is upgraded for snapshot-worker ownership
- **THEN** migration `20261106000000_snapshot_worker_ownership` persists without rewriting legacy evidence or adding tables
- **AND** a binary whose migration catalog predates that marker opens the database read-only through the existing unknown-migration gate instead of writing while ignoring new native admission fences

#### Scenario: Durable private snapshot reclamation plan
- **GIVEN** an ordinary file-copy restore has an exact committed settlement receipt and matching native acknowledgement
- **WHEN** eligible displaced payloads are reclaimed after retention
- **THEN** a bounded receipt-owned `snapshot.reclamation.planned.1` event SHALL commit before any private unlink
- **AND** the consuming writer SHALL revalidate the receipt and complete reference/uncertainty view while reference writes remain serialized
- **AND** `snapshot.reclamation.completed.1` SHALL retain the same plan identity and cumulative reclaimed bytes after durable filesystem completion
- **AND** an interrupted plan SHALL remain retryable without acknowledging independent native owners or replaying source effects

#### Scenario: Transactional snapshot reference index
- **GIVEN** retained events contain canonical `copy:<64 lowercase hex>` manifest identities
- **WHEN** reference-index storage is introduced or event data is inserted, updated or deleted
- **THEN** existing references SHALL be backfilled and each later change SHALL update its event-owned reference rows in the same transaction
- **AND** shared references SHALL remain protected until every referencing event is removed
- **AND** missing, unreadable or oversized reference views SHALL refuse reclamation

#### Scenario: Reclaim an acknowledged recovery pair
- **GIVEN** exact paired keep/rollback settlement and matching original/resolution native acknowledgement
- **WHEN** private recovery payloads become eligible after retention
- **THEN** a version-2 reclamation plan SHALL bind both journals and their exact immutable private leaves
- **AND** both exited lifetimes and complete inventories SHALL be checked before any unlink
- **AND** interrupted deletion SHALL resume only from that durable plan without reading or replaying source
- **AND** original version-1 restore plan serialization and IDs SHALL remain unchanged

#### Scenario: Cancelled prepared target cleanup
- **GIVEN** a positive pre-effect cancellation binds fully prepared native target identities and an immutable intent digest
- **WHEN** its cancellation is durably committed and eligible private payloads are reclaimed
- **THEN** the native cancellation event SHALL retain the reviewed intent digest and a version-3 reclamation plan SHALL match it
- **AND** missing historical digest evidence or changed target inodes SHALL refuse cleanup without creating new acknowledgement authority
- **AND** cancellation, intent and request lookup metadata SHALL remain after payload deletion without source admission or source replay
