## MODIFIED Requirements

### Requirement: Rewind targets
(P1) The system SHALL support rewinding to any user message with target `code` (restore files only), `conversation` (drop later messages only) or `both`. It SHALL expose this through `/rewind` (Esc Esc in the TUI), `cyber sessions rewind <session> --to <msg> --target <t>`, and `POST /api/v1/sessions/:id/rewind`.

#### Scenario: Code-only rewind
- **WHEN** the user rewinds `code` to message 3
- **THEN** files return to their state before message 3's Turns and the conversation is unchanged
