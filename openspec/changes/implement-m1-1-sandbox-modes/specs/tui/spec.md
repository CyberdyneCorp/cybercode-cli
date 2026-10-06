## MODIFIED Requirements

### Requirement: Permission mode indicator and cycling
(P1) The system SHALL always show the Session's permission mode in the footer. Shift+Tab SHALL cycle `default → accept-edits → plan → auto → default` as defined by `permissions-modes`. `bypass` and `dont-ask` SHALL be selectable only from `/mode <name>`, flags or config, and `bypass` only when it is not disabled by org policy, with an explicit confirmation. Mode changes SHALL be recorded as durable Session events.

#### Scenario: Cycle into plan mode
- **WHEN** the Session is in `accept-edits` and the user presses Shift+Tab
- **THEN** the mode becomes `plan`, the footer shows `⏸ plan` and the next Turn uses plan-mode rules

#### Scenario: Rapid cycling before an API acknowledgement
- **WHEN** the user presses Shift+Tab twice from `default` without waiting for the first API response
- **THEN** the latest selection SHALL be `plan`, mode writes SHALL be serialized and the UI SHALL distinguish an unacknowledged selection from a server-confirmed pending mode

#### Scenario: Confirm bypass from command or picker
- **WHEN** the user selects bypass through `/mode bypass` or the mode picker
- **THEN** the TUI SHALL explain that ordinary permission prompts are skipped and SHALL require an explicit confirmation before requesting the switch
- **AND** Enter or cancellation SHALL NOT silently enable bypass

#### Scenario: Bypass blocked by policy
- **WHEN** org policy lists `bypass` in `modes.disable` and the user runs `/mode bypass`
- **THEN** the TUI shows `bypass mode is disabled by your organization` and the mode is unchanged
