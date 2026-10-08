CREATE TABLE hook_execution (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK(status IN ('running', 'completed', 'unknown')),
    started_ms INTEGER NOT NULL,
    data TEXT NOT NULL
) STRICT;
CREATE INDEX hook_execution_session ON hook_execution(session_id, started_ms DESC, id DESC);
