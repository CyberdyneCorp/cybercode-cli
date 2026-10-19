-- Outcome tracking of messages held on a peer (cross-session-messaging → Peer outcome
-- tracking). The sender's `out` record of a message a peer holds carries the time of its
-- next poll of `GET /api/v1/peer/messages/:id`; the outcome tracker polls rows whose time
-- has come, so polling resumes after a restart. The projection sets it when such a record
-- is written and clears it when the outcome changes (released, dropped, expired).
ALTER TABLE xsession_message ADD COLUMN next_poll_ms INTEGER;
CREATE INDEX xsession_message_poll ON xsession_message (next_poll_ms)
    WHERE next_poll_ms IS NOT NULL;
