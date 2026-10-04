-- Idempotency keys for non-GET HTTP routes (`server-api` → Idempotency keys).
CREATE TABLE idempotency_key (
    principal TEXT NOT NULL,
    key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    status INTEGER NOT NULL,
    body BLOB NOT NULL,
    content_type TEXT NOT NULL,
    created_ms INTEGER NOT NULL,
    PRIMARY KEY (principal, key)
);
CREATE INDEX idempotency_key_created ON idempotency_key (created_ms);
