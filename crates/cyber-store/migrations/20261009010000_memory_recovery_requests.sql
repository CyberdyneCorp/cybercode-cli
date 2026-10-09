CREATE TABLE memory_recovery_request (
    key_hash TEXT PRIMARY KEY,
    request_hash TEXT NOT NULL,
    mutation_id TEXT NOT NULL REFERENCES memory_mutation(id)
) STRICT;
