-- Usage attribution (cross-session-messaging → Usage attribution): the origin of each
-- ledger row (`user`, `session`, `client`, `job`, `mcp`, or the admission source) and the
-- MCP client of `mcp` rows. Rows recorded before origins existed read as `user`.
ALTER TABLE usage_step ADD COLUMN origin TEXT NOT NULL DEFAULT 'user';
ALTER TABLE usage_step ADD COLUMN mcp_client TEXT;
CREATE INDEX usage_step_origin ON usage_step (origin);

-- Daily aggregates keep the origin in their key, so folded days still split by origin.
CREATE TABLE usage_daily_origin (
    day                TEXT    NOT NULL,
    root_id            TEXT    NOT NULL,
    agent              TEXT    NOT NULL,
    provider           TEXT    NOT NULL,
    model              TEXT    NOT NULL,
    origin             TEXT    NOT NULL,
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
    PRIMARY KEY (day, root_id, agent, provider, model, origin)
) STRICT;
INSERT INTO usage_daily_origin (day, root_id, agent, provider, model, origin, directory, steps,
    turns, input_tokens, output_tokens, reasoning_tokens, cache_read_tokens, cache_write_tokens,
    cost, unpriced_steps, max_prompt_tokens, api_duration_ms)
SELECT day, root_id, agent, provider, model, 'user', directory, steps, turns, input_tokens,
    output_tokens, reasoning_tokens, cache_read_tokens, cache_write_tokens, cost, unpriced_steps,
    max_prompt_tokens, api_duration_ms
FROM usage_daily;
DROP TABLE usage_daily;
ALTER TABLE usage_daily_origin RENAME TO usage_daily;
