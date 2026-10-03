# client-sdk Specification

## Purpose
The client SDKs let applications, CI scripts and other agents drive Cyber Code programmatically through the public server API. They follow OpenCode v2's design (clients generated from the authoritative OpenAPI, an embedded in-process mode, application-registered tools) and the Claude Agent SDK / Codex SDK idea of scripting an agent session. They add typed helpers for workflows, goals and durable event replay.

## Requirements

### Requirement: Packages
(P0) The project SHALL publish `@cyber-code/sdk` (TypeScript, ESM, Node ≥ 20, Bun and Deno) and the `cyber-sdk` Rust crate. In P3 it SHALL also publish the Python package `cyber-code` (Python ≥ 3.10, sync and asyncio clients). All SDKs SHALL be versioned with the server minor version they were generated from.

#### Scenario: Version alignment
- **WHEN** `@cyber-code/sdk@1.4.0` connects to a server reporting version `1.6.2`
- **THEN** the client works, because `/api/v1` is additive, and exposes `server.version` for feature checks

### Requirement: Generation from OpenAPI
(P0) The SDK sources SHALL be generated from `/api/v1/openapi.json` by a repository task (`just sdk-generate`) and SHALL NOT be edited by hand. A CI check SHALL regenerate them and fail on uncommitted drift.

#### Scenario: Drift detected
- **WHEN** a route is added to the server without regenerating the SDK
- **THEN** the CI check fails naming the changed generated files

### Requirement: Client construction
(P0) `Cyber.connect({ baseUrl?, socketPath?, auth?, fetch?, headers?, directory? })` SHALL construct a client without network I/O. With no `baseUrl` or `socketPath` it SHALL read `~/.local/state/cyber/server.json` and use the registered server. `directory` SHALL be sent as `x-cyber-directory` on every Location-scoped request.

#### Scenario: Default to the registered server
- **WHEN** an application calls `Cyber.connect()` while a service is registered
- **THEN** the client targets the registered server's socket or URL with the stored password

### Requirement: Authentication helpers
(P0) The SDK SHALL support `auth: { type: "password", password }`, `auth: { type: "account", token | tokenProvider }` (Bearer, refreshed through the provider before expiry) and `auth: { type: "socket" }`. The default SHALL be the local password file when it is readable.

#### Scenario: Token refresh
- **WHEN** an account token expires during a long event stream
- **THEN** the SDK obtains a new token from `tokenProvider` and reconnects with `after=<last seq>`

### Requirement: Embedded mode
(P0) `Cyber.start({ mode: "attach" | "spawn" | "embedded" })` SHALL either attach to a running service, spawn `cyber service start` and attach, or run a private server whose lifetime is tied to the returned handle: TypeScript through `cyber serve --stdio` (`server-api` Stdio transport), Rust through in-process construction. An embedded server SHALL use a private database (`db` option, default in-memory) and SHALL never write to the user's shared database. `handle.close()` and an `AbortSignal` SHALL stop a spawned or embedded server.

#### Scenario: Private server for tests
- **WHEN** a test calls `Cyber.start({ mode: "embedded" })` and later `handle.close()`
- **THEN** a private server serves the test and is stopped, with no change to the user's registered service or database

### Requirement: Results and errors
(P0) Methods SHALL resolve with parsed bodies, unwrapping `{ data }` envelopes, and SHALL reject with typed errors that carry the server `_tag` (`SessionNotFoundError`, `EntitlementRequiredError`, …). Transport failures SHALL reject with `CyberClientError` whose `reason` is `Transport`, `UnexpectedStatus`, `MalformedResponse` or `Timeout`. Generated guards (`isSessionBusyError`, …) SHALL discriminate on `_tag`.

#### Scenario: Busy session
- **WHEN** `sessions.revert.commit()` is called while the Session is running
- **THEN** the promise rejects with an error for which `isSessionBusyError(err)` is true

### Requirement: Event streams with resume
(P0) `sessions.events(id, { after? })` and `events.subscribe()` SHALL return async iterators (TypeScript) or `Stream`s (Rust). They SHALL reconnect automatically with exponential backoff from 500 ms to 15 s, resuming durable session streams from the last seen sequence so no durable event is lost or duplicated. Iteration end or `break` SHALL close the connection.

#### Scenario: Dropped connection
- **WHEN** the network drops after the iterator yielded seq 120
- **THEN** the SDK reconnects with `after=120` and continues yielding from seq 121

### Requirement: Idempotent writes
(P0) Every mutating SDK call SHALL send an auto-generated `Idempotency-Key` (UUIDv7) unless the caller supplies one, and SHALL retry transport failures up to 3 times with the same key.

#### Scenario: Safe retry
- **WHEN** `sessions.create()` times out and the SDK retries
- **THEN** the server replays the original response and only one Session exists

### Requirement: Prompt helper
(P0) `session.prompt(parts, { delivery?, agent?, model?, mode?, system_prompt?, append_system_prompt?, tools?: { allow?, deny? }, wait? })` SHALL admit a prompt. `system_prompt` and `append_system_prompt` SHALL apply to the Session as the matching CLI flags do; `tools` SHALL narrow the Session's tool set through the Session ruleset. With `wait: true` it SHALL resolve when the Session next becomes idle, with the final assistant message, the usage totals and the reason the Drain stopped.

