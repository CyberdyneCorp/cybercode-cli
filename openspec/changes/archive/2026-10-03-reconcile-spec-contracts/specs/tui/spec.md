## MODIFIED Requirements

### Requirement: Launch and server transport
(P0) The system SHALL start the TUI with `cyber [project]`, resolving `project` relative to the current directory (default: the current directory). By default it SHALL connect to the registered background server, starting it as `cyber service start` does when none is registered. `--embedded` SHALL run a private in-process server with no listener on a private database, for tests and single-process offline use, and SHALL never open the shared database for writing. `cyber attach <url>` SHALL connect to a remote server. Every transport SHALL use only the public `/api/v1` routes and SSE/WebSocket event streams.

#### Scenario: Default in-process launch
- **WHEN** a user runs `cyber --embedded --ephemeral` in `/repo`
- **THEN** the TUI starts an in-process server for Location `/repo` without opening a TCP listener and writes nothing to the shared database

#### Scenario: First launch starts the service
- **WHEN** a user runs `cyber` in `/repo` with no background service registered
- **THEN** the TUI starts the background server, connects to it for Location `/repo`, and renders the home screen within 500 ms of the server reporting ready

#### Scenario: Attach to the background service
- **WHEN** `cyber service status` reports `running http://127.0.0.1:4747` and the user runs `cyber`
- **THEN** the TUI connects to that server with the stored service credentials instead of starting a new one

#### Scenario: Attach to a remote server
- **WHEN** a user runs `cyber attach https://box.example:4747 --cwd /srv/repo`
- **THEN** the TUI authenticates with the configured credentials and opens Location `/srv/repo` on the remote server

### Requirement: Mentions and fuzzy search
(P0) The system SHALL open an autocomplete menu when `@` is typed at a word boundary. The menu SHALL offer, ranked by fuzzy match:
- project files and directories (via `GET /api/v1/fs/find`, limit 20)
- non-hidden subagents
- MCP resources as `@server:uri`

A file mention SHALL accept a line range suffix `#L10` or `#L10-40`, and only that range SHALL be attached.

#### Scenario: File range mention
- **WHEN** the user selects `src/auth.rs` and types `#L10-40`
- **THEN** the submitted message carries a file part for `src/auth.rs` lines 10 through 40 only

#### Scenario: Agent mention invokes a subagent
- **WHEN** the user submits `@explore find all callers of parse_token`
- **THEN** the message is routed to the `explore` subagent as described in `agents-subagents`, without an `agent` permission prompt

### Requirement: Permission mode indicator and cycling
(P1) The system SHALL always show the Session's permission mode in the footer. Shift+Tab SHALL cycle `default → accept-edits → plan → auto → default` as defined by `permissions-modes`. `bypass` and `dont-ask` SHALL be selectable only from `/mode <name>`, flags or config, and `bypass` only when it is not disabled by org policy, with an explicit confirmation. Mode changes SHALL be recorded as durable Session events.

#### Scenario: Cycle into plan mode
- **WHEN** the Session is in `accept-edits` and the user presses Shift+Tab
- **THEN** the mode becomes `plan`, the footer shows `⏸ plan` and the next Turn uses plan-mode rules

#### Scenario: Bypass blocked by policy
- **WHEN** org policy lists `bypass` in `modes.disable` and the user runs `/mode bypass`
- **THEN** the TUI shows `bypass mode is disabled by your organization` and the mode is unchanged

### Requirement: Model, variant and agent selection
(P0) The system SHALL provide a model picker (`/model`, Ctrl+X M) listing available models grouped by provider, with Favorites and Recents sections at the top. It SHALL be persisted in `~/.local/state/cyber/model.json` (maximum 10 recents). Ctrl+T in the picker SHALL cycle reasoning variants. Tab SHALL cycle primary, non-hidden agents. Selecting a model or agent SHALL call the Session switch operations, so the change applies at the next Turn.

#### Scenario: Unavailable model rejected
- **WHEN** a favorite model's provider has no credentials
- **THEN** the picker shows it dimmed with `not connected`, and selecting it opens the connect dialog instead of switching

#### Scenario: Agent cycling
- **WHEN** the user presses Tab in the composer with primary agents `build` and `docs` available
- **THEN** the active agent switches from `build` to `docs` and the footer shows the agent's color and name

### Requirement: Remote control indicator and pairing
(P3) The system SHALL show a `⇄ remote` footer indicator, with the count of connected Devices, while the Session is exposed through the Relay. `/remote` SHALL toggle exposure, as defined in `remote-control`. When pairing a new Device it SHALL render a QR code and a short pairing code in the terminal. Messages sent from a remote Device SHALL appear in the transcript labelled with the Device name.

#### Scenario: Pair a phone
- **WHEN** the user runs `/remote` while signed in to a Cyber Account and then `cyber remote pair`
- **THEN** the TUI shows a QR code and `Pairing code: 7K4-Q9M`, and after the phone pairs the footer shows `⇄ remote (1)`

#### Scenario: Not signed in
- **WHEN** the user runs `/remote` without a Cyber Account
- **THEN** the TUI offers `Sign in with cyber login to use remote control` and changes nothing
