CREATE TABLE job (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
    child_id TEXT,
    name TEXT NOT NULL,
    status TEXT NOT NULL,
    data TEXT NOT NULL
) STRICT;
CREATE INDEX job_session ON job(session_id, id);
CREATE UNIQUE INDEX job_name ON job(session_id, name);
