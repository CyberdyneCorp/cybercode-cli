# Design

## Decisions

**Service-first.** One registered `cyber` server per OS user owns the shared database and every Drain. All clients, including the TUI and `cyber exec`, talk to it over the public API. An `--embedded` process is a private server with a private database (in-memory when `--ephemeral`), so two processes never contend for the writer lock and the durability story stays simple. The per-session advisory file lock is dropped; per-session serialization is an in-process concern of the owner.

**Plan is a Mode.** Codex and OpenCode model planning as an agent, Claude Code as a mode. Keeping both produced two independent axes that could disagree. The Mode wins because it composes with any agent and with org ceilings. `plan_enter` and `plan_exit` are ordinary tools gated by permission rules and the question flow.

**Protected versus content paths.** `.cyber/` holds two kinds of files: configuration documents that grant executable trust (`cyber.jsonc`, `hooks.jsonc`, `mcp.json`, plugin manifests and lockfile) and content the system itself writes on the user's behalf (plans, workflows, agents, skills, commands, teams, output styles). Only the first kind is protected and sandbox read-only.

**One owner per contract.** Each shared contract now has one owning spec that others reference: command tree (cli-commands), route groups (server-api), Budget object and budget flags (observability-costs), log files (cli-commands), GC cadence (storage-events), deferred-tool threshold (tool-registry), `monitor` (background-tasks), export and import (storage-events), subagent permission inheritance (agents-subagents).

**Mode ladder.** Org ceilings order Modes by what can execute without a human decision: `plan < default < dont-ask < accept-edits < auto < bypass`. `dont-ask` executes only rule-allowed actions, `accept-edits` additionally executes edits, so a CI ceiling of `accept-edits` still permits `dont-ask`.

**Unpriced spend.** Unpriced usage counts as zero toward spend limits and is reported separately; organizations that want hard limits set `spend.block_unpriced`.

## Validation

`openspec validate --all --strict`, `python3 scripts/spec_lint.py` (registries of commands, routes, tools, config keys, ID prefixes, slash commands and phase dependencies), `python3 scripts/spec_inventory.py`, and a manual read of every modified requirement against its scenarios.

## Tradeoffs

Service-first adds a daemon to the simplest single-shot use. The cost is one `cyber service start` on first use, which the CLI performs automatically. Removing the `plan` agent breaks OpenCode muscle memory (Tab to plan); `/mode plan` and Shift+Tab remain. Renaming budget fields and flags now avoids a compatibility burden later.
