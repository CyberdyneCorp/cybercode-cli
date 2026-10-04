-- Append-only event store (storage-events → Durable event store).
-- Sequence numbers start at 0 per aggregate; event_sequence holds the latest seq.
CREATE TABLE event (
    id           TEXT PRIMARY KEY,
    aggregate_id TEXT    NOT NULL,
    seq          INTEGER NOT NULL,
    type         TEXT    NOT NULL,
    data         TEXT    NOT NULL,
    time         INTEGER NOT NULL,
    causation_id TEXT,
    UNIQUE (aggregate_id, seq)
) STRICT;

CREATE TABLE event_sequence (
    aggregate_id TEXT PRIMARY KEY,
    seq          INTEGER NOT NULL
) STRICT;

CREATE TABLE data_migration (
    id             TEXT PRIMARY KEY,
    time_completed INTEGER NOT NULL
) STRICT;
