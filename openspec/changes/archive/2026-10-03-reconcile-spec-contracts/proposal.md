# Reconcile cross-spec contracts

## Why

A full review of the 46 capability specs found one structural contradiction (the process model), two design ambiguities (plan as both an agent and a Mode; protected `.cyber/` paths blocking the system's own writes) and roughly thirty contract drifts between specs written in parallel: command names, flag names, route paths, tool parameters, config keys, ID prefixes, budget vocabulary, GC cadences and phase dependencies. Implementation cannot start against contradictory contracts.

## What Changes

- Service-first process model: the TUI and `cyber exec` connect to the registered background server; `--embedded` runs a private server on a private database and never contends for the shared writer lock.
- Planning is a permission Mode only. The `plan` built-in agent is removed; `plan_enter` and `plan_exit` tools are specified.
- Protected-path and sandbox rules distinguish `.cyber/` configuration documents (protected, read-only) from `.cyber/` content directories (plans, workflows, agents, skills, commands, teams, output styles), which are writable.
- The CLI command tree is made authoritative and complete; session commands move under `cyber sessions`; `cyber import` imports other tools' setups only; `--teleport` becomes `cyber teleport`.
- The server route-group list is made authoritative and complete; session streams are `/events`, messages are `/messages`, and one inbox route releases or drops any held item.
- One output-format flag (`--format`), one working-directory flag (`--cwd`), one `auth_token` policy, one log-file contract, one deferred-tool threshold key, one Budget object and one set of budget flags.
- Tool contracts unified: `bash.background`, `monitor` owned by background-tasks, `notify`, `skill.arguments`, `todo`.
- Phase and policy fixes: `/review` runs a skill in P1 and the workflow from P2; the org mode ladder is ordered by what can run without a human; unpriced usage under spend limits is defined; `routines` runs use the `rrn_` prefix.
- Duplicate "Subagent permission inheritance" removed from permissions-modes (owned by agents-subagents). Export and import requirements removed from session-sharing (owned by storage-events).
- A spec lint script and README/ROADMAP updates accompany the change.

## Impact

Documentation-only revision of the greenfield target specification. Twenty-six capability specs receive MODIFIED, ADDED or REMOVED deltas. No runtime code exists yet, so nothing shipped changes behavior.
