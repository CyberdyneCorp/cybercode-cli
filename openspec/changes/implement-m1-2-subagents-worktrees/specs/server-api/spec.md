## ADDED Requirements

### Requirement: Explicit Session inbox wake
(P1) The authenticated server SHALL expose POST `/api/v1/sessions/{sessionID}/wake` and generated SDK `session.wake` to dispatch existing promotable inbox input without submitting a duplicate prompt. An idle child SHALL use exclusive execution ownership, verified checkout preparation and a schema-preserving fresh attempt. Held release SHALL atomically convert the existing row and reset its attempt only after preparation succeeds. A completed child without promotable input SHALL retain its result without new inference or checkout recreation.

#### Scenario: Deferred structured child prompt
- **WHEN** a completed structured child has an existing queued prompt admitted with `resume: false` and the client wakes it
- **THEN** the server starts a fresh owned attempt, preserves message identity and admission sequence and returns 204

#### Scenario: Held child input with busy result ownership
- **WHEN** a client releases a held child input while another tool or Job owns its terminal result
- **THEN** release is refused and both the held row and terminal result remain unchanged

### Requirement: Explicit subtree stop API
(P1) The authenticated server SHALL expose POST `/api/v1/sessions/{sessionID}/stop-subtree` and generated SDK `session.stopSubtree`. It SHALL return the runtime-owned report with original Session and scope identity, acknowledgement status, problems and receipt persistence. Unknown outcomes SHALL remain explicit successful report responses, never an empty success implying cancellation. The operation SHALL leave admission closed even after local acknowledgement, preserve inbox rows and restrict cancellation to the requested subtree. Ordinary Session interruption SHALL retain its background-child independence. Reopening and reviewed recovery remain separate required contracts.

#### Scenario: Scoped explicit stop
- **WHEN** a client explicitly stops a child subtree while an unrelated background Job runs
- **THEN** scoped Jobs settle, unrelated work continues and queued input remains durable
- **AND** the report retains the requested Session identity and admission remains closed

#### Scenario: Missing acknowledgement
- **WHEN** held result ownership prevents acknowledgement
- **THEN** the API and SDK return an Unknown report with problems and persistence evidence
- **AND** a new stop request may reassess the same closure after ownership settles without reopening admission

#### Scenario: Authentication and identity
- **WHEN** an unauthenticated caller requests subtree stop
- **THEN** authentication fails before closure or cancellation
- **AND** an authenticated request for a missing Session returns the tagged Session-not-found error
