## Why

The TUI sidebar hid the MCP section whenever no server was configured, so users could not tell whether MCP was set up at all, and a configured server that no Session had opened yet was reported as `failed` ("MCP server has not started for this Location"), which reads as an error. The P0 `cyber mcp add|list|remove` commands were also missing, so the only way to add a server was editing JSONC by hand.

## What Changes

- New status `idle` for a configured, allowed server that no Session has opened at the Location yet (never `failed`); `GET /api/v1/mcp` rows gain `transport` (`remote` or `stdio`).
- `cyber mcp list` (live status, transport, tools), `cyber mcp add <name> [--scope user|project|local] (--url <url> [--header K=V]... | [--env K=V]... -- <command> [args...])` and `cyber mcp remove <name> [--scope ...]`, editing the right document while keeping every comment.
- The sidebar MCP section is always shown when the route answers: every server with its status word, transport and tool count, a note line for failed (sanitized error) and needs-auth (`cyber mcp auth <name>`) servers, `Blocked` for policy-blocked servers, at most six rows then `+N more · /status`, and an empty state with the `cyber mcp add` hint.

## Impact

- `crates/cyber-server/src/runtime/mcp.rs`, `crates/cyber-tools/src/mcp/pool.rs`, `crates/cyber-core/src/config/{mcp.rs,edit.rs}`, `crates/cyber-cli/src/commands/{mcp.rs,mcp_config.rs}`, `crates/cyber-tui/src/{sidebar.rs,view/sidebar.rs}`, `sdk/openapi.json` and the generated SDK.
