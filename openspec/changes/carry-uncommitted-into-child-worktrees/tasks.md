## 1. Carry

- [x] 1.1 `worktrees::carry::plan`: `git diff --name-only HEAD` (deletions and the rest) and `git ls-files --others --exclude-standard`; non-regular and unreadable files skipped; the size limit is checked before creation.
- [x] 1.2 `Repository::create_carrying`: after `.worktreeinclude` copies, delete and copy the planned files, `git add`, `commit-tree` on the parent `HEAD`, and move the branch; the commit becomes `Managed.base`. Skipped when the base is not the parent `HEAD`.
- [x] 1.3 Isolated child creation plans the carry with the parent Session's `read` rules, Mode, taint and sandbox-hidden paths; `worktrees.carry_limit_mb`; the child result reports `base`.
- [x] 1.4 Tests: staged, unstaged, untracked, deleted and renamed files; ignored files and an unreadable `.env` left behind; `.worktreeinclude` copies stay uncommitted; another base or a clean checkout carries nothing; the size limit message; an end-to-end isolated subagent.
- [x] 1.5 Guide and CHANGELOG.
