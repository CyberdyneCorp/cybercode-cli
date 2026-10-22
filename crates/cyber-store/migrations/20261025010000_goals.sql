-- Session Goals (`goals`): the index of the `session.goal.*` events that restore and
-- listings read without loading every Session. Rebuildable from events.
CREATE TABLE goal (
    goal_id    TEXT PRIMARY KEY,
    session_id TEXT    NOT NULL REFERENCES session (id) ON DELETE CASCADE,
    status     TEXT    NOT NULL,
    condition  TEXT    NOT NULL,
    created_ms INTEGER NOT NULL,
    ended_ms   INTEGER
);
CREATE INDEX goal_session ON goal (session_id, status);
CREATE INDEX goal_status ON goal (status);
