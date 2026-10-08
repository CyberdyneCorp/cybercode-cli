-- Retain class evidence when a billed descendant's conversation is removed.
ALTER TABLE session_children_charge ADD COLUMN token_classes TEXT;
ALTER TABLE session ADD COLUMN children_token_classes TEXT NOT NULL DEFAULT '{"input":0,"output":0,"reasoning":0,"cache_read":0,"cache_write":0}';
ALTER TABLE session ADD COLUMN children_token_classes_complete INTEGER NOT NULL DEFAULT 1;

UPDATE session_children_charge SET token_classes=(
    SELECT json_object(
        'input',COALESCE(json_extract(data,'$.usage.input'),0),
        'output',COALESCE(json_extract(data,'$.usage.output'),0),
        'reasoning',COALESCE(json_extract(data,'$.usage.reasoning'),0),
        'cache_read',COALESCE(json_extract(data,'$.usage.cache_read'),0),
        'cache_write',COALESCE(json_extract(data,'$.usage.cache_write'),0)
    ) FROM event WHERE id=event_id AND json_type(data,'$.usage')='object'
      AND json_type(data,'$.usage.input')='integer' AND json_extract(data,'$.usage.input')>=0
      AND json_type(data,'$.usage.output')='integer' AND json_extract(data,'$.usage.output')>=0
      AND json_type(data,'$.usage.reasoning')='integer' AND json_extract(data,'$.usage.reasoning')>=0
      AND json_type(data,'$.usage.cache_read')='integer' AND json_extract(data,'$.usage.cache_read')>=0
      AND json_type(data,'$.usage.cache_write')='integer' AND json_extract(data,'$.usage.cache_write')>=0
);

UPDATE session SET children_token_classes=(
    SELECT json_object(
        'input',COALESCE(SUM(json_extract(token_classes,'$.input')),0),
        'output',COALESCE(SUM(json_extract(token_classes,'$.output')),0),
        'reasoning',COALESCE(SUM(json_extract(token_classes,'$.reasoning')),0),
        'cache_read',COALESCE(SUM(json_extract(token_classes,'$.cache_read')),0),
        'cache_write',COALESCE(SUM(json_extract(token_classes,'$.cache_write')),0)
    ) FROM session_children_charge WHERE parent_id=session.id
), children_token_classes_complete=CASE WHEN children_usage_complete=1
    AND NOT EXISTS(SELECT 1 FROM session_children_charge WHERE parent_id=session.id AND token_classes IS NULL)
    THEN 1 ELSE 0 END;
