CREATE UNIQUE INDEX hook_execution_once ON hook_execution(session_id, json_extract(data, '$.digest'))
WHERE json_extract(data, '$.once') = 1;
