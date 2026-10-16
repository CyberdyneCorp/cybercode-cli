CREATE TABLE workflow_run (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    name TEXT NOT NULL,
    status TEXT NOT NULL,
    started_ms INTEGER NOT NULL,
    data TEXT NOT NULL
) STRICT;
CREATE INDEX workflow_run_session ON workflow_run(session_id, id);
CREATE TABLE workflow_agent (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES workflow_run(id) ON DELETE CASCADE,
    call_key TEXT NOT NULL,
    session_id TEXT,
    status TEXT NOT NULL,
    settled_seq INTEGER NOT NULL,
    data TEXT NOT NULL
) STRICT;
CREATE INDEX workflow_agent_run ON workflow_agent(run_id, settled_seq);
