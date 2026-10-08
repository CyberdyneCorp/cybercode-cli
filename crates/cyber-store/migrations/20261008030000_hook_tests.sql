-- Synthetic identities are not Session foreign keys and never enter Session history.
CREATE TABLE hook_test_execution (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('running', 'completed', 'unknown')),
    started_ms INTEGER NOT NULL,
    data TEXT NOT NULL
) STRICT;
CREATE INDEX hook_test_invocation ON hook_test_execution(session_id, started_ms DESC, id DESC);
CREATE UNIQUE INDEX hook_test_once ON hook_test_execution(session_id, json_extract(data, '$.digest'))
WHERE json_extract(data, '$.once') = 1;
