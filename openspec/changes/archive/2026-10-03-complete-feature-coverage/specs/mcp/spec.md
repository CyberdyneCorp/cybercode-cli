## ADDED Requirements

### Requirement: Server options
(P1) A server entry MAY set `required: true` (the Session's first Turn SHALL wait up to the connect `timeout` for it and fail with `McpRequiredError` naming the server if it is not connected), `headers_command` (a shell command, run outside the sandbox and outside project trust until approved, whose JSON stdout supplies `headers`, refreshed every `headers_refresh_seconds`, default 3600, and redacted from logs) and `output_token_limit` (per-server cap on model-visible tool output, default the global `tool_output` budget). `cyber mcp get <name>` SHALL display the resolved options with header values redacted.

#### Scenario: Required server missing
- **WHEN** server `db` has `required: true` and fails to connect within 30 seconds
- **THEN** the first Turn fails with `McpRequiredError: db` and the prompt stays retryable in the inbox

### Requirement: WebSocket transport
(P2) A remote entry whose `url` uses `ws://` or `wss://` SHALL connect over the MCP WebSocket transport, with the same OAuth, status, timeout and reconnection rules as Streamable HTTP. `ws://` to a non-loopback host SHALL be refused unless `insecure: true` is set.

#### Scenario: WebSocket server
- **WHEN** config defines `{ type: "remote", url: "wss://mcp.example.com/ws" }`
- **THEN** the server connects over WebSocket and its tools are registered as `mcp__<name>__<tool>`
