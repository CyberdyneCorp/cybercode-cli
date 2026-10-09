## MODIFIED Requirements

### Requirement: Unknown and stale calls
(P0) The system SHALL settle a call to a name not advertised in the Turn with the error result `Unknown tool: <name>`. A call whose advertised registration was removed or replaced before settlement SHALL get `Stale tool call: <name>`. Neither case SHALL invoke any executor. Before reporting an unknown tool, the system SHALL retry the lookup once with the lower-cased name.

#### Scenario: Case-insensitive recovery
- **WHEN** the model calls `Read` and only `read` is advertised
- **THEN** the call is dispatched to `read`

#### Scenario: Stale call after plugin reload
- **WHEN** a plugin tool is replaced between advertisement and settlement
- **THEN** the call settles with `Stale tool call: <name>` and the new executor is not invoked

#### Scenario: Same exposed name belongs to a replacement native registration
- **WHEN** an MCP or client registration replaces an advertised registration without changing its visible name
- **THEN** runtime precheck SHALL compare the retained registration identity and refuse the old call as stale
- **AND** dispatch SHALL retain that identity outside model arguments and recheck it at the owning host before executor/channel effects

#### Scenario: Client registration takes precedence over an MCP name
- **WHEN** a client registers the same name as a shared MCP tool
- **THEN** new materializations SHALL select the effective client definition once
- **AND** a call advertised for the previous MCP registration SHALL NOT reach that client executor

#### Scenario: Client replacement during bounded channel admission
- **WHEN** a dispatched client call waits for channel capacity and its registration is replaced or cancelled
- **THEN** channel admission SHALL recheck registration/cancellation before enqueueing or inserting pending RPC ownership
- **AND** registration replies and execution requests SHALL carry the opaque identity for SDK verification against the exact channel/handler incarnation
- **AND** missing or stale SDK identities SHALL refuse handler effects
