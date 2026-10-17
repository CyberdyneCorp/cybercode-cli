## MODIFIED Requirements

### Requirement: Session picker and actions
(P0) The system SHALL provide a Session picker (`/resume`, Ctrl+X L). It SHALL list root Sessions of the Location newest first, with fuzzy search over name and title and a preview of the last message. Child and forked Sessions SHALL show as an expandable tree. From the picker or the command palette the user SHALL be able to rename, fork (from the latest message or a chosen one), archive, delete (with confirmation), and export a Session. `/rename <title>` SHALL rename the current Session at once, and `/rename` without a title SHALL open a dialog prefilled with the current title (Enter saves, Esc cancels). `/rename` SHALL be listed in `/help` and in the command palette as **Rename session**, with the binding action `session_rename` (unbound by default). The header, the sidebar and the terminal window title SHALL show a new title when `session.renamed.1` or `session.title.generated.1` arrives, whichever client renamed it.

#### Scenario: Fork from a message
- **WHEN** the user opens `/fork` and chooses the third user message
- **THEN** a new Session is created containing the messages before that point, and the TUI switches to it with the chosen message prefilled in the composer

#### Scenario: Delete requires confirmation
- **WHEN** the user selects Delete on a Session with two child Sessions
- **THEN** the TUI asks `Delete session and 2 child sessions?` and deletes nothing until confirmed

#### Scenario: Rename the current Session
- **WHEN** the user runs `/rename Auth refactor`
- **THEN** the Session is renamed and the header and sidebar show `Auth refactor` once `session.renamed.1` arrives
