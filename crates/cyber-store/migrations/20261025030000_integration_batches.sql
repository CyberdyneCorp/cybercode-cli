CREATE TABLE integration_batch (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    directory TEXT NOT NULL,
    status TEXT NOT NULL,
    revision INTEGER NOT NULL,
    created_ms INTEGER NOT NULL,
    data TEXT NOT NULL
) STRICT;
CREATE INDEX integration_batch_location ON integration_batch(directory, created_ms DESC);
CREATE INDEX workflow_integration_batch ON workflow_run(json_extract(data, '$.args.integration_admission.batch_id')) WHERE name='integrate';
