-- Billing receipts retain charges on surviving ancestors after child history deletion.
ALTER TABLE session ADD COLUMN children_cost REAL NOT NULL DEFAULT 0;
ALTER TABLE session ADD COLUMN children_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE session ADD COLUMN children_unpriced_steps INTEGER NOT NULL DEFAULT 0;
-- Historical deletion was not journaled; old Sessions cannot prove complete child billing.
ALTER TABLE session ADD COLUMN children_usage_complete INTEGER NOT NULL DEFAULT 0;
CREATE TABLE session_children_charge (
    parent_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    cost REAL NOT NULL,
    tokens INTEGER NOT NULL,
    unpriced INTEGER NOT NULL,
    PRIMARY KEY(parent_id, event_id)
) STRICT;

-- Include hidden calls, matching runtime lifetime totals rather than only visible steps.
CREATE TEMP TABLE cyber_usage_backfill AS
SELECT id AS event_id, aggregate_id AS source_id,
       COALESCE(json_extract(data,'$.cost'),0) AS cost,
       CASE WHEN json_extract(data,'$.cost') IS NULL THEN 1 ELSE 0 END AS unpriced,
       COALESCE(json_extract(data,'$.usage.input'),0) AS input,
       COALESCE(json_extract(data,'$.usage.output'),0) AS output,
       COALESCE(json_extract(data,'$.usage.reasoning'),0) AS reasoning,
       COALESCE(json_extract(data,'$.usage.cache_read'),0) AS cache_read,
       COALESCE(json_extract(data,'$.usage.cache_write'),0) AS cache_write
FROM event
WHERE type IN ('session.step.ended.1','session.compaction.completed.1')
   OR (type IN ('session.title.generated.1','permission.auto_decided.1')
       AND json_type(data,'$.usage')='object');

WITH RECURSIVE ancestors(parent_id,source_id) AS (
    SELECT parent_id,id FROM session WHERE parent_id IS NOT NULL AND parent_id!=id
    UNION
    SELECT s.parent_id,a.source_id FROM ancestors a JOIN session s ON s.id=a.parent_id
    WHERE s.parent_id IS NOT NULL AND s.parent_id!=a.source_id
)
INSERT INTO session_children_charge(parent_id,source_id,event_id,cost,tokens,unpriced)
SELECT a.parent_id,u.source_id,u.event_id,u.cost,
       u.input+u.output+u.reasoning+u.cache_read+u.cache_write,u.unpriced
FROM ancestors a JOIN cyber_usage_backfill u ON u.source_id=a.source_id
JOIN session p ON p.id=a.parent_id;

UPDATE session SET
    cost=COALESCE((SELECT SUM(cost) FROM cyber_usage_backfill WHERE source_id=session.id),0),
    unpriced_steps=COALESCE((SELECT SUM(unpriced) FROM cyber_usage_backfill WHERE source_id=session.id),0),
    input_tokens=COALESCE((SELECT SUM(input) FROM cyber_usage_backfill WHERE source_id=session.id),0),
    output_tokens=COALESCE((SELECT SUM(output) FROM cyber_usage_backfill WHERE source_id=session.id),0),
    reasoning_tokens=COALESCE((SELECT SUM(reasoning) FROM cyber_usage_backfill WHERE source_id=session.id),0),
    cache_read_tokens=COALESCE((SELECT SUM(cache_read) FROM cyber_usage_backfill WHERE source_id=session.id),0),
    cache_write_tokens=COALESCE((SELECT SUM(cache_write) FROM cyber_usage_backfill WHERE source_id=session.id),0),
    children_cost=COALESCE((SELECT SUM(cost) FROM session_children_charge WHERE parent_id=session.id),0),
    children_tokens=COALESCE((SELECT SUM(tokens) FROM session_children_charge WHERE parent_id=session.id),0),
    children_unpriced_steps=COALESCE((SELECT SUM(unpriced) FROM session_children_charge WHERE parent_id=session.id),0);
DROP TABLE cyber_usage_backfill;
