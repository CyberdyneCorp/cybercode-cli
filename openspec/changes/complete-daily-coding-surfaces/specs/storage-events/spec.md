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
