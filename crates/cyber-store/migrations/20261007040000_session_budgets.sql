CREATE TABLE session_budget (
    session_id TEXT PRIMARY KEY REFERENCES session(id) ON DELETE CASCADE,
    spec TEXT NOT NULL,
    activated_ms INTEGER
) STRICT;
CREATE TABLE session_budget_signal (
    scope_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
    limit_name TEXT NOT NULL,
    level TEXT NOT NULL,
    PRIMARY KEY(scope_id, limit_name, level)
) STRICT;
ALTER TABLE session_children_charge ADD COLUMN turns INTEGER NOT NULL DEFAULT 0;
UPDATE session_children_charge SET turns=1
WHERE event_id IN (SELECT id FROM event WHERE type='session.step.ended.1');
-- Receipts from purged pre-migration children lack evidence for completed Turn counts.
UPDATE session SET children_usage_complete=0 WHERE id IN (
    SELECT parent_id FROM session_children_charge c
    WHERE NOT EXISTS (SELECT 1 FROM event e WHERE e.id=c.event_id)
);
