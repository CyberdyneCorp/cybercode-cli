## ADDED Requirements

### Requirement: Session branch routes
(P1) The server SHALL provide:

- `GET /api/v1/sessions/:sessionID/tree`, returning user-message nodes `{ id, parent_id, preview, created_at, labels, active, tip, descendants }`.
- `POST .../checkout` with `{ message_id?, label?, exact?: boolean, code?: boolean }`, returning the new active tip and, when `code` is true, the applied revert state.
- `PUT .../labels/:name` with `{ message_id }`, and `DELETE .../labels/:name`.

`GET .../messages` SHALL return only the active path unless `branches=all` is given, in which case every message SHALL include `parent_message_id`. Checkout on a busy Session SHALL fail with `409` and `SessionBusyError`. An unknown message or label SHALL fail with `404`.

#### Scenario: Linear view for existing clients
- **WHEN** a client that does not know about branches calls `GET .../messages` on a Session with two branches
- **THEN** it receives the active path in order, exactly as a linear conversation

#### Scenario: Checkout by label
- **WHEN** a client posts `{ "label": "fix-a" }` to `.../checkout` on an idle Session
- **THEN** the path ending at the labeled message's newest tip becomes active and the response names that tip
