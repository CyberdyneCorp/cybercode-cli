## MODIFIED Requirements

### Requirement: Packages
(P0) The project SHALL publish `@cyber-code/sdk` (TypeScript, ESM, Node ≥ 20, Bun and Deno) and the `cyber-sdk` Rust crate. In P3 it SHALL also publish the Python package `cyber-code` (Python ≥ 3.10, sync and asyncio clients). All SDKs SHALL be versioned with the server minor version they were generated from.

#### Scenario: Version alignment
- **WHEN** `@cyber-code/sdk@1.4.0` connects to a server reporting version `1.6.2`
- **THEN** the client works, because `/api/v1` is additive, and exposes `server.version` for feature checks

### Requirement: Prompt helper
(P0) `session.prompt(parts, { delivery?, agent?, model?, mode?, system_prompt?, append_system_prompt?, tools?: { allow?, deny? }, wait? })` SHALL admit a prompt. `system_prompt` and `append_system_prompt` SHALL apply to the Session as the matching CLI flags do; `tools` SHALL narrow the Session's tool set through the Session ruleset. With `wait: true` it SHALL resolve when the Session next becomes idle, with the final assistant message, the usage totals and the reason the Drain stopped.

#### Scenario: Scripted one-shot
- **WHEN** a script calls `await s.prompt("fix the failing test", { wait: true })`
- **THEN** it receives the final assistant text after the Drain finishes

## ADDED Requirements

### Requirement: Hook callbacks
(P1) `client.hooks.on(event, handler, { matcher?, timeout? })` SHALL register an in-process hook handler for the client's Sessions. The server SHALL forward matching events over the client's JSON-RPC channel as `hook/execute`, treat the returned object as a hook decision (same schema as `hooks`), merge it with configured hooks in declared order (client handlers last), and drop the registration when the client disconnects. Handlers SHALL be recorded as `hook.executed.1` events with scope `client`.

#### Scenario: Approve edits programmatically
- **WHEN** an application registers a `PreToolUse` handler that returns `{ decision: "allow" }` for edits under `docs/**`
- **THEN** those edits skip the prompt and the execution is recorded with scope `client`
