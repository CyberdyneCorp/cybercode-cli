CREATE TABLE mcp_connection (
    id TEXT PRIMARY KEY,
    directory TEXT NOT NULL,
    name TEXT NOT NULL,
    phase TEXT NOT NULL,
    owner_hash TEXT NOT NULL,
    data TEXT NOT NULL
) STRICT;
CREATE UNIQUE INDEX mcp_connection_unsettled
    ON mcp_connection(directory, name) WHERE phase != 'settled';
