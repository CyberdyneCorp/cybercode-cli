-- Immutable attribution captured with request admission, including late owner receipts.
ALTER TABLE budget_reservation ADD COLUMN origin_context TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(origin_context));
