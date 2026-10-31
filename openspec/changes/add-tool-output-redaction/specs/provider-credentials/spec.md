## MODIFIED Requirements

### Requirement: Redaction
(P0) The system SHALL redact credential values from logs, error messages, `cyber debug config`, exported sessions and telemetry, replacing them with `***`, including values found under keys matching `key|secret|token|password|authorization|cookie|credential`. Tool output shown to the model SHALL instead have each credential value replaced by its `[REDACTED:<kind>]` marker, with resolved provider keys matched as exact values (`tool-registry` → Tool output redaction).

#### Scenario: Debug config output
- **WHEN** the user runs `cyber debug config` with a key placed inline in config
- **THEN** the printed value is `***`

#### Scenario: Provider key in tool output
- **WHEN** a tool prints the configured value of `providers.corp.api_key`
- **THEN** the model receives `[REDACTED:known_secret]` in its place
