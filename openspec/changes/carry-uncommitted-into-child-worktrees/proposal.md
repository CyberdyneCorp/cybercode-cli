## Why

An isolated subagent started from the parent checkout's `HEAD` only. Work the user or the parent had not committed yet (edits in progress, new files) was missing from the child's checkout, so the child worked on stale code or redid the edits. The `worktrees` spec already said the uncommitted changes should be committed to a temporary ref first, but this was never built: only `.worktreeinclude` patterns were copied.

## What Changes

- `worktrees` "Subagent isolation": the parent checkout's staged, unstaged and untracked non-ignored work (deletions and renames included) is applied to the child's new checkout and committed there on top of the parent `HEAD`. That commit is the child's base, so the child starts clean, its diff summary shows only its own changes, and cleanup still removes an unchanged checkout. The parent checkout and its index are not touched.
- Files are left behind when the parent Session may not read them: a `read` rule other than `allow` (so `.env` and `.env.*` by default), a Mode or taint that would not allow the read, or a sandbox-hidden path. Ignored files are never carried, and `.worktreeinclude` keeps its behavior.
- New setting `worktrees.carry_limit_mb` (default 100, `0` turns carrying off). Over the limit, creation fails before any checkout exists, with a message naming the total, the limit and the setting.
- The child's worktree result adds `base`, so the parent can diff `base..branch`.

## Non-goals

- Reproducing the parent's index split in the child. Staged and unstaged edits both land in the base commit.
- Carrying into teammate or Goal-fork root worktrees, Workflow agents with a pinned base, or recreated checkouts.

## Impact

- `crates/cyber-core`: `worktrees::carry` (plan and apply), `Repository::create_carrying`, and `Settings::carry_limit_mb`.
- `crates/cyber-tools`: isolated child creation plans the carry under the parent's read rules and sandbox, and the result reports `base`.
- No HTTP route or OpenAPI change.
