-- Usage limit waits (`session-runtime` → Usage limit wait recovery): the pending wait of
-- each Session, which startup recovery reads without loading every Session.
-- Rebuildable from the `session.usage_limit.*` events.
CREATE TABLE session_usage_wait (
    session_id   TEXT PRIMARY KEY REFERENCES session (id) ON DELETE CASCADE,
    message_id   TEXT    NOT NULL,
    resume_at_ms INTEGER NOT NULL
) STRICT;
