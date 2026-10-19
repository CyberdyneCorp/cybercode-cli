-- Session-scoped loops (`loops-scheduling`): the due-time index of the `loop.*` events
-- the background sweep reads without loading every Session. Rebuildable from events.
CREATE TABLE session_loop (
    loop_id     TEXT PRIMARY KEY,
    session_id  TEXT    NOT NULL REFERENCES session (id) ON DELETE CASCADE,
    status      TEXT    NOT NULL,
    next_due_ms INTEGER,
    inflight    INTEGER NOT NULL DEFAULT 0,
    created_ms  INTEGER NOT NULL,
    ended_ms    INTEGER
);
CREATE INDEX session_loop_due ON session_loop (status, next_due_ms);
CREATE INDEX session_loop_session ON session_loop (session_id, status);
