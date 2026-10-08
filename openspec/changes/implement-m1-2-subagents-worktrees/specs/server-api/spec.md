## ADDED Requirements

### Requirement: Explicit Session inbox wake
(P1) The authenticated server SHALL expose POST `/api/v1/sessions/{sessionID}/wake` and generated SDK `session.wake` to dispatch existing promotable inbox input without submitting a duplicate prompt. An idle child SHALL use exclusive execution ownership, verified checkout preparation and a schema-preserving fresh attempt. Held release SHALL atomically convert the existing row and reset its attempt only after preparation succeeds. A completed child without promotable input SHALL retain its result without new inference or checkout recreation.

#### Scenario: Deferred structured child prompt
- **WHEN** a completed structured child has an existing queued prompt admitted with `resume: false` and the client wakes it
- **THEN** the server starts a fresh owned attempt, preserves message identity and admission sequence and returns 204

#### Scenario: Held child input with busy result ownership
- **WHEN** a client releases a held child input while another tool or Job owns its terminal result
- **THEN** release is refused and both the held row and terminal result remain unchanged
