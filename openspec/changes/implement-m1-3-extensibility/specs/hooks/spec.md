## MODIFIED Requirements

### Requirement: Hook configuration
(P1) The system SHALL read hooks from the `hooks` config key as a map from event name to an ordered array of hook groups `{ matcher?, hooks: [handler, ...] }`, where each handler has a required `type` (`command`, `http`, `prompt`, or `mcp_tool`) and optional `timeout` (seconds), `async` (boolean), `id` and `description`. Unknown event names SHALL fail config validation with the path of the offending key.

#### Scenario: Valid PreToolUse hook
- **WHEN** `cyber.jsonc` contains a `PreToolUse` group with matcher `bash` and a command handler
- **THEN** the definition SHALL resolve with its declared matcher and command

#### Scenario: Unknown event rejected
- **WHEN** a configuration declares `hooks.BeforeEverything`
- **THEN** configuration loading SHALL fail with an invalid-config error naming that key

#### Scenario: Invalid handler contract rejected
- **WHEN** a trusted hook has an unsupported type, a missing type-specific field, an invalid selector or a timeout outside 1 through 600 seconds
- **THEN** configuration loading SHALL fail with the hook definition path
- **AND** validation SHALL execute no handlers
