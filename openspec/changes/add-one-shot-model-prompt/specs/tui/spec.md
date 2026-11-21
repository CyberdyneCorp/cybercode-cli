## MODIFIED Requirements

### Requirement: Model, variant and agent selection
(P0) The system SHALL provide a model picker (`/model`, Ctrl+X M) listing available models grouped by provider, with Favorites and Recents sections at the top. It SHALL be persisted in `~/.local/state/cyber/model.json` (maximum 10 recents). Ctrl+T in the picker SHALL cycle reasoning variants. Tab SHALL cycle primary, non-hidden agents. Selecting a model or agent SHALL call the Session switch operations, so the change applies at the next Turn. `/model <provider/model> <prompt>` SHALL send `<prompt>` with that `model` (`session-runtime` → Agent, model, and mode switching), so that model answers it and the Session's model answers the next prompt; it SHALL be queued while the Session runs. `/model` with no argument or a single argument SHALL open the picker as before, and a first argument without `/` followed by text SHALL show `Usage: /model <provider/model> <prompt>`.

#### Scenario: Unavailable model rejected
- **WHEN** a favorite model's provider has no credentials
- **THEN** the picker shows it dimmed with `not connected`, and selecting it opens the connect dialog instead of switching

#### Scenario: Agent cycling
- **WHEN** the user presses Tab in the composer with primary agents `build` and `docs` available
- **THEN** the active agent switches from `build` to `docs` and the footer shows the agent's color and name

#### Scenario: One-shot model prompt
- **WHEN** the user submits `/model anthropic/claude-sonnet explain this diff` in a Session on `openai/gpt-6`
- **THEN** `explain this diff` is answered by `anthropic/claude-sonnet`, the footer still shows `openai/gpt-6`, and the next prompt is answered by `openai/gpt-6`
