## ADDED Requirements

### Requirement: Redaction markers are never written
(P1) `write`, `edit`, `apply_patch` and `notebook_edit` SHALL refuse a change, before asking for permission, when its new text holds a redaction marker (`[REDACTED:<kind>]`) more often than the current text does. For `apply_patch` this SHALL be counted across all of the patch's files, so moving a file is not an addition.

`bash`, `powershell`, `pty_start` and `pty_write` SHALL refuse a command or input that holds a marker; this covers heredocs, `sed -i` and redirects.

The refusal SHALL name the marker, say that a secret was hidden and that the marker only stands in for it, and tell the model to leave the line unchanged or refer to the secret indirectly. An `edit` whose `old_string` holds a marker and does not match SHALL say that the file holds the hidden value. A marker the file already contains literally may be kept.

#### Scenario: Placeholder not written over a secret
- **WHEN** the model read `.env` as `API_TOKEN=[REDACTED:assignment]` and calls `write`, `edit`, `apply_patch` or `bash` to store `API_TOKEN=[REDACTED:assignment]`
- **THEN** each call fails with a message naming `[REDACTED:assignment]`, and `.env` still holds the real value

#### Scenario: Documentation keeps its example
- **WHEN** `docs.md` already contains `[REDACTED:github_token]` and the model edits another phrase of it
- **THEN** the edit succeeds
