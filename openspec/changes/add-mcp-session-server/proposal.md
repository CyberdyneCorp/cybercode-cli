## Why

Product decision of 2026-10-12: Cyber Code must be usable as an MCP server in P1, so external agents (Claude Code, Codex, another Cyber, IDEs) can create and drive Sessions, exchange messages with them and split work by driving several Sessions. The decision fixes a **trusted local client** model: an MCP client has the same trust as the TUI and the TypeScript SDK, uses the same server-API authentication, is limited to loopback or the Unix socket, and gets no new privilege model.

Today `mcp` "MCP server mode" is one P2 paragraph with no transport, result, approval or limit contract, while the ROADMAP already lists `cyber mcp serve` in P1.

## What Changes

- New capability `mcp-server` (11 P1 requirements, 4 P2, 1 P3):
  - `cyber mcp serve` as a stdio bridge to the shared server, and Streamable HTTP at `/api/v1/mcp/serve` on the existing listener (route group `mcp`), loopback or Unix socket only, with the server API's local authentication.
  - One MCP revision, `2025-06-18`, with normal version negotiation; tools only; no tasks, elicitation, resources, prompts or sampling.
  - Ten tools as a thin mapping over existing routes: `cyber_session_create|list|read|transcript`, `cyber_prompt` (wait with timeout, or no wait with a receipt), `cyber_prompt_wait`, `cyber_interrupt`, `cyber_message_list|send|history`. Existing idempotency keys, tagged errors, durable events and messaging semantics are reused.
  - Approvals are not relayed: a human client or the Session Mode's non-interactive rules handle them, and results report `waiting_for_approval`; Sessions created through MCP carry the `exec-mode` non-interactive Session rules (no `question`).
  - Safety bounds: a Mode ceiling for MCP-created Sessions (default the user's global `mode`, never `bypass`), Sessions per connection, concurrent waits, running MCP Sessions, output and transcript size limits, a recursion depth guard (`CYBER_MCP_DEPTH`, `X-Cyber-MCP-Depth`, `_meta` marker) and usage origin `mcp`.
  - Later phases, one short requirement each: per-client grants (P2), approval relay through elicitation (P2), team tasks over MCP (P2), workflow and goal tools (P2), remote access with OAuth (P3).
- `mcp` "MCP server mode" moves from P2 to P1 and points at `mcp-server`; the workflow and goal tool names and `--port` move out.
- `cross-session-messaging` "Usage attribution" keeps its sentence and adds usage origin `mcp` (with `mcp_client`) for runs started by MCP input (prompts and messages sent through MCP); messages keep their Messaging API origin for inbound rules.
- `configuration` "Top-level keys" adds the global-only key `mcp_serve`.

## Capabilities

### New Capabilities
- `mcp-server`: expose Cyber Code Sessions to external MCP clients as trusted local clients of the server API.

### Modified Capabilities
- `mcp`: "MCP server mode" (P2 → P1, now a pointer).
- `cross-session-messaging`: "Usage attribution".
- `configuration`: "Top-level keys".

`server-api` and `cli-commands` need no change: the endpoint lives in the existing `mcp` route group, and `mcp serve` is already registered under the `mcp` command group.

## Impact

- Specification only; nothing is implemented.
- Code areas when implemented: `crates/cyber-server/src/http/` (MCP endpoint, loopback and Host guard, mapping onto the existing handlers), `crates/cyber-cli/src/commands/mcp.rs` (`serve` bridge), the runtime (`mcp_depth`, `mcp_client`, `CYBER_MCP_DEPTH`), Cyber's MCP client (`_meta` caller marker), usage records (origin `mcp`).
- Prerequisites: the messaging tools depend on the `cross-session-messaging` P1 core and its Messaging API, which are not implemented yet.
- Sequencing: archive after `implement-m0-1-skeleton`, which also modifies `configuration` "Top-level keys" (it adds `autoupdate`); this restatement already includes that key.
- Canonical inventory after archive: +1 capability and +16 requirements (P1 +11, P2 +4, P3 +1); "MCP server mode" moves from P2 to P1 (P1 +1, P2 −1).
