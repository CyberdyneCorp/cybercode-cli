-- Scoped counters for decisions recorded after checkout-scope capture was introduced.
CREATE TABLE permission_auto_statistics (
    checkout_root TEXT PRIMARY KEY,
    allowed INTEGER NOT NULL DEFAULT 0 CHECK(allowed >= 0),
    blocked INTEGER NOT NULL DEFAULT 0 CHECK(blocked >= 0),
    fallback INTEGER NOT NULL DEFAULT 0 CHECK(fallback >= 0),
    classifier INTEGER NOT NULL DEFAULT 0 CHECK(classifier >= 0),
    policy INTEGER NOT NULL DEFAULT 0 CHECK(policy >= 0),
    recorded_since INTEGER NOT NULL,
    reset_at INTEGER
) STRICT;
