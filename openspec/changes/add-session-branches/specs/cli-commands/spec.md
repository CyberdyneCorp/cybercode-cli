## ADDED Requirements

### Requirement: Session branch commands
(P1) `cyber sessions` SHALL provide:

- `tree <session>`: prints the user-message tree as ASCII with the active path marked, or as the server's tree nodes with `--format json`.
- `checkout <session> <message|label> [--code] [--exact]`: makes a branch active.
- `label <session> <name> [--at <message>] [--delete]`: sets or removes a label. Without `--at`, it labels the active tip.

A message argument SHALL accept an ID, a unique ID prefix or a label. A busy Session SHALL make `checkout` exit 1 with `SessionBusyError`, and an ambiguous prefix SHALL exit 2 and list the matching IDs.

#### Scenario: Checkout from the CLI
- **WHEN** the user runs `cyber sessions checkout "Auth refactor" fix-a --code`
- **THEN** the labeled branch becomes active, files are restored to its last snapshot, and the command prints the new tip and the restored file count
