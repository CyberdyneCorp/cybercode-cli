-- Workflow replay, lifecycle and run budgets (`workflows` → Durable agent results and resume,
-- Replay of workflow host calls, Run budgets).
ALTER TABLE session ADD COLUMN workflow_run_id TEXT;
UPDATE session SET workflow_run_id = (
    SELECT json_extract(e.data, '$.info.workflow_run_id') FROM event e
    WHERE e.aggregate_id = session.id AND e.type = 'session.created.1'
);
CREATE INDEX session_workflow_run ON session(workflow_run_id) WHERE workflow_run_id IS NOT NULL;

-- One row per script revision; the source lets a relaunch re-execute inline scripts.
CREATE TABLE workflow_run_revision (
    run_id TEXT NOT NULL REFERENCES workflow_run(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL,
    digest TEXT NOT NULL,
    config_digest TEXT NOT NULL,
    source TEXT NOT NULL,
    PRIMARY KEY (run_id, revision)
) STRICT;

-- Host calls in issue order per revision: agents once started, checkpoints and reused
-- results once settled. `settled_order` is the order results reached the script.
CREATE TABLE workflow_call (
    run_id TEXT NOT NULL REFERENCES workflow_run(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    call_key TEXT NOT NULL,
    agent_id TEXT,
    status TEXT NOT NULL,
    settled_order INTEGER,
    outcome TEXT,
    PRIMARY KEY (run_id, revision, seq)
) STRICT;
CREATE INDEX workflow_call_key ON workflow_call(run_id, call_key);

-- A run budget joins the budget scopes of every Session that carries its run ID.
-- `activated_ms` is the start of the current running segment; `active_ms` accumulates
-- earlier segments, so paused time is not charged to max_wall_seconds.
CREATE TABLE workflow_run_budget (
    run_id TEXT PRIMARY KEY REFERENCES workflow_run(id) ON DELETE CASCADE,
    spec TEXT NOT NULL,
    activated_ms INTEGER,
    active_ms INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE TABLE workflow_run_budget_signal (
    scope_id TEXT NOT NULL REFERENCES workflow_run(id) ON DELETE CASCADE,
    limit_name TEXT NOT NULL,
    level TEXT NOT NULL,
    PRIMARY KEY (scope_id, limit_name, level)
) STRICT;
