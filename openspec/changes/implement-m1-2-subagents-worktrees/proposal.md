## Why
P1 M1.2 requires isolated subagent sessions, validated structured results, background handback and bounded concurrency. Managed worktrees must have consistent configuration and safe lifecycle ownership before CLI, tools and subagents use them. This change implements the full M1.2 contract; stages do not redefine completion.

## What Changes
- Add validated worktree settings and names, managed Git worktree ownership, cross-process locking, inclusion/setup, cleanup, CLI and API management, and lifecycle events.
- Implement permission-gated child sessions with model/profile selection, structured results, one validation retry, fork/resume, worktree isolation and background handback.
- Enforce concurrency, nesting and inherited permission ceilings; connect task/thread views and cost attribution.

## Capabilities
### Modified Capabilities
- `worktrees`: shared configuration, names and managed lifecycle.
- `agents-subagents`: full P1 child-session execution contract.

## Impact
Core configuration, durable store, runtime/tool host, CLI/API/SDK and TUI. Existing P0 recovery and active M1.1 Windows enforcement work remain required and separate.
