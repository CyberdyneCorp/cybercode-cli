## MODIFIED Requirements

### Requirement: Route groups
(P0) The server SHALL expose exactly these route groups under `/api/v1`: `health`, `location`, `session`, `message`, `permission`, `question`, `agent`, `model`, `provider`, `credential`, `tool`, `fs`, `pty`, `workflow`, `goal`, `loop`, `job`, `messaging` (cross-session messages), `remote`, `runner`, `routine`, `channel`, `worktree`, `memory`, `lsp`, `formatter`, `usage`, `vcs`, `autofix` (PR auto-fix subscriptions), `service` (local service lifecycle), `system` (machine stats), `peer` (peer listener), `peers` (local peer management), `teams` (teams and definitions), `ide`, `policy`, `mcp`, `skill`, `command`, `hook`, `plugin`, `config`, `account`, `oauth` and `event`. Group names are OpenAPI tags and operation-ID prefixes (e.g. `v1.session.prompt`). `/api/v1/files` belongs to `fs`; `/api/v1/autofix/subscriptions` belongs to `autofix`; `/api/v1/team-definitions` belongs to `teams`; `/api/v1/peer/*` is served only on the peer listener. This list is the single registry of route groups; a capability SHALL NOT introduce a route outside these groups without adding the group here. URL paths SHALL use the plural collection form (`/api/v1/sessions/{id}`, `/api/v1/sessions/{id}/messages`, `/api/v1/permissions/requests`, `/api/v1/workflows/runs`, `/api/v1/channels`, `/api/v1/routines`, `/api/v1/runners`, `/api/v1/agents`, `/api/v1/worktrees`, `/api/v1/credentials`, `/api/v1/oauth-attempts`), except singletons and streams (`/api/v1/health`, `/api/v1/config`, `/api/v1/account`, `/api/v1/policy`, `/api/v1/vcs`, `/api/v1/system`, `/api/v1/usage`, `/api/v1/memory`, `/api/v1/lsp`, `/api/v1/formatters`, `/api/v1/ide`, `/api/v1/event`, `/api/v1/fs`, `/api/v1/messaging`, `/api/v1/ws`). A group whose capability belongs to a later phase SHALL respond `503` `ServiceUnavailableError` with `service` naming the group until it is implemented.

#### Scenario: Unimplemented group
- **WHEN** a client calls `GET /api/v1/runners` on a build without runner support
- **THEN** the server responds `503` with `{ "_tag": "ServiceUnavailableError", "service": "runner" }`

## ADDED Requirements

### Requirement: Self-hosted auto-fix owner API
(P3) The owner API SHALL expose Location-scoped `GET` and `POST /api/v1/autofix/subscriptions`, `GET /api/v1/autofix/subscriptions/{id}` and `POST /api/v1/autofix/subscriptions/{id}/control`. Enable SHALL require an idempotency key and bind the configured linked Session to the current Location. Control SHALL require the exact current `expected_revision` and an action of `pause`, `resume` or `disable`. Read and mutation routes SHALL use owner authentication and the existing Origin policy; peer Session grants SHALL NOT authorize subscription controls. OpenAPI and generated SDK SHALL expose the same typed contract.

#### Scenario: Stale control
- **WHEN** a client posts a control with a revision older than the current subscription
- **THEN** the API responds with a conflict and leaves subscription state unchanged

#### Scenario: Foreign Location
- **WHEN** an owner requests a subscription using a different Location
- **THEN** the API refuses the request without modifying the subscription

#### Scenario: Missing enable key
- **WHEN** a client enables auto-fix without an idempotency key
- **THEN** the API refuses before recording a subscription or dispatching inference
