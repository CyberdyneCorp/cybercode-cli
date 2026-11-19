-- Reviewer context tool calls counted with the checkout's auto-mode statistics.
ALTER TABLE permission_auto_statistics
    ADD COLUMN reviewer_tool_calls INTEGER NOT NULL DEFAULT 0 CHECK(reviewer_tool_calls >= 0);
