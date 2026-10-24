## MODIFIED Requirements

### Requirement: Agent, model, and mode switching
(P0) The system SHALL record agent, model/variant, mode and output style switches as durable events (`session.agent.switched.1`, `session.model.switched.1`, `session.mode.switched.1`, `session.output_style.switched.1`). They SHALL take effect at the next Turn and append history markers that are not sent to the model. Switching to the current selection SHALL be a no-op. An output style switch SHALL record the Session's own selection (`null` returns to the configured style) and SHALL be refused for Sessions with a parent.

#### Scenario: Switch model mid-session
- **WHEN** the user switches from `openai/gpt-6` to `anthropic/claude-sonnet` during a Turn
- **THEN** the current Turn completes on `openai/gpt-6` and the next Turn uses `anthropic/claude-sonnet`

#### Scenario: Output style survives a restart
- **WHEN** the user switches a Session to output style `concise` and the server restarts
- **THEN** the Session still reports `output_style: "concise"` and its next Turn uses that style
