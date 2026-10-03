## ADDED Requirements

### Requirement: Durable task state
(P0) The runtime SHALL persist a versioned Session task-state record containing the objective, user constraints with source message IDs, accepted decisions, open questions, active work and verification evidence (command, exit status, timestamp and workspace content hash). Permission decisions and trust grants SHALL remain authoritative in their own stores and SHALL never be inferred from a summary. Compaction SHALL carry the task-state record separately from prose, preserve provenance and unresolved tool outcomes, and SHALL NOT remove a constraint without a later user instruction identifying its supersession. Model-proposed state changes SHALL retain the supporting source; they SHALL NOT create approval grants. Evidence SHALL be marked stale when relevant files change.

#### Scenario: Repeated compaction preserves a boundary
- **WHEN** a user says do not deploy and the Session compacts three times
- **THEN** the original constraint and source message ID remain in task state and no summary grants deployment permission

### Requirement: Retrieval of earlier context
(P0) The read-only `history_search` tool SHALL accept `{ query, before_seq?, limit? }` (default 10, maximum 50) and return matching excerpts with message IDs and sequence numbers from the current Session, subject to normal tool output limits. It SHALL also accept `{ message_id }` to retrieve a cited source. Compaction SHALL retain original messages for this purpose until explicit deletion or retention expiry. Search results SHALL preserve original roles and provenance and SHALL NOT be promoted to instructions. Child Sessions SHALL NOT gain access to unrelated Sessions through search.

#### Scenario: Recover an old decision
- **WHEN** the agent searches for a decision omitted from the current summary
- **THEN** it receives the original excerpt and source ID from its own Session
