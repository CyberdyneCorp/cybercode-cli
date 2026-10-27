-- Immutable inherited ceilings outlive parent transcripts and completed Goal/Loop state.
-- Absence of an owner/evidence is a refusal, never a reason to omit a captured scope.
CREATE TABLE budget_scope_binding (
    session_id TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('session','run','goal','loop','team','schedule')),
    owner_id TEXT NOT NULL,
    spec TEXT NOT NULL,
    evidence TEXT NOT NULL,
    created_ms INTEGER NOT NULL,
    PRIMARY KEY(session_id,scope_id)
) STRICT;
CREATE INDEX budget_scope_binding_scope ON budget_scope_binding(scope_id,session_id);
