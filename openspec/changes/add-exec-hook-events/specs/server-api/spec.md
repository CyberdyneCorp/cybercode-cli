## MODIFIED Requirements

### Requirement: Instance event stream
(P0) `GET /api/v1/event` SHALL return `text/event-stream`. Its first event SHALL be `server.connected`, followed by live events for the requested Location (or all Locations with `scope=all`). With `include=hook_receipts` the stream SHALL also carry `session.hook.receipt` live events (`hooks` → Hook receipt summaries for clients) for the Sessions in scope; without it no stream SHALL carry them. A `: heartbeat` comment SHALL be sent every 15 seconds. Each event SHALL carry `id`, `type`, `data`, optional `location`, and for durable events `durable: { aggregateID, seq, version }`. Responses SHALL set `Cache-Control: no-cache, no-transform` and `X-Accel-Buffering: no`.

#### Scenario: Heartbeat
- **WHEN** no events occur for 15 seconds
- **THEN** the stream emits a `: heartbeat` comment

#### Scenario: Hook receipts are opt-in
- **WHEN** one client subscribes to `/api/v1/event?scope=all` and another to `/api/v1/event?scope=all&include=hook_receipts`, and a hook runs
- **THEN** only the second receives `session.hook.receipt` events, one when the receipt is admitted and one when it settles

### Requirement: Durable session replay
(P0) `GET /api/v1/sessions/:sessionID/events?after=<seq>` SHALL replay the Session's durable events with sequence greater than `after`, then tail newly committed events without gaps. It SHALL honor `Last-Event-ID` as an alternative to `after`. `GET .../history?after&limit` (limit 1–500, default 100) SHALL return `{ data, hasMore }`. Live-only deltas (text, reasoning and tool-input fragments) SHALL be excluded from replay. (P1) With `include=hook_receipts` the stream SHALL also carry, as live events in the order the runtime published them relative to that Session's other events, the `session.hook.receipt` summaries (`hooks` → Hook receipt summaries for clients) whose `session_id` is that Session, and never another Session's (a child or sibling Session included), under the same authentication and Session authorization as the stream itself. Without it the Session stream SHALL carry no receipt summary. Any other `include` value SHALL be rejected with 400. Receipt summaries are live only and SHALL NOT be replayed or appear in history.

#### Scenario: Reconnect without loss
- **WHEN** a client reconnects with `Last-Event-ID: 41` after events 42–45 were committed
- **THEN** events 42–45 are delivered first in order, followed by live events

#### Scenario: Session stream receipts are opt-in and per Session
- **WHEN** clients follow `/api/v1/sessions/A/events?include=hook_receipts`, `/api/v1/sessions/B/events?include=hook_receipts` and `/api/v1/sessions/A/events`, and a hook of Session A runs before a later live notice of A
- **THEN** only the first stream receives A's `session.hook.receipt` events, both before the notice; B's opted-in stream and A's stream without the opt-in receive none
