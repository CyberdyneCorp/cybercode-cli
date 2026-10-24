## MODIFIED Requirements

### Requirement: Update prompt
(P0) The TUI SHALL show the server's update check (`installation-upgrade` → Background update policy), which runs at most once per 24 hours after the first check, unless `autoupdate` is `false` or `CYBER_DISABLE_AUTOUPDATE` is set. When a newer version exists, it SHALL show a non-blocking toast `cyber X.Y.Z is available — run "cyber upgrade"`, from `installation.update_available.1` or, when it attaches after the check, from `GET /api/v1/service/update`. After a silent upgrade (`installation.updated.1`) it SHALL show `cyber updated to X.Y.Z; restart cyber to use it`. It SHALL never restart a running Session to apply an update.

#### Scenario: Update notice during a run
- **WHEN** an update is found while a Drain is running
- **THEN** the toast appears and the Drain continues uninterrupted

#### Scenario: Attaching after the check
- **WHEN** the server found 1.5.0 before the TUI attached
- **THEN** the TUI shows the toast once on start, read from `GET /api/v1/service/update`
