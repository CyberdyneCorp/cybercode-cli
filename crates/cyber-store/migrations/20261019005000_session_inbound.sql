-- The Session-level `messaging.inbound` override (cross-session-messaging → Inbound controls,
-- Non-interactive sessions), projected from `session.created.1` and
-- `session.messaging.updated.1`. NULL is no override: the Location's default applies.
ALTER TABLE session ADD COLUMN inbound TEXT;
-- Why the override refuses (`non_interactive`), shown to refused senders.
ALTER TABLE session ADD COLUMN inbound_reason TEXT;
