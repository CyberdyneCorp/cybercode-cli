---
name: batch
description: Split a large change into 5-30 focused subagents in separate worktrees.
argument-hint: '<large change>'
---
Implement the requested large change: $ARGUMENTS

Inspect the project and divide the requested change into 5-30 coherent, independently verifiable tasks. Record each task's name, objective, owned files, dependency order and validation. Avoid overlapping file ownership. If the request cannot be split into at least five useful tasks, explain the constraint and ask the user to refine the scope instead of creating artificial work.

Delegate each ready task with the `agent` tool using `agent: general`, a distinct `name`, `isolation: worktree` and `background: true`. Include its objective, file ownership, constraints and required checks in its prompt. Respect the configured concurrency limit and permission decisions; launch dependent tasks only after prerequisites are available. Do not replace worktree isolation with edits in the caller's directory when creation is refused. On denied, failed or uncertain admission, retain the returned receipt and report the blocker; do not blindly relaunch a task whose ownership is unknown.

Collect the normal background completion handbacks and retained worktree metadata. Report each task's status, changes, checks, branch/worktree and integration dependencies. Stop owned background work with `task_stop` if the user cancels. Preserve user edits and retained worktrees. Do not claim the combined change is complete until its integration and checks have actually run; report conflicts and remaining integration work. Apply, merge, commit or publish only when those actions are within the user's request.
