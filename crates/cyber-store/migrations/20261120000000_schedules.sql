-- Persistent local schedules (`loops-scheduling` → Persistent local schedules): the
-- projection of the `schedule.*` events of each schedule's own stream, read by the
-- service's sweep and the `cyber loops` routes. Rebuildable from events.
CREATE TABLE schedule (
    id          TEXT PRIMARY KEY,
    name        TEXT    NOT NULL,
    cron        TEXT    NOT NULL,
    directory   TEXT    NOT NULL,
    spec        TEXT    NOT NULL,
    status      TEXT    NOT NULL,
    reason      TEXT,
    next_due_ms INTEGER,
    inflight    TEXT,
    runs        INTEGER NOT NULL DEFAULT 0,
    cost_usd    REAL    NOT NULL DEFAULT 0,
    tokens      INTEGER NOT NULL DEFAULT 0,
    turns       INTEGER NOT NULL DEFAULT 0,
    created_ms  INTEGER NOT NULL,
    updated_ms  INTEGER NOT NULL
) STRICT;
CREATE INDEX schedule_due ON schedule (status, next_due_ms);

CREATE TABLE schedule_run (
    id            TEXT PRIMARY KEY,
    schedule_id   TEXT    NOT NULL REFERENCES schedule (id) ON DELETE CASCADE,
    n             INTEGER NOT NULL,
    due_ms        INTEGER NOT NULL,
    started_ms    INTEGER,
    ended_ms      INTEGER,
    session_id    TEXT,
    message_id    TEXT,
    outcome       TEXT    NOT NULL,
    noop          INTEGER NOT NULL DEFAULT 0,
    reason        TEXT,
    cost_usd      REAL    NOT NULL DEFAULT 0,
    tokens        INTEGER NOT NULL DEFAULT 0,
    turns         INTEGER NOT NULL DEFAULT 0,
    cost_before   REAL    NOT NULL DEFAULT 0,
    tokens_before INTEGER NOT NULL DEFAULT 0,
    turns_before  INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE INDEX schedule_run_schedule ON schedule_run (schedule_id, n);
CREATE INDEX schedule_run_session ON schedule_run (session_id, outcome);
