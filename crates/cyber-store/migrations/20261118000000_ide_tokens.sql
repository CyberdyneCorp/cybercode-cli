-- Local IDE tokens (`server-api` → Local IDE token issuance): a scoped, revocable
-- credential for editor clients. Only the SHA-256 of each token is stored; the token is
-- returned once by `POST /api/v1/ide/tokens`.
CREATE TABLE ide_token (
    -- `itk_<ulid>`.
    id            TEXT    PRIMARY KEY,
    -- The editor client; today only `vscode`.
    client        TEXT    NOT NULL,
    name          TEXT,
    -- Hex SHA-256 of the whole `cyi_…` token.
    sha256        TEXT    NOT NULL UNIQUE,
    created_ms    INTEGER NOT NULL,
    expires_ms    INTEGER NOT NULL,
    last_used_ms  INTEGER,
    revoked_ms    INTEGER
) STRICT;

-- One active (unrevoked) token per client; issuing another revokes the previous one in
-- the same transaction.
CREATE UNIQUE INDEX ide_token_active_client ON ide_token (client) WHERE revoked_ms IS NULL;
