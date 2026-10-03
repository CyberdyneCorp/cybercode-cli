## MODIFIED Requirements

### Requirement: Per-Session serialization
(P0) The system SHALL run at most one Drain per Session. Drains SHALL run only in the registered server that holds the writer ownership lock (`storage-events`), coordinated in-process, while different Sessions run concurrently. Any other process (TUI, `cyber exec`, SDK) that wants a Session drained SHALL admit its input through the server API and wake the owner; it SHALL NOT run a Drain of its own against the shared database. An `--embedded` process owns a private database and drains only its own Sessions.

#### Scenario: TUI and exec on the same Session
- **WHEN** `cyber exec -s ses_1 "..."` runs while the TUI is attached to `ses_1`
- **THEN** the exec prompt is admitted and executed by the server's Drain, and exec streams the result from the event stream

### Requirement: Durable event replay
(P0) The system SHALL expose each Session's durable events as a finite page (`GET /api/v1/sessions/{id}/history?after=<seq>&limit=<1..500>`) and as an SSE stream (`GET /api/v1/sessions/{id}/events?after=<seq>`) that replays events after the exclusive sequence and then tails new durable events without gaps.

#### Scenario: Lossless reconnect
- **WHEN** a client reconnects with `after=1042`
- **THEN** it receives every durable event with sequence 1043 and higher, in order
