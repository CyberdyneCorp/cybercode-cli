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

### Requirement: Hook callbacks
(P1) `client.hooks.on(event, handler, { matcher?, timeout? })` SHALL register an in-process hook handler for the client's Sessions. The server SHALL forward matching events over the client's JSON-RPC channel as `hook/execute`, treat the returned object as a hook decision (same schema as `hooks`), merge it with configured hooks in declared order (client handlers last), and drop the registration when the client disconnects. Handlers SHALL be recorded as `hook.executed.1` events with scope `client`.

The SDK registers over the same JSON-RPC channel as application tools (`v1.hook.register` `{event, matcher?, timeout?}` returns a `hkr_` registration ID; `v1.hook.unregister` `{registration_id}` removes one) and restores its registrations after a reconnect. A registration applies only to the client's Sessions and their descendants: the Sessions the client creates or forks (over HTTP the SDK attaches each one; over the channel the server does), and those it attaches with `client.hooks.attach(sessionID)` (`v1.hook.attach` `{session_id}`). Another client's Sessions, and Sessions nobody attached, never reach the handler. When the client disconnects, an in-flight `hook/execute` fails as disconnected at once instead of waiting for its timeout. The event, matcher (the configured syntax: a glob, `*` or `/regex/`) and timeout (1 through 600 seconds, default 60) are validated at registration. `hook/execute` carries `{registration_id, event, input}`, where `input` is the event envelope configured handlers receive. A handler that throws, returns invalid JSON or does not answer in time is a non-blocking error; it never fails closed. A managed-only hooks policy refuses client handlers like every other non-managed scope. Client handlers are never read from configuration (`type: client` is rejected) and run outside every sandbox, in the client's own process.

#### Scenario: Approve edits programmatically
- **WHEN** an application registers a `PreToolUse` handler that returns `{ decision: "allow" }` for edits under `docs/**`
- **THEN** those edits skip the prompt and the execution is recorded with scope `client`

#### Scenario: Other edits still ask
- **WHEN** the same handler returns no decision for an edit under `src/`
- **THEN** that edit raises its ordinary permission request

#### Scenario: Another client's Session
- **WHEN** one client registers a `PreToolUse` handler that allows everything and another client's Session edits a file
- **THEN** the handler receives no event for that Session and the edit raises its ordinary permission request

#### Scenario: Registration ends with the connection
- **WHEN** the registering client disconnects
- **THEN** the registration is dropped, later events are not forwarded, and an in-flight execution fails as disconnected without blocking the call

#### Scenario: Invalid registration refused
- **WHEN** a client registers an unknown event, a malformed matcher or a timeout outside 1 through 600
- **THEN** the registration fails with JSON-RPC `-32602` and nothing is registered
