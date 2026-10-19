-- Cross-session messaging (cross-session-messaging → Transcript and audit). Both tables are
-- projections of the durable `messaging.*` events in the Session streams, written by the
-- projector in the event transaction, so they can be rebuilt from the events.

-- One row per Session side of a message: `in` in the target, `out` in the sender.
CREATE TABLE xsession_message (
    session_id      TEXT    NOT NULL REFERENCES session (id) ON DELETE CASCADE,
    message_id      TEXT    NOT NULL,
    direction       TEXT    NOT NULL,
    -- The other side's address as shown.
    peer_address    TEXT    NOT NULL,
    -- The other side's local Session, if any.
    peer_session_id TEXT,
    -- `session` | `client` today; an open set (peers add `peer`).
    origin_kind     TEXT    NOT NULL,
    -- The rate-limit key of the sender.
    sender_key      TEXT    NOT NULL,
    -- delivered | held | refused | released | dropped
    outcome         TEXT    NOT NULL,
    reason          TEXT,
    -- UTF-8 bytes of the text.
    size            INTEGER NOT NULL,
    sha256          TEXT    NOT NULL,
    -- NULL when refused: a refused message stores no text anywhere.
    text            TEXT,
    depth           INTEGER NOT NULL DEFAULT 0,
    hops            INTEGER NOT NULL DEFAULT 1,
    created_ms      INTEGER NOT NULL,
    updated_ms      INTEGER NOT NULL,
    -- The event seq that last changed the row.
    seq             INTEGER NOT NULL,
    PRIMARY KEY (session_id, message_id, direction)
) STRICT;
CREATE INDEX xsession_message_rate ON xsession_message (session_id, direction, sender_key, created_ms);
CREATE INDEX xsession_message_history ON xsession_message (session_id, seq);
-- History pages (`GET /sessions/:id/messaging`) walk this key, which never changes.
CREATE INDEX xsession_message_created ON xsession_message (session_id, created_ms, message_id, direction);

-- Idle notification subscriptions, owned by the subscriber Session.
CREATE TABLE messaging_watch (
    id              TEXT    PRIMARY KEY,
    subscriber_id   TEXT    NOT NULL REFERENCES session (id) ON DELETE CASCADE,
    target_address  TEXT    NOT NULL,
    target_id       TEXT    NOT NULL,
    -- active | fired | expired
    status          TEXT    NOT NULL,
    created_ms      INTEGER NOT NULL,
    expires_ms      INTEGER NOT NULL,
    fired_ms        INTEGER,
    -- `msgx_watch_<id>` once the notice is admitted.
    notice_id       TEXT
) STRICT;
CREATE INDEX messaging_watch_target ON messaging_watch (target_id, status);
CREATE INDEX messaging_watch_expiry ON messaging_watch (status, expires_ms);
