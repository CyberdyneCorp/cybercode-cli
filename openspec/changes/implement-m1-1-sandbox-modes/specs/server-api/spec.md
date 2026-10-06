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
