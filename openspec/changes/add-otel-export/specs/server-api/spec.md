## MODIFIED Requirements

### Requirement: Route groups
(P0) The server SHALL expose exactly these route groups under `/api/v1`: `health`, `location`, `session`, `message`, `permission`, `question`, `agent`, `model`, `provider`, `credential`, `tool`, `fs`, `pty`, `workflow`, `goal`, `loop`, `job`, `messaging` (cross-session messages), `remote`, `runner`, `routine`, `channel`, `worktree`, `memory`, `lsp`, `formatter`, `usage`, `vcs`, `service` (local service lifecycle), `system` (machine stats), `peer` (peer listener), `peers` (local peer management), `teams` (teams and definitions), `ide`, `policy`, `mcp`, `skill`, `command`, `hook`, `plugin`, `config`, `account`, `oauth`, `telemetry` (OpenTelemetry export status) and `event`. Group names are OpenAPI tags and operation-ID prefixes (e.g. `v1.session.prompt`). `/api/v1/files` belongs to `fs`; `/api/v1/team-definitions` belongs to `teams`; `/api/v1/peer/*` is served only on the peer listener. This list is the single registry of route groups; a capability SHALL NOT introduce a route outside these groups without adding the group here. URL paths SHALL use the plural collection form (`/api/v1/sessions/{id}`, `/api/v1/sessions/{id}/messages`, `/api/v1/permissions/requests`, `/api/v1/workflows/runs`, `/api/v1/channels`, `/api/v1/routines`, `/api/v1/runners`, `/api/v1/agents`, `/api/v1/worktrees`, `/api/v1/credentials`, `/api/v1/oauth-attempts`), except singletons and streams (`/api/v1/health`, `/api/v1/config`, `/api/v1/account`, `/api/v1/policy`, `/api/v1/vcs`, `/api/v1/system`, `/api/v1/usage`, `/api/v1/memory`, `/api/v1/lsp`, `/api/v1/formatters`, `/api/v1/ide`, `/api/v1/event`, `/api/v1/fs`, `/api/v1/messaging`, `/api/v1/telemetry`, `/api/v1/ws`). A group whose capability belongs to a later phase SHALL respond `503` `ServiceUnavailableError` with `service` naming the group until it is implemented.

#### Scenario: Unimplemented group
- **WHEN** a client calls `GET /api/v1/runners` on a build without runner support
- **THEN** the server responds `503` with `{ "_tag": "ServiceUnavailableError", "service": "runner" }`

#### Scenario: Telemetry status
- **WHEN** an owner client calls `GET /api/v1/telemetry` (`v1.telemetry.get`)
- **THEN** the server returns its OpenTelemetry export status (`observability-costs` → OpenTelemetry configuration), with every header value as `***`
