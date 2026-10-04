-- Saved approvals per checkout (permissions-modes → Persisted approvals).
CREATE TABLE permission_saved (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    checkout_root  TEXT    NOT NULL,
    action         TEXT    NOT NULL,
    resource       TEXT    NOT NULL,
    created_at     INTEGER NOT NULL,
    source_session TEXT    NOT NULL,
    UNIQUE (checkout_root, action, resource)
) STRICT;

-- The Session task list (builtin-tools → todo tool), persisted with the Session.
CREATE TABLE session_todo (
    session_id TEXT PRIMARY KEY,
    items      TEXT NOT NULL
) STRICT;
