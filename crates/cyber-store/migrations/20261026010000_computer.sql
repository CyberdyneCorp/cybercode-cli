-- Native inputs are durable intents; a missing settlement is never replay authority.
CREATE TABLE computer_operation (
    operation_key TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    action TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    audit TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('intent','posted','captured','unknown','unknown_acknowledged','cancelled')),
    started_ms INTEGER NOT NULL,
    settled_ms INTEGER,
    receipt TEXT
);
CREATE INDEX computer_operation_session ON computer_operation(session_id,started_ms);
CREATE TABLE computer_frame (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    metadata TEXT NOT NULL,
    native_target TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    created_ms INTEGER NOT NULL,
    expires_ms INTEGER,
    ready INTEGER NOT NULL DEFAULT 0,
    expired INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX computer_frame_session ON computer_frame(session_id,created_ms);
