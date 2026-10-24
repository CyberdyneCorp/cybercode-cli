# Architecture

Cyber Code runs one background server per user. Clients share its Sessions, permissions, scheduling and execution state. Closing a client does not stop the server's work.

```mermaid
flowchart TD
    Tui["Terminal UI"] --> Client["Rust API client"]
    Exec["cyber exec"] --> Client
    Client --> Api["Public server API"]
    Web["Local web dashboard"] --> Api
    Sdk["TypeScript SDK"] --> Api
    Mcp["MCP clients"] --> Api
    Api --> Runtime["Durable Session runtime"]
    Runtime --> Store["SQLite WAL and one writer"]
    Runtime --> Models["Model adapters"]
    Runtime --> Tools["Tools and permissions"]
    Tools --> Sandbox["OS sandbox"]
    Tools --> Snapshots["Shadow-git snapshots"]
    Sandbox --> Repo["Repository and worktrees"]
    Snapshots --> Repo
```

The server owns execution, compaction and recovery. Clients use HTTP, SSE, WebSocket or stdio JSON-RPC. SQLite records execution state; shadow-git snapshots provide file recovery. The browser bundle is embedded in the Rust binary and uses authenticated public APIs.

```mermaid
flowchart LR
    Prompt["Submit prompt"] --> Save["Persist admission"]
    Save --> Model["Call model"]
    Model --> Review["Check tool permission"]
    Review --> Execute["Run allowed tool"]
    Execute --> Record["Persist result and stream events"]
    Record --> Model
    Model --> Done["Finish turn"]
```

Built with Rust, tokio, axum, rusqlite and ratatui, with TypeScript for the browser client and SDK. See the [server lifecycle guide](local-core-guide.md#background-server-and-rebuilds), [storage decision](decisions/0001-storage-architecture.md) and [dashboard security/access guide](web-dashboard.md).
