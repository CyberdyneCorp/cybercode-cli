## MODIFIED Requirements

### Requirement: Embedded mode
(P0) `Cyber.start({ mode: "attach" | "spawn" | "embedded" })` SHALL either attach to a running service, spawn `cyber service start` and attach, or run a private server whose lifetime is tied to the returned handle: TypeScript through `cyber serve --stdio` (`server-api` Stdio transport), Rust through in-process construction. An embedded server SHALL use a private database (`db` option, default in-memory) and SHALL never write to the user's shared database. `handle.close()` and an `AbortSignal` SHALL stop a spawned or embedded server.

#### Scenario: Private server for tests
- **WHEN** a test calls `Cyber.start({ mode: "embedded" })` and later `handle.close()`
- **THEN** a private server serves the test and is stopped, with no change to the user's registered service or database
