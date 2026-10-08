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

### Requirement: Hook scopes and merge order
(P1) The system SHALL collect hooks from the managed, global (`~/.config/cyber`), project (`cyber.jsonc` and `.cyber/` files) and local (`.cyber/cyber.local.jsonc`) scopes plus enabled plugins, and SHALL run every matching hook from every scope rather than letting one scope replace another. Hooks SHALL execute in the order managed, global, project, local, plugin. When `policy.hooks.managed_only` is true, only managed-scope hooks SHALL run.

#### Scenario: Hooks from two scopes both run
- **WHEN** global and project scopes each define a `PostToolUse` hook matching `edit`
- **THEN** both contributions SHALL survive configuration resolution and run in scope order

#### Scenario: Per-handler origins remain available
- **WHEN** hook groups from different configuration files are appended
- **THEN** every group and handler SHALL retain its own source label at its resolved index
- **AND** an empty later event array SHALL NOT erase earlier groups

#### Scenario: Managed-only policy
- **WHEN** the organization policy sets `hooks.managed_only: true`
- **THEN** project, local, global and plugin hooks SHALL be skipped and a single notice SHALL list how many were skipped
