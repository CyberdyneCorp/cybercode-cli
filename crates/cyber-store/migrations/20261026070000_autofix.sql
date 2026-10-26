CREATE TABLE autofix_subscription (
    id TEXT PRIMARY KEY,
    directory TEXT NOT NULL,
    repository_id INTEGER NOT NULL,
    pr INTEGER NOT NULL,
    status TEXT NOT NULL,
    revision INTEGER NOT NULL,
    data TEXT NOT NULL
) STRICT;
CREATE INDEX autofix_subscription_location ON autofix_subscription(directory);
CREATE UNIQUE INDEX autofix_subscription_active ON autofix_subscription(directory,repository_id,pr) WHERE status NOT IN ('disabled','closed');

-- A host-owned immutable capability marker exists before any attempt Session can run.
CREATE TABLE autofix_session (
    session_id TEXT PRIMARY KEY,
    subscription_id TEXT NOT NULL REFERENCES autofix_subscription(id),
    attempt_id TEXT NOT NULL,
    directory TEXT NOT NULL
) STRICT;