#### Scenario: Scripted one-shot
- **WHEN** a script calls `await s.prompt("fix the failing test", { wait: true })`
- **THEN** it receives the final assistant text after the Drain finishes

### Requirement: Application-registered tools
(P0) `client.tools.register({ name, description, input, output?, execute })` SHALL register tools that run in the calling process. The SDK SHALL hold a JSON-RPC 2.0 channel with the server (WebSocket, or stdio in embedded mode); the server SHALL forward calls to it and settle results through the standard tool boundary. Registrations SHALL disappear when the client disconnects. Names SHALL match `^[A-Za-z][A-Za-z0-9_-]{0,63}$`.

#### Scenario: App tool called by the model
- **WHEN** an application registers `lookup_order` and the model calls it
- **THEN** the server sends a JSON-RPC `tool.execute` request to the application and returns its result to the model

#### Scenario: Client disconnects
- **WHEN** the registering client disconnects mid-session
- **THEN** pending calls settle with `Tool execution interrupted` and the tool is no longer advertised from the next Turn

### Requirement: Workflow helpers
(P2) `client.workflows.run({ script | name, args, budget? })` SHALL start a Workflow Run and return a handle with `events()`, `status()`, `pause()`, `resume()`, `cancel()` and `result()`.

#### Scenario: Await a workflow
- **WHEN** a script calls `await (await client.workflows.run({ name: "audit" })).result()`
- **THEN** it receives the run's returned value or a typed `WorkflowFailedError`

### Requirement: Goal helpers
(P2) `session.goals.set(condition, { evaluatorModel?, maxTurns? })`, `.queue(condition)`, `.status()`, `.pause()`, `.resume()` and `.clear()` SHALL map to the goal routes, and `session.goals.wait()` SHALL resolve when the active goal is met, judged impossible, or cleared.

#### Scenario: Wait for a goal
- **WHEN** a CI script sets a goal and calls `wait()`
- **THEN** it resolves with `{ outcome: "met" | "impossible" | "cleared" | "failed", turns, usage }`

### Requirement: Permission and question handling
(P0) The SDK SHALL expose `client.permissions.onRequest(handler)` and `client.questions.onRequest(handler)`, which subscribe to asked events for the client's Locations and reply with the handler's result (`once`, `always`, `reject` with optional message; answer labels for questions).

#### Scenario: Programmatic approval
- **WHEN** a handler returns `"once"` for a bash permission request
- **THEN** the SDK posts the reply and the tool proceeds

### Requirement: Directory scoping
(P0) `client.at(directory)` SHALL return a scoped client that sends that directory on every Location-scoped request, without mutating the parent client.

#### Scenario: Two repos in one script
- **WHEN** a script uses `client.at("/repo/a")` and `client.at("/repo/b")`
- **THEN** each scoped client lists only the agents and sessions of its own Location

### Requirement: Remote access through the Relay
(P3) `Cyber.connectRemote({ account, server: <machine name> })` SHALL connect to a user's server through the Relay using an account token and the end-to-end encrypted Device channel defined by `remote-control`, exposing the same client surface.

#### Scenario: Script on another machine
- **WHEN** a script on a laptop connects remotely to the desktop server `workstation`
- **THEN** calls are tunneled through the Relay, encrypted end to end, and behave like local calls

### Requirement: Browser-safe schemas
(P0) `@cyber-code/sdk/schema` SHALL export browser-safe types and validators for `Session`, `Message`, `Part`, `Prompt`, `Location`, `Model`, `Agent`, `Permission`, `WorkflowRun`, `Goal` and event envelopes, with no Node built-ins.

#### Scenario: Web client bundle
- **WHEN** a web client imports `@cyber-code/sdk/schema`
- **THEN** it bundles without Node polyfills

### Requirement: Documented examples
(P0) The SDK repository SHALL ship runnable examples covering: driving a session to completion, streaming events with resume, registering an app tool, approving permissions programmatically, running a workflow (P2), and setting a goal (P2). CI SHALL type-check every example.

#### Scenario: Broken example
- **WHEN** an API change breaks an example's types
- **THEN** CI fails on the example type-check

### Requirement: Hook callbacks
(P1) `client.hooks.on(event, handler, { matcher?, timeout? })` SHALL register an in-process hook handler for the client's Sessions. The server SHALL forward matching events over the client's JSON-RPC channel as `hook/execute`, treat the returned object as a hook decision (same schema as `hooks`), merge it with configured hooks in declared order (client handlers last), and drop the registration when the client disconnects. Handlers SHALL be recorded as `hook.executed.1` events with scope `client`.

#### Scenario: Approve edits programmatically
- **WHEN** an application registers a `PreToolUse` handler that returns `{ decision: "allow" }` for edits under `docs/**`
- **THEN** those edits skip the prompt and the execution is recorded with scope `client`
