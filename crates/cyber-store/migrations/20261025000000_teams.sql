-- Agent teams (`agent-teams`): projections of the `team.*` events of the lead's stream,
-- the team's single writer. Rebuildable from events. Teammates are root Sessions, so no
-- row references `session`: a team outlives the deletion of its lead (it is stopped with
-- `lead_deleted` instead).
CREATE TABLE team (
    id            TEXT PRIMARY KEY,               -- team_<ulid>
    name          TEXT NOT NULL,                  -- definition name or "adhoc"
    lead_id       TEXT NOT NULL,                  -- the lead Session
    directory     TEXT NOT NULL,
    status        TEXT NOT NULL,                  -- active | stopping | stopped
    stop_reason   TEXT,                           -- user | budget | lead_deleted
    budget        TEXT,                           -- Budget JSON
    instructions  TEXT,                           -- lead instructions from the definition body
    created_ms    INTEGER NOT NULL,
    updated_ms    INTEGER NOT NULL,
    former_leads  TEXT NOT NULL DEFAULT '[]'      -- JSON array of earlier leads (lead_deleted)
);
CREATE INDEX team_lead ON team (lead_id);
CREATE TABLE team_member (
    team_id       TEXT NOT NULL REFERENCES team(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,                  -- unique in the team; "lead" for the lead
    session_id    TEXT NOT NULL,
    role          TEXT NOT NULL,                  -- lead | teammate
    agent         TEXT NOT NULL,
    model         TEXT,
    isolation     TEXT NOT NULL,                  -- shared | worktree
    worktree_id   TEXT,
    branch        TEXT,
    owns          TEXT,                           -- JSON array of globs
    status        TEXT NOT NULL,                  -- working | idle | waiting_input | stopped
    last_status   TEXT,                           -- last status line, <= 200 chars
    PRIMARY KEY (team_id, name),
    UNIQUE (session_id)
);
CREATE TABLE team_task (
    id            TEXT PRIMARY KEY,               -- task_<ulid>
    team_id       TEXT NOT NULL REFERENCES team(id) ON DELETE CASCADE,
    seq           INTEGER NOT NULL,               -- display order in the team
    title         TEXT NOT NULL,
    description   TEXT NOT NULL DEFAULT '',
    status        TEXT NOT NULL,                  -- todo | in_progress | blocked | done | cancelled
    owner         TEXT,                           -- member name
    depends_on    TEXT NOT NULL DEFAULT '[]',     -- JSON array of task ids in the same team
    created_by    TEXT NOT NULL,
    created_ms    INTEGER NOT NULL,
    updated_ms    INTEGER NOT NULL,
    version       INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX team_task_board ON team_task (team_id, status, seq);
