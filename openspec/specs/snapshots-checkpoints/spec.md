# snapshots-checkpoints Specification

## Purpose
Records the working tree around every model step in a private git repository separate from the user's history. This lets Cyber Code show per-turn diffs and rewind code, conversation, or both, to any earlier point. It takes OpenCode's shadow-git snapshots, which capture all changes including shell and subagent edits (Claude Code's checkpointing does not track either), OpenCode v2's three-phase revert (stage, clear, commit), and Claude Code's `/rewind` choice of code-only, conversation-only or both.

## Requirements

### Requirement: Snapshot enablement
(P0) The system SHALL take snapshots when `snapshots` is not `false` (default enabled). In git repositories it SHALL use the shadow-git mechanism, and in non-git directories it SHALL use the file-copy fallback. When disabled, steps SHALL carry no snapshot IDs and rewinding SHALL affect only the conversation.

#### Scenario: Snapshots disabled
- **WHEN** config sets `"snapshots": false`
- **THEN** `/rewind` offers only "conversation"

### Requirement: Shadow repository location
(P0) The system SHALL store snapshots in a separate git directory `<data>/snapshot/<project_id>/<sha1(worktree path)[0..16]>` that uses the project worktree as its work tree and shares the project's objects through git alternates. On first use it SHALL initialize with `core.autocrlf=false` and `core.fsmonitor=false`. It SHALL never modify the user's index, refs, stash or config.

#### Scenario: User history untouched
- **WHEN** snapshots are taken during a Session
- **THEN** `git status`, `git log` and `git stash list` in the user's repository are unchanged

### Requirement: Tracked file set
(P0) A snapshot SHALL include modified tracked files and untracked files under the Location, respecting `.gitignore`, `.git/info/exclude` and `snapshots.ignore`. It SHALL exclude untracked files larger than `snapshots.max_file_bytes` (default 2 MiB) and record them as `skipped` in step metadata.

#### Scenario: Large binary skipped
- **WHEN** the model creates an untracked 50 MiB file
- **THEN** the snapshot excludes it and the step lists it under `skipped`

### Requirement: Per-step snapshots capture all changes
(P0) The system SHALL take a snapshot before each Turn and after each Turn's tool settlements, recording the tree IDs on the step markers. When files differ, it SHALL record a `patch` part listing the changed paths with the starting snapshot ID. Changes made by `bash`, background tasks, formatters, subagents in the same worktree and external editors during the step SHALL be captured.

#### Scenario: Shell change captured
- **WHEN** the model runs `sed -i 's/a/b/' src/x.rs` via `bash`
- **THEN** the step's patch lists `src/x.rs`

### Requirement: Non-git fallback
(P1) In non-git Locations the system SHALL snapshot by copying changed files (detected by mtime and size, verified by BLAKE3 hash) into `<data>/snapshot/<project_id>/files/<hash>`, up to `snapshots.fallback_max_bytes` (default 200 MiB per Location). When the limit is exceeded it SHALL disable code rewind for that Location with a one-time warning.

#### Scenario: Non-git project
- **WHEN** a Session runs in a folder without git
- **THEN** edits are still rewindable through the file-copy store

### Requirement: Per-turn diff summary
(P0) After each Turn the system SHALL compute file diffs for the user message from the first pre-Turn snapshot to the last post-Turn snapshot and store `{ file, status (added|deleted|modified), additions, deletions, patch }` on the message summary. It SHALL publish `session.diff.1` and serve the diffs at `GET /api/v1/sessions/:id/diff?message_id=`.

#### Scenario: Diff for a turn
- **WHEN** a client requests the diff for a user message whose Turns edited two files
- **THEN** two entries with additions and deletions are returned

### Requirement: Rewind targets
(P1) The system SHALL support rewinding to any user message with target `code` (restore files only), `conversation` (drop later messages only) or `both`. It SHALL expose this through `/rewind` (Esc Esc in the TUI), `cyber sessions rewind <session> --to <msg> --target <t>`, and `POST /api/v1/sessions/:id/rewind`.

#### Scenario: Code-only rewind
- **WHEN** the user rewinds `code` to message 3
- **THEN** files return to their state before message 3's Turns and the conversation is unchanged

### Requirement: Three-phase revert
(P0) Rewinds affecting the conversation SHALL be staged first: `stage { message_id, target }` restores files (when the target includes code) and records the revert state, `clear` restores the pre-stage working tree, and `commit` deletes the messages and inbox rows after the boundary while retaining unresolved tool recovery records independently of conversation projection. A staged revert SHALL be committed automatically on the next prompt, shell command or compaction. The routes SHALL be `POST /api/v1/sessions/:id/revert/{stage,clear,commit}`.

#### Scenario: Undo a staged revert
- **WHEN** a revert is staged and the user calls `clear`
- **THEN** the working tree returns to its pre-stage state and the messages remain

### Requirement: Revert snapshot of the current tree
(P0) Before the first stage on a Session, the system SHALL snapshot the current working tree as the revert baseline (reused on repeated stages). It SHALL store a unified diff between the baseline and the reverted tree on the revert state.

#### Scenario: Re-staging reuses baseline
- **WHEN** the user stages a revert to message 5 and then to message 3
- **THEN** both restores are computed from the same baseline and `clear` returns to it

### Requirement: Busy guard
(P0) Stage, clear, commit and rewind SHALL fail with HTTP 409 `SessionBusyError` while the Session's Drain is running. The TUI SHALL offer to interrupt first.

#### Scenario: Rewind during a running turn
- **WHEN** a client posts a rewind while the Session is busy
- **THEN** the response is 409 `SessionBusyError`

### Requirement: Undo and redo shortcuts
(P1) The TUI SHALL provide `/undo` (stage a revert to the previous user message and restore that message's text into the prompt) and `/redo` (move the staged boundary forward, or clear the revert when none remains).

#### Scenario: Undo restores prompt text
- **WHEN** the user runs `/undo`
- **THEN** the previous user message text appears in the input box and its Turns' file changes are reverted

### Requirement: Subagent and worktree snapshots
(P2) Subagents running in the parent's worktree SHALL contribute their changes to the parent's step snapshots. Subagents isolated in their own worktree SHALL keep separate snapshot repositories keyed by that worktree path.

#### Scenario: Isolated subagent rewind
- **WHEN** a worktree-isolated subagent's Session is rewound
- **THEN** only its worktree files are restored

### Requirement: Snapshot garbage collection
(P0) The system SHALL run `git gc --prune=7.days` on each shadow repository about 1 minute after server start and then every 24 hours. It SHALL prune file-copy fallback objects not referenced by any Session updated in the last `snapshots.retention_days` (default 7).

#### Scenario: Old snapshots pruned
- **WHEN** a snapshot is unreferenced and older than 7 days
- **THEN** it is removed at the next gc run

### Requirement: Snapshot events
(P0) The system SHALL publish `snapshot.taken.1` `{ session_id, step_id, tree, changed }` and `session.reverted.1` `{ session_id, message_id, target, phase }` as durable events.

#### Scenario: Revert event published
- **WHEN** a revert is committed
- **THEN** `session.reverted.1` with `phase: "commit"` is stored

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
