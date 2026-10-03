## 1. Structural contradictions

- [x] 1.1 Service-first process model across cli-commands, tui, exec-mode, session-runtime, storage-events, server-api (stdio transport), client-sdk.
- [x] 1.2 Plan as a Mode only: agents-subagents, permissions-modes (plan_enter/plan_exit), tui scenarios.
- [x] 1.3 Protected configuration documents versus writable content directories: permissions-modes, sandbox.

## 2. Authoritative registries

- [x] 2.1 Complete the CLI command tree and resource groups; move session commands under `cyber sessions`; remove `--teleport`.
- [x] 2.2 Complete the server route-group list; `/events`, `/messages`, single inbox release/drop route; restrict `auth_token`.
- [x] 2.3 Define the Budget object and budget flags once; update workflows, goals, loops, routines, agent-teams.

## 3. Drift fixes

- [x] 3.1 Flags, ports, logs, thresholds, tool parameters, config keys, ID prefixes, GC cadence.
- [x] 3.2 Remove duplicates: subagent inheritance (permissions-modes), export/import (session-sharing).
- [x] 3.3 Phase and policy fixes: `/review`, mode ladder, unpriced spend.

## 4. Documentation and tooling

- [x] 4.1 Add `scripts/spec_lint.py` and document it in ROADMAP.
- [x] 4.2 Update README (quickstart path, reconciled contracts) and ROADMAP (entitlement plans, inventory).
