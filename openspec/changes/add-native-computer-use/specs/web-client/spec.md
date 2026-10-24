## MODIFIED Requirements

### Requirement: Desktop application
(P4) The project SHALL ship a desktop application for macOS, Windows and Linux that wraps the web client in a Tauri shell (origin `tauri://localhost`, already allowed by `server-api`). It SHALL start or attach to the user's background server, register the `cyber://` URL scheme, show native notifications, support several windows on different Locations, and offer the same remote-attach flow as the web client. Native computer automation is owned by `computer-use` and does not require this desktop wrapper.

#### Scenario: Desktop attaches to the service
- **WHEN** the user opens the desktop application
- **THEN** it ensures the background server runs and lists Sessions without opening a browser
