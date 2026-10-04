## MODIFIED Requirements

### Requirement: Reconciliation at Safe Boundaries
(P0) The system SHALL, at each Safe Boundary after initialization, observe sources once and compare them with the Context Snapshot, combining every changed, added, and removed source into a single durable `session.context.updated.1` event whose text is sent after newly promoted input as a Mid-Conversation System Message: a user-role message wrapped in `<system-reminder>` tags, because several providers reject system-role messages after the first turn, and advancing the snapshot in the same commit. A changed source SHALL never wake an idle Session.

#### Scenario: AGENTS.md edited mid-session
- **WHEN** the user edits `AGENTS.md` while a Session is idle and then sends a prompt
- **THEN** the next Turn contains one mid-conversation system message stating the new instructions replace the previous ones

#### Scenario: Update on a provider without mid-conversation system messages
- **WHEN** a Session on an Anthropic model receives a context update
- **THEN** the update is sent as a user-role message wrapped in `<system-reminder>` tags and the system prompt bytes are unchanged
