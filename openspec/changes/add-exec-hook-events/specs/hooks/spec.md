## ADDED Requirements

### Requirement: Hook receipt summaries for clients
(P1) When a hook receipt is admitted and when it settles, the runtime SHALL publish a live summary with the receipt `id`, `hook_id`, `event`, `scope`, `call_id` and `tool_name` when known, `started_ms`, and, once settled, `duration_ms`, `outcome` and the decision kind. The summary SHALL have no field for stdin, stdout, stderr, the decision reason, rewritten input, added context or environment, so none of them can reach a client even when `telemetry.log_hook_io` is on. It SHALL reach a client only on an instance stream that asks for it (`server-api` → Instance event stream) as `session.hook.receipt` with `data: { session_id, receipt }`, never on Session replay, history, WebSocket or MCP streams. It is live only: the durable receipts listed by `GET /api/v1/sessions/{sessionID}/hook-executions` remain the record, and a client that must not miss a receipt SHALL reconcile with that listing.

#### Scenario: Settled summary without IO
- **WHEN** a `PreToolUse` command hook with `log_hook_io` on prints to stderr and denies a `bash` call
- **THEN** an opted-in stream receives a `session.hook.receipt` with `outcome: "blocked"`, `decision: "deny"` and `tool_name: "bash"`, and no stderr text
