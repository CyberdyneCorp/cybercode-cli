## ADDED Requirements

### Requirement: Conflict-aware code restore
(P0) Code restore SHALL compare the target snapshot, the recorded post-edit state and the current file bytes before mutation. It SHALL preserve unrelated user and other-Session edits by three-way application where unambiguous, and fail with `RewindConflictError` listing paths otherwise. Stage and clear SHALL use the same checks. The server SHALL hold a workspace mutation lock across conflict checking and replacement, pause its other writers in the same workspace, and recheck file hashes immediately before replacement. A backup of replaced bytes SHALL be retained so a detected external race is recoverable. Symlink targets SHALL be revalidated. The UI SHALL preview the proposed diff; conflicting paths SHALL never be silently overwritten.

#### Scenario: User edits after the agent
- **WHEN** the user changes the same lines after an agent edit and then requests rewind
- **THEN** rewind reports a conflict, preserves the current file and offers the saved versions for manual resolution

### Requirement: Rewind scope disclosure
(P0) Rewind previews SHALL identify tracked file changes, excluded files and known external effects. Rewind SHALL NOT claim to reverse database writes, remote API calls, package installs outside captured paths or messages already sent. Any external effect with an unknown outcome SHALL remain unresolved after conversation or code rewind.

#### Scenario: PR survives local rewind
- **WHEN** a user rewinds a Turn that created a remote PR
- **THEN** the preview states that the PR remains remote and the recovery record is preserved

## MODIFIED Requirements

### Requirement: Three-phase revert
(P0) Rewinds affecting the conversation SHALL be staged first: `stage { message_id, target }` restores files (when the target includes code) and records the revert state, `clear` restores the pre-stage working tree, and `commit` deletes the messages and inbox rows after the boundary while retaining unresolved tool recovery records independently of conversation projection. A staged revert SHALL be committed automatically on the next prompt, shell command or compaction. The routes SHALL be `POST /api/v1/sessions/:id/revert/{stage,clear,commit}`.

#### Scenario: Undo a staged revert
- **WHEN** a revert is staged and the user calls `clear`
- **THEN** the working tree returns to its pre-stage state and the messages remain
