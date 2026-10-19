-- Peer grants (`cross-session-messaging` → Peer pairing and grants): one row per peer
-- that may call this server's peer listener. Only the SHA-256 of each token's secret is
-- stored; the token itself is printed once by `cyber peers invite`.
CREATE TABLE peer_grant (
    -- `pgr_<ulid>`; the token is `cyp_pgr<ulid>_<secret>`.
    id            TEXT    PRIMARY KEY,
    -- The peer's name on this machine; senders are named after it (`ses_…@laptop`).
    name          TEXT    NOT NULL UNIQUE,
    -- Hex SHA-256 of the 32-byte secret.
    token_sha256  TEXT    NOT NULL,
    -- JSON array of scopes: `["message"]` or `["message","attach"]`.
    scopes        TEXT    NOT NULL,
    created_ms    INTEGER NOT NULL,
    -- NULL: no expiry.
    expires_ms    INTEGER,
    last_used_ms  INTEGER,
    revoked_ms    INTEGER
) STRICT;
