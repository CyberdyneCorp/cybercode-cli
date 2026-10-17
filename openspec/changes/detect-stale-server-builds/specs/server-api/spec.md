## MODIFIED Requirements

### Requirement: Background service management
(P0) The system SHALL provide `cyber service start|stop|restart|status`. Every binary SHALL embed a build identity, and SHALL stamp its executable (canonical path, length and modification time) when it starts. `start` SHALL reuse a registered server whose `GET /api/v1/health` answers within 2 seconds with this client's version and build, and, when the registration names this client's executable path, with this client's executable stamp. A registered server of another version, build or stamp is stale. A stale server SHALL be stopped and replaced only when its registration names this client's executable path and the authenticated `GET /api/v1/service` reports its registration `id`, no running Session and no active workflow run; the client SHALL then print `restarted the Cyber server: it was running an older build` to stderr. A stale server that runs work, belongs to another executable path, or cannot report its work SHALL be kept and used, and the client SHALL print `The Cyber server runs an older build; restart it with `cyber service restart` when idle` to stderr. With no healthy registered server, `start` SHALL spawn a detached `cyber serve --register` and poll every 50 ms, up to 100 attempts, for a healthy registration. A registered server SHALL write `~/.local/state/cyber/server.json` (mode 0600) with `id`, `version`, `build`, `binary`, `binary_stamp`, `url`, `socket` and `pid`, and SHALL remove it on shutdown when the file still holds its own `id`. Every client that starts the service (TUI, `exec`, `api`, `mcp serve` and the resource commands) SHALL use this rule.

#### Scenario: Reuse a healthy server
- **WHEN** `cyber service start` runs and `server.json` points to a server reporting the same version and build
- **THEN** no new process is spawned and the existing URL is printed

#### Scenario: Replace a stale server
- **WHEN** `cyber` was rebuilt or upgraded, and the registered server started from the same executable path reports another build or stamp with no running Session or workflow run
- **THEN** the old server is stopped, a new server is spawned from the new binary, `server.json` is rewritten with the new build, and stderr says `restarted the Cyber server: it was running an older build`

#### Scenario: Keep a busy server of an older build
- **WHEN** the registered server of another build has a running Turn
- **THEN** it keeps running, the client uses it, and stderr says it runs an older build and to run `cyber service restart` when idle

#### Scenario: Another install's server
- **WHEN** the registered server of another build was started from another executable path, or predates `GET /api/v1/service`
- **THEN** it is never stopped by `start`; the client uses it and prints the same warning

#### Scenario: Status
- **WHEN** the user runs `cyber service status`
- **THEN** it reports the registration, whether it is healthy for this version, and whether it is `stale` for this build

### Requirement: Health and version
(P0) `GET /api/v1/health` SHALL be unauthenticated and return `{ healthy: true, version, build, apiVersion: "v1", features: [...] }`. `build` SHALL be the server binary's build identity. `features` SHALL list the enabled capability groups so clients can adapt to phased builds. The authenticated `GET /api/v1/service` SHALL return the listening server's `id`, `version`, `build`, `pid`, executable path, the number of Sessions running a Turn and the number of workflow runs queued, running or waiting for input; a transport that owns no listener SHALL answer `404 ServiceUnavailableError`.

#### Scenario: Feature probe
- **WHEN** a mobile client calls `/api/v1/health`
- **THEN** it learns whether `workflow`, `goal` and `remote` are available before showing those screens

#### Scenario: Build probe
- **WHEN** a client of another build calls `/api/v1/health`
- **THEN** the response's `build` differs from the client's, and `GET /api/v1/service` tells the client whether the server is running work
