## MODIFIED Requirements

### Requirement: Session listing and paging
(P0) The system SHALL list Sessions filtered by directory, project, workspace, parent, archived flag, and title search, ordered by `updated_at` descending by default, with opaque cursors and a limit of 1–200 (default 50), and SHALL page a Session's messages by durable sequence with the same cursor contract. A root Session created in its own managed worktree on behalf of another Session (a forked Goal, a teammate) SHALL record that Session's directory as its source Location, and a directory filter SHALL also match root Sessions whose source Location is that directory.

#### Scenario: Next page
- **WHEN** a client requests the next page with the returned cursor
- **THEN** it receives the following 50 Sessions without duplicates

#### Scenario: Forked goal listed in the source checkout
- **WHEN** the user runs `/goal fork upgrade all dependencies` in `/repo` and then `cyber sessions list` in `/repo`
- **THEN** the forked Session, which runs in its `goal-<ulid>` worktree, is listed
