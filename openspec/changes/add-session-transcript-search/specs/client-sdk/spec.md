## ADDED Requirements

### Requirement: Session search method
(P1) The generated SDK SHALL expose `session.search(query, { scope?, archived?, sessionId?, sort?, limit?, cursor? })` for `GET /api/v1/sessions/search`. It SHALL return the typed `{ sessions, index, cursor? }` result, and an async iterator helper SHALL follow `cursor` until the results end.

#### Scenario: Script finds a past Session
- **WHEN** a script calls `await client.session.search("ECONNRESET", { scope: "all" })`
- **THEN** it receives the matching Sessions with their hits, and `index.state` tells whether the backfill is still running
