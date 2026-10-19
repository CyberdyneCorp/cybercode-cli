-- Messages between direct peers (cross-session-messaging → Direct peer transport). A
-- record of a message to or from another machine names that peer (`remote_peer`: the
-- remote's name on the sender, the grant's name on the target) and the Session on the
-- other machine (`remote_session_id`), which `peer_session_id` cannot hold because it
-- references only local Sessions. A sender's `out` record whose send failed on the
-- network has outcome `failed`; a retry with the same ID replaces it.
ALTER TABLE xsession_message ADD COLUMN remote_peer TEXT;
ALTER TABLE xsession_message ADD COLUMN remote_session_id TEXT;
CREATE INDEX xsession_message_remote ON xsession_message (message_id, remote_peer)
    WHERE remote_peer IS NOT NULL;
