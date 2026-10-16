-- Usage step ledger (observability-costs → Usage step ledger): one row per provider step
-- (finished or failed) and per auxiliary model call, written by the projector in the
-- event transaction. Rows outlive Session deletion, like `session_children_charge`.
CREATE TABLE usage_step (
    event_id           TEXT PRIMARY KEY,
    session_id         TEXT    NOT NULL,
    root_id            TEXT    NOT NULL,
    parent_id          TEXT,
    workflow_run_id    TEXT,
    agent              TEXT    NOT NULL,
    -- `turn` for provider steps, else the auxiliary purpose (`title`, `compaction`, ...).
    purpose            TEXT    NOT NULL,
    failed             INTEGER NOT NULL DEFAULT 0,
    provider           TEXT    NOT NULL,
    model              TEXT    NOT NULL,
    started_ms         INTEGER NOT NULL,
    ended_ms           INTEGER NOT NULL,
    input_tokens       INTEGER NOT NULL,
    output_tokens      INTEGER NOT NULL,
    reasoning_tokens   INTEGER NOT NULL,
    cache_read_tokens  INTEGER NOT NULL,
    cache_write_tokens INTEGER NOT NULL,
    -- input + cache read + cache write.
    prompt_tokens      INTEGER NOT NULL,
    -- NULL when the call was unpriced.
    cost               REAL,
    directory          TEXT    NOT NULL
) STRICT;
CREATE INDEX usage_step_root ON usage_step (root_id);
CREATE INDEX usage_step_session ON usage_step (session_id);
CREATE INDEX usage_step_parent ON usage_step (parent_id);
CREATE INDEX usage_step_ended ON usage_step (ended_ms);

-- Rows older than `telemetry.usage_retention_days`, folded per UTC day.
CREATE TABLE usage_daily (
    day                TEXT    NOT NULL,
    root_id            TEXT    NOT NULL,
    agent              TEXT    NOT NULL,
    provider           TEXT    NOT NULL,
    model              TEXT    NOT NULL,
    directory          TEXT    NOT NULL,
    steps              INTEGER NOT NULL,
    turns              INTEGER NOT NULL,
    input_tokens       INTEGER NOT NULL,
    output_tokens      INTEGER NOT NULL,
    reasoning_tokens   INTEGER NOT NULL,
    cache_read_tokens  INTEGER NOT NULL,
    cache_write_tokens INTEGER NOT NULL,
    cost               REAL    NOT NULL,
    unpriced_steps     INTEGER NOT NULL,
    max_prompt_tokens  INTEGER NOT NULL,
    api_duration_ms    INTEGER NOT NULL,
    PRIMARY KEY (day, root_id, agent, provider, model)
) STRICT;

-- The resumable backfill: events up to `target` (by rowid) are projected in batches,
-- after `cursor`. The row is removed when the backfill finishes.
CREATE TABLE usage_backfill (
    id     INTEGER PRIMARY KEY CHECK (id = 1),
    cursor INTEGER NOT NULL,
    target INTEGER NOT NULL
) STRICT;
INSERT INTO usage_backfill (id, cursor, target)
SELECT 1, 0, MAX(rowid) FROM event HAVING COUNT(*) > 0;
