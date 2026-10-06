## MODIFIED Requirements

### Requirement: Listener defaults
(P0) The server SHALL bind `127.0.0.1` by default. It SHALL try port 4747 first and fall back to an OS-assigned free port. On macOS and Linux it SHALL also listen on the Unix domain socket `~/.local/state/cyber/cyber.sock` (mode 0600). `--hostname`, `--port` and `--socket` SHALL override these defaults, and `--no-tcp` SHALL disable the TCP listener.

#### Scenario: Port 4747 busy
- **WHEN** port 4747 is already in use and no `--port` is given
- **THEN** the server binds a random free port and records it in `server.json`

#### Scenario: Socket-only mode
- **WHEN** the server starts with `--no-tcp` on macOS or Linux
- **THEN** it accepts requests only on the Unix socket and `server.json` has no `url`

#### Scenario: Windows registration advertises its actual listener
- **WHEN** a Windows server starts with default listener options
- **THEN** it SHALL serve TCP, record `socket: null` and remove only its own registration on shutdown
- **AND** a pre-existing file at the Unix socket's default path SHALL remain untouched

#### Scenario: A Unix-only option is unavailable on Windows
- **WHEN** a Windows user requests `--socket` or `--no-tcp`
- **THEN** startup SHALL fail clearly before listeners, registration or readiness notification, and SHALL NOT delete a supplied socket path

### Requirement: Background service management
(P0) The system SHALL provide `cyber service start|stop|restart|status`. `start` SHALL reuse a registered server whose `GET /api/v1/health` answers within 2 seconds with a matching version, and otherwise SHALL spawn a detached `cyber serve --register` and poll promptly within a five-second deadline for a healthy registration. A registered server SHALL write its private `server.json` with `id`, `version`, `url`, `socket` and `pid`, and SHALL remove it on shutdown only when the file still holds its own `id`. `stop` SHALL request authenticated listener shutdown for that registration ID instead of signaling an unchecked recorded PID. TCP requests SHALL require the local password; Unix socket requests SHALL require same-user peer credentials. Mismatched identity, invalid authentication or an unverified shutdown response SHALL retain the registration and fail clearly.

#### Scenario: Reuse a healthy server
- **WHEN** `cyber service start` runs and `server.json` points to a server reporting the same version
- **THEN** no new process is spawned and the existing URL is printed

#### Scenario: Replace a stale server
- **WHEN** the registered server reports a different version or does not answer within 2 seconds
- **THEN** replacement SHALL verify shutdown of the registered server or prove the local server lock is unowned before spawning a replacement and rewriting registration
- **AND** an unverified endpoint or stale PID SHALL NOT authorize signaling a process

#### Scenario: Status
- **WHEN** the user runs `cyber service status`
- **THEN** it prints `running <url> (v<version>)` for a healthy matching server, otherwise `stopped`, and removes a stale `server.json`

#### Scenario: Stop a Windows service without a Unix signal command
- **WHEN** `cyber service stop` finds a registered Windows TCP server
- **THEN** it SHALL submit an authenticated stop request carrying that server's registration ID and wait for unregistration
- **AND** it SHALL NOT invoke a Unix `kill` command or target the recorded PID

#### Scenario: Stale process ID with a live registered server
- **WHEN** the registered endpoint and ID identify a live server but the recorded PID is stale
- **THEN** stopping SHALL follow the endpoint and registration ID, without signaling the stale PID

#### Scenario: Stop request does not match the current server
- **WHEN** a stop request has invalid credentials or an outdated registration ID
- **THEN** the server SHALL reject it without notifying listener shutdown or removing registration

#### Scenario: Stop a socket-only service
- **WHEN** a macOS or Linux registered server has no TCP URL
- **THEN** stopping SHALL use its registered Unix socket with same-user peer authentication

#### Scenario: Password replacement uses the current credentials
- **WHEN** a user replaces the local service password while a server is registered
- **THEN** shutdown SHALL authenticate with the existing password before the replacement is written
- **AND** refused shutdown SHALL preserve the existing credentials and registration
- **AND** successful replacement SHALL restart the service with the new password

#### Scenario: Shutdown settles active runtime work
- **WHEN** the listener owner begins service shutdown
- **THEN** runtime admission SHALL close and all active session drains SHALL receive cancellation
- **AND** shutdown SHALL await tool settlement and background title-task completion
- **AND** side-effecting interrupted tools SHALL retain outcome-unknown settlement
- **AND** pending durable inbox rows SHALL remain available to a new runtime after restart
- **AND** new sessions, forks, admissions, releases, compaction and resume requests SHALL fail clearly after admission closes

#### Scenario: Attached streams cannot keep a stopped service alive
- **WHEN** service shutdown starts with connected instance/session event streams or WebSockets
- **THEN** event stream bodies and producer tasks SHALL observe runtime shutdown
- **AND** producer tasks SHALL also end when their receiving client disconnects
- **AND** each WebSocket SHALL cancel and join its pending request tasks and release its channel-owned tool registrations
- **AND** its shutdown close-frame write SHALL be bounded so a client that stops reading cannot hold cleanup indefinitely
