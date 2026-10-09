## MODIFIED Requirements

### Requirement: Idempotency keys
(P0) Every non-GET route SHALL accept an `Idempotency-Key` header of 1–128 printable characters. The server SHALL retain the key, request hash and response for 24 hours per authenticated principal. A retry with the same key and body SHALL return the original response with header `Idempotent-Replayed: true`, and a retry with the same key but a different body SHALL fail with `409` `ConflictError`.

#### Scenario: Network retry
- **WHEN** a client resends `POST /api/v1/sessions` with the same `Idempotency-Key` and body after a timeout
- **THEN** the same Session is returned and no second Session is created

#### Scenario: Location-scoped request identity
- **WHEN** an Idempotency-Key is reused with a different effective Location or request query
- **THEN** the server SHALL return ConflictError instead of replaying another request's response
- **AND** same-request replay SHALL NOT close a newly reopened MCP connection
