## MODIFIED Requirements

### Requirement: Rewind targets
(P1) The system SHALL support rewinding to any user message with target `code` (restore files only), `conversation` (move the active branch to before that message, keeping later messages as an inactive branch as defined by `session-runtime` → Session branches) or `both`. It SHALL expose this through `/rewind` (Esc Esc in the TUI), `cyber sessions rewind <session> --to <msg> --target <t>`, and `POST /api/v1/sessions/:id/rewind`.

#### Scenario: Code-only rewind
- **WHEN** the user rewinds `code` to message 3
- **THEN** files return to their state before message 3's Turns and the conversation is unchanged

#### Scenario: Conversation rewind keeps a branch
- **WHEN** the user rewinds `conversation` to message 3 of 5
- **THEN** messages 3 to 5 leave the model's view and remain reachable as an inactive branch through `/tree` and `cyber sessions checkout`
