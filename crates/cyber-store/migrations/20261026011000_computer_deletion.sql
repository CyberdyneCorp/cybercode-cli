-- A deleted Session can never reserve another private capture, including a late
-- blocking worker whose caller finished before Session deletion.
CREATE TABLE computer_deleted_session (
    session_id TEXT PRIMARY KEY,
    deleted_ms INTEGER NOT NULL
);
INSERT INTO computer_deleted_session(session_id,deleted_ms)
SELECT DISTINCT frame.session_id,CAST(strftime('%s','now') AS INTEGER)*1000
FROM computer_frame AS frame
WHERE NOT EXISTS(SELECT 1 FROM session WHERE session.id=frame.session_id);
UPDATE computer_frame SET expired=1
WHERE session_id IN (SELECT session_id FROM computer_deleted_session);
