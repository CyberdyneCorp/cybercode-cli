## MODIFIED Requirements

### Requirement: Side chat
(P1) The system SHALL support an ephemeral side chat (`/btw <question>`) that sends the question with a read-only copy of the current context to the Session's model using read-only tools only, shows the answer to the user, and records nothing in the Session's model-visible history.

#### Scenario: Quick question while working
- **WHEN** the user runs `/btw what does this regex match?` during a Drain
- **THEN** the answer is shown in a side panel and the main Drain's history is unchanged

#### Scenario: Side chat API and tools
- **WHEN** a client posts `POST /api/v1/sessions/{id}/side` with `{ question }`
- **THEN** the Session's model SHALL receive the Session's system prompt and model view plus the question, with only the `read`, `list`, `glob` and `grep` tools the Session's rules offer, for at most 8 requests
- **AND** a call to any other tool SHALL be answered `Unknown tool in a side chat`, and a call that would ask for permission SHALL be refused, since nobody can answer it
- **AND** the response SHALL be `{ answer, model, tools }`; only the call's usage SHALL be recorded (`purpose: side_chat`), with no message, call or inbox row added to the Session
