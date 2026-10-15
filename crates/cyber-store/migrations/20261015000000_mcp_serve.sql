-- MCP serve records (`mcp-server`): attribution of Sessions and inputs created through
-- MCP serve, and the tool-level argument fingerprints of its idempotency keys.
CREATE TABLE mcp_serve_session (
    session_id TEXT PRIMARY KEY,
    mcp_client TEXT NOT NULL,
    created_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE mcp_serve_input (
    message_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    mcp_client TEXT NOT NULL,
    depth INTEGER NOT NULL,
    admitted_ms INTEGER NOT NULL,
    idempotency_key TEXT
) STRICT;
CREATE INDEX mcp_serve_input_session ON mcp_serve_input (session_id, admitted_ms);
CREATE TABLE mcp_serve_key (
    key TEXT PRIMARY KEY,
    fingerprint TEXT NOT NULL,
    created_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX mcp_serve_key_created ON mcp_serve_key (created_ms);
