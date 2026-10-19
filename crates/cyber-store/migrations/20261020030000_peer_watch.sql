-- Watches on Sessions of a configured peer (cross-session-messaging → Peer watches and
-- listing). A watch on `X@P` keeps the peer's name; its `target_id` is `<ses_>@P`, which
-- no Session of this server has, so local idle checks never see it. The remote watch
-- poller asks `GET /api/v1/peer/sessions/:id/status` for these rows every 15 s.
ALTER TABLE messaging_watch ADD COLUMN target_peer TEXT;
CREATE INDEX messaging_watch_peer ON messaging_watch (target_peer)
    WHERE target_peer IS NOT NULL;
