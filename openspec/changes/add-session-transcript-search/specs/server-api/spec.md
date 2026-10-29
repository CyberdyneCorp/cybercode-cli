## ADDED Requirements

### Requirement: Session search route
(P1) The server SHALL provide `GET /api/v1/sessions/search` in the `session` route group (operation ID `v1.session.search`). The route SHALL be Location-scoped and SHALL take these query parameters:
- `q` (required);
- `scope` (`project` default, `directory` or `all`);
- `archived` (`include` default or `exclude`);
- `session_id`;
- `sort` (`relevance` default or `recent`);
- `limit` (1 to 100, default 20);
- `cursor`.

It SHALL search as `session-search` specifies and respond `{ location, data }` with:
- `data.sessions`: entries `{ id, title, directory, parent_id, root_id, archived, updated_at, score, hits }`;
- `data.index`: `{ state: "ready" | "backfilling", progress?, partial, evicted_sessions }`;
- `data.cursor`, when more results exist.

The static `search` segment SHALL take precedence over `/api/v1/sessions/:sessionID`. An invalid query SHALL fail with `400` `InvalidRequestError`, and disabled search with `503` `ServiceUnavailableError` naming `session_search`. The existing title filter `GET /api/v1/sessions?search=` SHALL keep its meaning.

#### Scenario: Search the project
- **WHEN** a client calls `GET /api/v1/sessions/search?q=flaky%20login` with `x-cyber-directory: /repo/app`
- **THEN** the response lists matching Sessions of the `/repo` project, best first, each with at most 3 hits and snippet highlight ranges

#### Scenario: Static segment is not a Session ID
- **WHEN** a client calls `GET /api/v1/sessions/search` without `q`
- **THEN** the server answers `400` `InvalidRequestError` naming `q`, not `404` for a Session named `search`
