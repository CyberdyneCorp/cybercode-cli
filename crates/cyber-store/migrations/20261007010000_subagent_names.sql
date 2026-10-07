ALTER TABLE session ADD COLUMN subagent_name TEXT;
CREATE UNIQUE INDEX session_subagent_name ON session(parent_id, subagent_name) WHERE subagent_name IS NOT NULL;
