-- Authoritative provider-dispatch allocations. Deliberately independent of Session/event
-- retention: deleting a transcript must not free an uncertain allocation or its audit.
CREATE TABLE budget_reservation (
    request_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    purpose TEXT NOT NULL,
    call_id TEXT,
    bound_source TEXT,
    normalized_sha256 TEXT,
    deadline_ms INTEGER,
    state TEXT NOT NULL CHECK (state IN ('reserved', 'uncertain', 'settled', 'released')),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    estimated_tokens TEXT NOT NULL,
    token_upper INTEGER NOT NULL CHECK (token_upper >= 0),
    turns_upper INTEGER NOT NULL CHECK (turns_upper >= 0),
    cost_upper_microusd INTEGER CHECK (cost_upper_microusd >= 0),
    actual_tokens TEXT,
    actual_turns INTEGER CHECK (actual_turns >= 0),
    actual_cost_microusd INTEGER CHECK (actual_cost_microusd >= 0),
    usage_complete INTEGER NOT NULL DEFAULT 0 CHECK (usage_complete IN (0, 1)),
    receipt_event_id TEXT,
    created_ms INTEGER NOT NULL,
    dispatched_ms INTEGER,
    updated_ms INTEGER NOT NULL,
    settled_ms INTEGER,
    released_ms INTEGER
) STRICT;
CREATE INDEX budget_reservation_session ON budget_reservation(session_id, created_ms, request_id);
CREATE INDEX budget_reservation_state ON budget_reservation(state, created_ms, request_id);

CREATE TABLE budget_reservation_allocation (
    request_id TEXT NOT NULL REFERENCES budget_reservation(request_id),
    scope_id TEXT NOT NULL,
    max_tokens INTEGER CHECK (max_tokens >= 0),
    max_turns INTEGER CHECK (max_turns >= 0),
    max_cost_microusd INTEGER CHECK (max_cost_microusd >= 0),
    spent_tokens INTEGER NOT NULL CHECK (spent_tokens >= 0),
    spent_turns INTEGER NOT NULL CHECK (spent_turns >= 0),
    spent_cost_microusd INTEGER NOT NULL CHECK (spent_cost_microusd >= 0),
    PRIMARY KEY (request_id, scope_id)
) STRICT;
CREATE INDEX budget_reservation_scope ON budget_reservation_allocation(scope_id, request_id);

CREATE TABLE budget_reservation_receipt (
    receipt_event_id TEXT PRIMARY KEY,
    request_id TEXT NOT NULL REFERENCES budget_reservation(request_id),
    actual TEXT NOT NULL,
    received_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE budget_reservation_reconciliation (
    operation_id TEXT PRIMARY KEY,
    request_id TEXT NOT NULL REFERENCES budget_reservation(request_id),
    expected_revision INTEGER NOT NULL,
    owner TEXT NOT NULL,
    reason TEXT NOT NULL,
    action TEXT NOT NULL,
    reconciled_ms INTEGER NOT NULL
) STRICT;
