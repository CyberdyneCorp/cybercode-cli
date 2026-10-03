## ADDED Requirements

### Requirement: Apply a remote session's changes locally
(P3) `cyber apply <ses_id> [--branch]` SHALL fetch a remote Session's branch and apply its diff against the Session's base commit to the local working tree as uncommitted changes (or check out the branch with `--branch`) without transferring Session ownership or history. Conflicts SHALL be reported per file with `ApplyConflictError`, leaving the tree unchanged. The remote Session SHALL remain resumable and `teleport` SHALL still be available afterwards.

#### Scenario: Pull a fix without the session
- **WHEN** the user runs `cyber apply ses_42` after a cloud Session fixed a bug
- **THEN** the local tree contains the Session's changes as uncommitted edits and the remote Session is unchanged

### Requirement: Best-of-N attempts
(P3) `cyber --cloud "<task>" --attempts <n>` (1–4, default 1) SHALL start `n` independent remote Sessions with the same prompt, repository state and Environment, each with the run Budget, and SHALL present them as a group in `cyber runners list`, the agent view and the web client with per-attempt diff summaries and cost. The user SHALL be able to `apply` or `teleport` any attempt and discard the others, which archives their Sessions.

#### Scenario: Three attempts
- **WHEN** the user runs `cyber --cloud "fix the flaky auth test" --attempts 3`
- **THEN** three Sessions run in parallel, the agent view groups them, and choosing attempt 2 applies its diff and archives attempts 1 and 3
