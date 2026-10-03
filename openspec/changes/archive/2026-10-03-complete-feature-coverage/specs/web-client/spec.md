## ADDED Requirements

### Requirement: Local web client
(P1) The system SHALL bundle a browser client into the `cyber` binary and serve it from the local server at `GET /` (and `/app/*`). `cyber web [--hostname <host>] [--port <n>] [--no-open]` SHALL ensure the background server is running and open the client in the default browser, authenticated with the local password through the login page (never via a URL token). The web client SHALL offer the same surface as the TUI through the public API: session list and picker, composer with steer and queue, transcript with diffs, permission and question prompts, `/tasks`, workflow monitor, goal panel, model and mode pickers, and settings backed by `PATCH /api/v1/config`. No Cyber Account SHALL be required. Binding a non-loopback `--hostname` SHALL print a warning that the password protects the server and SHALL require TLS (`server.tls.cert`/`key`) unless `--insecure-lan` is passed.

#### Scenario: Open the web client
- **WHEN** the user runs `cyber web`
- **THEN** the browser opens `http://127.0.0.1:4747/`, asks for the local password once, and shows the Sessions of the current directory

#### Scenario: LAN access requires TLS
- **WHEN** the user runs `cyber web --hostname 0.0.0.0` without TLS configured or `--insecure-lan`
- **THEN** the command fails with `binding a non-loopback address requires server.tls or --insecure-lan` and exit code 2

### Requirement: Shared bundle for Relay and desktop
(P3) The Relay web client (`remote-control`) SHALL be the same bundle, loaded with the Relay transport (OIDC login, Device pairing, end-to-end encrypted channel) instead of the local password transport. Feature probes SHALL come from `GET /api/v1/health` in both cases, so the client hides screens its server does not implement.

#### Scenario: One bundle, two transports
- **WHEN** the same client build is served by a local server and by the Relay
- **THEN** both show identical screens for the features the connected server reports in `features`

### Requirement: Desktop application
(P4) The project SHALL ship a desktop application for macOS, Windows and Linux that wraps the web client in a Tauri shell (origin `tauri://localhost`, already allowed by `server-api`). It SHALL start or attach to the user's background server, register the `cyber://` URL scheme, show native notifications, support several windows on different Locations, and offer the same remote-attach flow as the web client. Computer-use style desktop automation is out of scope.

#### Scenario: Desktop attaches to the service
- **WHEN** the user opens the desktop application
- **THEN** it ensures the background server runs and lists Sessions without opening a browser
