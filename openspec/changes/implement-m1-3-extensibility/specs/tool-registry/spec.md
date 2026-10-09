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

### Requirement: Per-turn materialization
(P0) The system SHALL materialize the effective tool set once per Turn for the Turn's agent and Mode. It SHALL omit a tool whose permission action has, as its last matching agent rule, resource `*` with effect `deny`, and SHALL omit mutating tools (not `read_only`) in `plan` mode except the plan-file write. Omission controls visibility only; execution still runs permission checks.

#### Scenario: Fully denied tool hidden
- **WHEN** an agent's rules end with `{ action: "webfetch", resource: "*", effect: "deny" }`
- **THEN** `webfetch` is not included in that agent's tool definitions

#### Scenario: Partially denied tool stays visible
- **WHEN** rules deny `bash` only for resource `rm *`
- **THEN** `bash` stays visible and individual calls are evaluated per call

#### Scenario: Client tools have conservative permission defaults
- **WHEN** a client registers a tool without trusted read-only annotations
- **THEN** its permission action SHALL be its exposed name with resource `*` and default ask
- **AND** current explicit/ancestor denies and Mode ceilings SHALL remain effective at execution

#### Scenario: Hidden higher registration does not reveal a lower executor
- **WHEN** a client overrides a read-only MCP name but that client tool is omitted for the current Mode or policy
- **THEN** the effective name SHALL be omitted rather than exposing the lower MCP registration
- **AND** public tool listing SHALL use the same composition as runtime materialization

### Requirement: Codec boundary on settlement
(P0) The system SHALL validate provider input against the tool's input schema before executing it, and SHALL validate the returned value against its output schema when one is declared. Invalid input SHALL settle as a model-visible error `Invalid tool input: <json-pointer>: <reason>` without executing. Invalid output SHALL settle as `Tool returned an invalid value for its output schema: <details>`.

#### Scenario: Invalid input returned to the model
- **WHEN** the model calls `edit` without `old_string`
- **THEN** the call settles with `Invalid tool input: /old_string: required` and the executor is not invoked

#### Scenario: Client schema and hook rewrite admission precedes RPC
- **WHEN** a client call supplies invalid input or a pre-tool hook rewrites its arguments
- **THEN** the declared schema SHALL be checked before client channel effects
- **AND** a valid rewrite SHALL become the input for subsequent permission and client execution
- **AND** the exact original registration identity SHALL remain retained through those checks
