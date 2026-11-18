## MODIFIED Requirements

### Requirement: Subagent isolation
(P2) An agent definition or subagent call with `isolation: "worktree"` SHALL run the child Session in a fresh managed worktree branched from the parent checkout's current `HEAD` plus its uncommitted work. The uncommitted work is the parent checkout's working tree compared with `HEAD`: staged and unstaged changes to tracked files, deletions, renames (a deletion plus a new file) and untracked files that `.gitignore` does not ignore. The system SHALL apply it to the new checkout and commit it there, with the parent `HEAD` as parent, as the child's base, so the child starts clean and its result shows only its own changes. The parent checkout, its index and its `HEAD` SHALL NOT change. Ignored files SHALL never be carried; `.worktreeinclude` copies SHALL be made as before and stay uncommitted. A file the parent Session may not read SHALL stay behind: one whose `read` rule is not `allow` (by default `.env` and `.env.*`), that the Session's Mode or taint would not allow, or that the sandbox hides (`deny_read`, unreadable paths). Symbolic links, submodules and other non-regular files SHALL stay behind. When the carried files total more than `worktrees.carry_limit_mb` (default 100), creation SHALL fail before any checkout is made, with a message naming the total, the limit and the setting; `0` turns carrying off. Nothing SHALL be carried onto a configured `worktrees.base` other than the parent's `HEAD`, onto a Workflow agent's pinned base, or into a recreated checkout. On completion the result SHALL include `worktree_path`, `branch`, `base` and a diff summary, and the parent SHALL decide whether to merge.

#### Scenario: Isolated subagent result
- **WHEN** a subagent with `isolation: "worktree"` finishes after editing 3 files
- **THEN** the parent receives the branch name and a diff summary of the 3 files

#### Scenario: Uncommitted work carried
- **WHEN** the parent checkout has a staged edit, an unstaged edit, a deleted file, a renamed file and an untracked `notes.md`, and starts an isolated subagent
- **THEN** the child's checkout has all of them, committed as its base on top of the parent `HEAD`, and `git status` in the child is clean

#### Scenario: Unreadable file left behind
- **WHEN** the parent checkout has an untracked, unignored `.env` and the default `read` rules ask for it
- **THEN** the child's checkout has no `.env`

#### Scenario: Too much to carry
- **WHEN** the parent checkout has a 300 MiB untracked file and `worktrees.carry_limit_mb` is 100
- **THEN** the subagent call fails with `Uncommitted changes to carry into the isolated worktree total 300 MiB, over the 100 MiB limit (worktrees.carry_limit_mb); commit, ignore or remove large files, or raise the limit` and no checkout is created
