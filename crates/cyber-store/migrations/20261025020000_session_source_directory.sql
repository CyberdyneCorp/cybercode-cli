-- The Location a root worktree Session (a forked Goal, a teammate) was started from;
-- Session listings of that Location include it.
ALTER TABLE session ADD COLUMN source_directory TEXT;
