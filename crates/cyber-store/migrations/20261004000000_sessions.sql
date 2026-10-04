-- Session projections (storage-events → Core tables). The event log is authoritative;
-- these rows are maintained by projectors in the same transaction as the events.
CREATE TABLE session (
    id              TEXT PRIMARY KEY,
    title           TEXT    NOT NULL,
    directory       TEXT    NOT NULL,
    parent_id       TEXT,
    agent           TEXT    NOT NULL,
    model           TEXT    NOT NULL,
    mode            TEXT    NOT NULL,
    archived        INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    last_seq        INTEGER NOT NULL,
    cost            REAL    NOT NULL DEFAULT 0,
    unpriced_steps  INTEGER NOT NULL DEFAULT 0,
    input_tokens    INTEGER NOT NULL DEFAULT 0,
    output_tokens   INTEGER NOT NULL DEFAULT 0,
    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE INDEX session_directory_updated ON session (directory, updated_at DESC);
CREATE INDEX session_parent ON session (parent_id);

CREATE TABLE session_input (
    message_id   TEXT PRIMARY KEY,
    session_id   TEXT    NOT NULL REFERENCES session (id) ON DELETE CASCADE,
    delivery     TEXT    NOT NULL,
    status       TEXT    NOT NULL,
    source       TEXT    NOT NULL,
    digest       TEXT    NOT NULL,
    admitted_seq INTEGER NOT NULL,
    promoted_seq INTEGER,
    held_reason  TEXT
) STRICT;
CREATE INDEX session_input_session ON session_input (session_id, admitted_seq);

CREATE TABLE tool_call (
    session_id    TEXT    NOT NULL REFERENCES session (id) ON DELETE CASCADE,
    call_id       TEXT    NOT NULL,
    message_id    TEXT    NOT NULL,
    name          TEXT    NOT NULL,
    status        TEXT    NOT NULL,
    retry_safety  TEXT    NOT NULL,
    attempt       INTEGER NOT NULL DEFAULT 0,
    input_digest  TEXT,
    PRIMARY KEY (session_id, call_id)
) STRICT;
