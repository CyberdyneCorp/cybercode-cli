## MODIFIED Requirements

### Requirement: Application-registered tools
(P0) `client.tools.register({ name, description, input, output?, execute })` SHALL register tools that run in the calling process. The SDK SHALL hold a JSON-RPC 2.0 channel with the server (WebSocket, or stdio in embedded mode); the server SHALL forward calls to it and settle results through the standard tool boundary. Registrations SHALL disappear when the client disconnects. Names SHALL match `^[A-Za-z][A-Za-z0-9_-]{0,63}$`.

#### Scenario: App tool called by the model
- **WHEN** an application registers `lookup_order` and the model calls it
- **THEN** the server sends a JSON-RPC `tool.execute` request to the application and returns its result to the model

#### Scenario: Client disconnects
- **WHEN** the registering client disconnects mid-session
- **THEN** pending calls settle with `Tool execution interrupted` and the tool is no longer advertised from the next Turn

#### Scenario: Client call uses ordinary hook and permission dispatch
- **WHEN** a client-registered tool is called through the application runtime
- **THEN** it SHALL pass schema/agent checks, pre-tool hooks and the ordinary permission engine before RPC effects
- **AND** PermissionRequest hooks SHALL answer only the current ask without saved approvals or hard-denial bypass
- **AND** success/failure hooks SHALL retain normal durable receipt correlation

#### Scenario: Replacement while permission admission awaits
- **WHEN** a client registration changes while an old call awaits a permission hook or approval
- **THEN** the approved old invocation SHALL remain bound to the original registration and refuse replacement channel effects as stale

#### Scenario: Client output uses the managed output budget
- **WHEN** success or expected failure text exceeds the configured tool output budget
- **THEN** model-visible text SHALL be bounded and the complete text SHALL remain in a managed artifact
