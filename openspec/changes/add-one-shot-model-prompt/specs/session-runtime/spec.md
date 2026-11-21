## MODIFIED Requirements

### Requirement: Agent, model, and mode switching
(P0) The system SHALL record agent, model/variant, mode and output style switches as durable events (`session.agent.switched.1`, `session.model.switched.1`, `session.mode.switched.1`, `session.output_style.switched.1`). They SHALL take effect at the next Turn and append history markers that are not sent to the model. Switching to the current selection SHALL be a no-op. A prompt MAY name a `model` (`provider/model[#variant]`) for itself alone: the Turns that answer it, tool calls included, SHALL run on that model until its final text reply, the Session's own model SHALL NOT change and no switch event SHALL be recorded, and the next prompt SHALL use the Session's model again. In a plan phase it SHALL take precedence over `model_roles.plan` for that prompt without suspending the planner, and its steps SHALL have no `model_role`. A prompt `model` that does not parse or resolve SHALL be refused before admission, and it SHALL take precedence over a command's `model`. The inbox row SHALL report the prompt's `model`, and an exact retry with another `model` SHALL be another prompt. An output style switch SHALL record the Session's own selection (`null` returns to the configured style) and SHALL be refused for Sessions with a parent.

#### Scenario: Switch model mid-session
- **WHEN** the user switches from `openai/gpt-6` to `anthropic/claude-sonnet` during a Turn
- **THEN** the current Turn completes on `openai/gpt-6` and the next Turn uses `anthropic/claude-sonnet`

#### Scenario: Output style survives a restart
- **WHEN** the user switches a Session to output style `concise` and the server restarts
- **THEN** the Session still reports `output_style: "concise"` and its next Turn uses that style

#### Scenario: One prompt on another model
- **WHEN** a prompt on a `openai/gpt-6` Session names `model: "anthropic/claude-sonnet"` and its answer calls `read`
- **THEN** both Turns of that answer use `anthropic/claude-sonnet`, the Session still reports `openai/gpt-6`, and the next prompt runs on `openai/gpt-6`

#### Scenario: Unknown prompt model
- **WHEN** a prompt names `model: "missing/model"`
- **THEN** it is refused and nothing is admitted
