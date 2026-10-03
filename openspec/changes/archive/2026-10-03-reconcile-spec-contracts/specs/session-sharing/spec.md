## MODIFIED Requirements

### Requirement: Manual share and confirmation
(P3) The system SHALL share a Session on `/share` (TUI), `cyber sessions share <id>`, exec `--share`, or `POST /api/v1/sessions/:id/share` with optional `visibility`. Before the first share in a project, the TUI SHALL confirm with the visibility and redaction summary. The confirmation SHALL be remembered per project. The share URL SHALL be stored on the Session and copied to the clipboard.

#### Scenario: Copy existing link
- **WHEN** the Session is already shared and the user runs `/share`
- **THEN** the existing URL is copied without creating a new share

### Requirement: Unshare and deletion
(P3) The system SHALL unshare on `/unshare`, `cyber sessions unshare <id>`, or `DELETE /api/v1/sessions/:id/share`. It SHALL delete the remote copy, clear the Session's share field, and remove the local share record. Deleting a shared Session SHALL delete its remote share. A remote deletion failure SHALL be retried and surfaced as a warning toast.

#### Scenario: Delete removes remote copy
- **WHEN** the user deletes a shared Session
- **THEN** the share service receives a delete for that share and the URL returns 404 afterwards

## REMOVED Requirements

### Requirement: Export with sanitization
**Reason**: Duplicate of `storage-events` → Session export (`cyber sessions export <id> [--sanitize] [--include-children] [--format json|md]`), which owns the contract. A separate top-level `cyber export` command is not part of the command tree.
**Migration**: Use `cyber sessions export`.

### Requirement: Import
**Reason**: Duplicate of `storage-events` → Session import (`cyber sessions import <file|share-url>`), which already covers share URLs. A session-file `cyber import` collided with `compat-import`'s `cyber import <tool>`.
**Migration**: Use `cyber sessions import`. Fetching a non-public share with the account token remains a `storage-events` concern.
