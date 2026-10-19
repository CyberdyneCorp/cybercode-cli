## MODIFIED Requirements

### Requirement: Typed Context Sources
(P0) The system SHALL model each Context Source as a stable key matching `^[a-z0-9]+(/[a-z0-9._-]+)+$`, a JSON codec for its value, an infallible loader, a pure baseline renderer, a pure update renderer, and an optional removal renderer. Built-in keys SHALL include `core/environment`, `core/date`, `core/instructions`, `core/skills`, `core/references`, `core/memory`, `core/mcp-instructions`, `core/messaging` (cross-session-messaging guidance, present only while the built-in messaging tools are advertised), `core/mode`, `core/goal`, and `core/output-style`, and `core/task-state` (compaction).

#### Scenario: Invalid key rejected
- **WHEN** a plugin registers a source with key `Bad Key`
- **THEN** registration fails with `ContextSourceKeyError`

#### Scenario: Messaging guidance follows the tools
- **WHEN** messaging is disabled for a Session's Location after its baseline was rendered
- **THEN** `send_message`, `list_sessions` and `watch_session` are no longer advertised and the next reconciliation withdraws `core/messaging`
