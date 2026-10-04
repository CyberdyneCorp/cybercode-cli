# Design

## Decisions

**The router knows only the runtime.** `cyber-server::http` takes a `Runtime`, the store and a `Services` trait for catalogs and Location lookups. Assembly lives in `cyber-app`, because the tool host depends on the server crate and not the other way round. Every client uses the same router. The TUI and `cyber exec` go over TCP to the background service, or through `EmbeddedClient` in process, and never call the runtime directly.

**Schemas come from code.** Request and response types derive `JsonSchema`, and one operation table produces the OpenAPI 3.1 document. A test checks that every documented operation is routed. A second test compares the committed `sdk/openapi.json`, from which the TypeScript SDK is generated.

**Streams never lose durable events.** The Session stream subscribes to the live bus before replaying from the store. It then follows live commits, re-reading from the store on lag or on a sequence gap. Clients resume from the last sequence they saw. The instance stream carries live deltas, idle and error notices, filtered by Location.

**Idempotency is durable.** Keys, request hashes and responses live in SQLite for 24 hours. Concurrent requests with the same key get a conflict instead of a second execution.

**Exec never waits for a human.** New exec Sessions start in `dont-ask` with Session rules that deny `question`, `plan_enter` and `plan_exit`, so those tools are not even offered. Budgets are enforced by the client: it watches step usage and interrupts the Drain, so the server stays budget-agnostic until the observability milestone.

**The TUI is a state machine.** Keys and server events update `App` and return `Action`s, which the runner performs through the API. Results come back as messages. Rendering and behavior are tested on an in-memory terminal. The TUI refreshes Session snapshots on durable events, at most one refresh in flight, and shows live text deltas until the message is durable.

## Tradeoffs

Refreshing whole snapshots is simple and correct for local servers, but very long Sessions will want incremental updates later. Client-side budget checks stop a run up to one Turn late, which matches the spec ("checked after each Turn").
