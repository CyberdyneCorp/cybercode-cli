## ADDED Requirements

### Requirement: Transcript persistence switch
(P1) `storage.persist_sessions: false` (or `CYBER_EPHEMERAL=1`) SHALL make every Session of that process ephemeral: the server opens an in-memory database, reports `durability: ephemeral` in `GET /api/v1/health` and the session header, writes no transcripts, snapshots or usage records to disk, and disables resume, cross-session messaging to other processes, remote control, loops and routines with `EphemeralModeError`. Config, credentials and memory files remain on disk. The switch SHALL be documented as trading every durability guarantee of this spec for privacy.

#### Scenario: Private session
- **WHEN** a user starts `cyber` with `CYBER_EPHEMERAL=1`
- **THEN** the header shows `durability: ephemeral`, `cyber sessions list` in another terminal shows nothing new, and `/remote` fails with `EphemeralModeError`
