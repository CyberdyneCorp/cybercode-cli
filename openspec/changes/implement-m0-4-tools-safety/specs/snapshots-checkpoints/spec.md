## MODIFIED Requirements

### Requirement: Three-phase revert
(P0) Rewinds affecting the conversation SHALL be staged first: `stage { message_id, target }` restores files (when the target includes code) to their state before the target message's first Turn and records the revert state, `clear` restores the pre-stage working tree, and `commit` makes the revert final. When the target includes the conversation, `commit` SHALL delete the target user message and every message and inbox row after it, while retaining unresolved tool recovery records independently of conversation projection; a compaction whose retained tail starts after the boundary SHALL be discarded. A `code`-only commit SHALL leave the conversation unchanged. A staged revert SHALL be committed automatically on the next prompt, shell command or compaction. Restores that cannot be applied without overwriting later edits SHALL fail with `RewindConflictError` and leave the revert unstaged. The routes SHALL be `POST /api/v1/sessions/:id/revert/{stage,clear,commit}`.

#### Scenario: Undo a staged revert
- **WHEN** a revert is staged and the user calls `clear`
- **THEN** the working tree returns to its pre-stage state and the messages remain

#### Scenario: Commit removes the boundary message
- **WHEN** the user stages `both` at message 3 and commits
- **THEN** message 3 and everything after it are gone and files match their state before message 3's Turns

#### Scenario: Next prompt commits
- **WHEN** a `conversation` revert is staged and the user sends a new prompt
- **THEN** the revert is committed before the prompt is admitted and the next Turn does not include the removed messages
