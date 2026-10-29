-- A user acknowledgement is consumed across all observation Locations. Keep
-- that single-writer proof indexed rather than scanning an unbounded journal.
CREATE INDEX event_file_observation_renewal
ON event(json_extract(data, '$.renewal.event_id'))
WHERE type = 'file.watch.changed.1'
  AND json_type(data, '$.renewal.event_id') = 'text';
