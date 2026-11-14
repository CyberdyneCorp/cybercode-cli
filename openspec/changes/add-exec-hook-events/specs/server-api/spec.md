## MODIFIED Requirements

### Requirement: Instance event stream
(P0) `GET /api/v1/event` SHALL return `text/event-stream`. Its first event SHALL be `server.connected`, followed by live events for the requested Location (or all Locations with `scope=all`). With `include=hook_receipts` the stream SHALL also carry `session.hook.receipt` live events (`hooks` → Hook receipt summaries for clients) for the Sessions in scope; without it no stream SHALL carry them. A `: heartbeat` comment SHALL be sent every 15 seconds. Each event SHALL carry `id`, `type`, `data`, optional `location`, and for durable events `durable: { aggregateID, seq, version }`. Responses SHALL set `Cache-Control: no-cache, no-transform` and `X-Accel-Buffering: no`.

#### Scenario: Heartbeat
- **WHEN** no events occur for 15 seconds
- **THEN** the stream emits a `: heartbeat` comment

#### Scenario: Hook receipts are opt-in
- **WHEN** one client subscribes to `/api/v1/event?scope=all` and another to `/api/v1/event?scope=all&include=hook_receipts`, and a hook runs
- **THEN** only the second receives `session.hook.receipt` events, one when the receipt is admitted and one when it settles
