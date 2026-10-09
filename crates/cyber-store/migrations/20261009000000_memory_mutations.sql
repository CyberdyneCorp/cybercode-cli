CREATE TABLE memory_mutation (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    owner_hash TEXT NOT NULL,
    data TEXT NOT NULL,
    result TEXT
) STRICT;
CREATE UNIQUE INDEX memory_mutation_pending
    ON memory_mutation(project_id) WHERE result IS NULL;
