DROP INDEX job_name;
CREATE UNIQUE INDEX job_name ON job(session_id, name) WHERE status = 'running';
ALTER TABLE job ADD COLUMN usage_baseline TEXT NOT NULL DEFAULT '{}';
