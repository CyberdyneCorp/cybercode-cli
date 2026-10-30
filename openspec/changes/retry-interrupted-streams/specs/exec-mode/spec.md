## MODIFIED Requirements

### Requirement: Stream JSON event schema
(P0) In `stream-json` mode, each line SHALL be an object with `type`, `ts` (epoch ms) and `session_id`. The types are:

| type | Payload | When |
|---|---|---|
| `system` (`subtype: "init"`) | model, agent, mode, cwd, tool names, MCP server statuses | first |
| `assistant_text` | message ID, text | each finished text part |
| `reasoning` | reasoning text | finished reasoning parts, only with `--thinking` |
| `tool_use` | call ID, tool, input | — |
| `tool_result` | call ID, status, output (truncated to 2,000 chars) | — |
| `retry` | message ID, attempt, delay_ms, error, discarded | each provider retry |
| `permission_denied` | — | each denial |
| `goal_verdict` | — | goal-driven runs |
| `workflow_progress` | — | workflow runs |
| `error` | — | — |
| `result` | same shape as the json-mode result | last |

The schema SHALL be versioned by `schema_version` in the `init` event (starting at `1`), and changes SHALL be additive within a major version. Only recorded content SHALL produce `assistant_text`, `reasoning` and `tool_use` lines, so an attempt the retry policy discarded never appears.

#### Scenario: Event order
- **WHEN** a stream-json run makes one bash call and finishes
- **THEN** stdout lines are, in order: `system`, `tool_use`, `tool_result`, `assistant_text`, `result`

#### Scenario: Answer replayed after a dropped connection
- **WHEN** the provider connection drops after part of the answer and the replayed request completes
- **THEN** stdout has one `retry` line with `discarded: true`, then a single `assistant_text` holding only the replayed answer
