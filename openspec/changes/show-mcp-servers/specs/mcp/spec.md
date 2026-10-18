## MODIFIED Requirements

### Requirement: Status model
(P0) The system SHALL report each server's status as `idle`, `connecting`, `connected`, `disabled`, `failed` (with error), `needs_auth`, or `needs_client_registration`, together with its `transport` (`remote` or `stdio`). `idle` SHALL mean the server is configured and allowed but no Session has opened its Location yet; such a server SHALL NOT be reported as `failed`, and `idle` SHALL never be recorded as a connection transition. It SHALL publish `mcp.status.changed.1` on every transition, and on connection loss SHALL set `failed`, remove the server's tools and retry with exponential backoff (1 s to 60 s, at most 10 attempts).

#### Scenario: Reconnect after drop
- **WHEN** a connected remote server drops the connection
- **THEN** its tools are removed, status becomes `failed`, and reconnection is retried with backoff

#### Scenario: Configured but not started
- **WHEN** a server is configured globally and no Session has opened the Location yet
- **THEN** `GET /api/v1/mcp` reports it `idle` with `transport` `remote`, and it connects once a Session opens the Location

### Requirement: MCP CLI and UI
(P0) The system SHALL provide `cyber mcp add <name> [--scope user|project|local] (--url <url> [--header K=V]... | [--env K=V]... -- <command> [args...])`, `list`, `get <name>`, `remove <name> [--scope user|project|local]`, `auth <name>`, `logout <name>`, `debug <name>` and the in-session `/mcp` panel (status, tools, reconnect, auth). `user` scope SHALL write the global document, `project` the project's `.cyber/cyber.jsonc` (which then needs `cyber mcp trust`), and `local` the project's `.cyber/cyber.local.jsonc`. Edits SHALL preserve JSONC comments. `add` SHALL refuse a name already defined in the target document, and `remove` without `--scope` SHALL refuse a name defined in more than one document.

#### Scenario: Add remote server
- **WHEN** the user runs `cyber mcp add linear --url https://mcp.linear.app/mcp`
- **THEN** a remote entry is written to user-scope config and the server connects

#### Scenario: Comments survive an edit
- **WHEN** the global `cyber.jsonc` has comments and the user adds and then removes a server
- **THEN** every comment and every other setting is unchanged
