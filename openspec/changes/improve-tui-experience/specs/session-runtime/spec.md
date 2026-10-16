## ADDED Requirements

### Requirement: Assistant message timing
(P1) Each assistant history entry returned by `GET .../messages` and `GET .../messages/:messageID` SHALL carry `agent` (the agent pinned by `session.step.started.1`), `started_ms` (the time of that event) and `ended_ms` (the time of `session.step.ended.1` or `session.step.failed.1`, null while the Turn runs) and `error_kind` (the `session.step.failed.1` kind, such as `interrupted`, else null). Each user entry SHALL carry `promoted_ms`. The values SHALL come from durable event times, so they are identical after replay or a server restart.

#### Scenario: Timing survives restart
- **WHEN** a Turn starts at 10:00:00.000 and ends at 10:00:11.300, and the server then restarts
- **THEN** the assistant entry still reports `started_ms` and `ended_ms` 11,300 ms apart

### Requirement: Session recap
(P1) The system SHALL generate a Session recap on `POST /api/v1/sessions/:sessionID/recap` with the `small` model role, no tools, at most 120 output tokens, and a prompt asking for one or two sentences in the first person past tense about what was done and what comes next. The result SHALL be trimmed to 400 characters with reasoning tags removed, and stored as a durable `session.recap.generated.1` event with `text`, `model` and `through_seq` (the last durable sequence it covers). The call's usage SHALL be recorded as `usage.recorded.1` with purpose `recap`. When the stored recap already covers the latest finished Turn, the route SHALL return it without a model call. `GET /api/v1/sessions/:sessionID/recap` SHALL return the latest recap or `null`. A Session with no finished Turn SHALL answer `409 ConflictError` with reason `nothing_to_recap`. A failed model call SHALL answer the model error (`503 ServiceUnavailableError` with `service: "model"`) and leave the previous recap in place; a reply that is empty after cleaning records its usage and answers the same way. The recap also carries `generated_ms` (the event time). Concurrent requests for one Session are served one at a time, so they make at most one model call. The prompt holds the newest 16,000 characters of the conversation (user and assistant text, each cut to 2,000 characters, and the names of the tools each assistant message called).

#### Scenario: Cached recap
- **WHEN** a recap exists through the latest finished Turn and a client posts to `/recap` again
- **THEN** the stored recap is returned and no model call is made

#### Scenario: Recap usage is attributed
- **WHEN** a recap is generated
- **THEN** a `usage.recorded.1` event with purpose `recap` is appended and counted in the Session's totals
