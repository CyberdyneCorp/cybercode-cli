# Feature reference

Implemented behavior and remaining scope, reviewed on **2026-10-10**. Start with the [README](../README.md); use the [P1 status](implementation/p1-status.md) and [acceptance audit](implementation/p1-acceptance-audit.md) for detailed evidence.


“Available” describes implemented local-core behavior. “Partial P1” means working pieces exist and the full contract still needs implementation or acceptance.

| Area | What you can use today | Status |
|---|---|---|
| Models | Provider adapters, model catalog, credentials and usage pricing | Available; broader catalog work remains |
| Sessions | Durable inbox, streaming tool loops, interrupt/resume, crash recovery and compaction | Available |
| Tools | Repository inspection/editing, notebook cell editing, command execution, web tools and instruction/skill loading | Available; broader skills/commands remain |
| Safety | Workspace trust, permission rules, protected paths, auto classification with confirmed `/approve`, macOS/Linux sandboxing and credential masking | Available; Windows enforcement, proxy coverage and full auto-mode controls remain partial P1 |
| File recovery | Shadow-git snapshots and conflict-aware restore preserving user edits | Available; broader rewind UI remains |
| Clients | TUI, noninteractive `exec`, background service, HTTP/SSE/WebSocket/stdio API and generated TypeScript SDK | Available; complete P1 client surfaces remain |
| Subagents | Foreground/background children, named resume, forked context, structured results, approvals and task controls | Partial P1 |
| Worktrees | Managed startup and isolated children, setup journals, file summaries, cleanup and reviewed setup retry | Partial P1; public lifecycle and unknown-effect recovery remain |
| Cancellation | Durable admission closure, exclusive child result ownership, bounded stop reports and matched reopening | Partial P1; unknown-effect recovery, cross-process actor cancellation and exec/TUI adoption remain |
| Spending | Durable own/descendant billing, atomic usage API, TUI `/cost` and soft Session budgets | Partial P1; reservations, daily caps and complete enforcement/displays remain |
| Hooks | CLI/TUI definition and receipt review, exact handler trust, Unix command, HTTP, prompt and local MCP-tool hooks, plus synthetic `cyber hooks test` | Partial P1; other handlers/events, complete scheduling, Windows confinement and plugins remain |
| MCP | Local approvals, shared tools/status, startup waiting, schema search and bounded reconnect | Partial P1; remote transports and resources/prompts remain |
| Memory | Durable project/global notes, built-in memory tool, session index updates, CLI management and local/server recovery, authenticated HTTP/SDK CRUD/recovery, TUI review/editing/deletion/recovery, private draft/request checkpoints, request outcome lookup/record controls and durable change events | Partial P1; legacy recovery, strengthened native recovery checks, broader client acceptance and multi-client retention remain |
| Code intelligence | Local discovery and authenticated live CLI status; LSP read warming, edit error feedback and eight navigation operations including rename preview; automatic sandboxed formatting after edits; owned services with activity-aware idle release, scoped close/reload and HTTP/SDK status; fourteen servers/twelve formatters | Partial P1; native acceptance, enforced Windows launch and full diagnostic lifecycle/client controls remain |
| Migration | Read-only static source inventory, Claude/OpenCode permissions and constant Codex rule conversion; `cyber import` is not available yet | Partial P1; source adapters, reviewed writes and editor integrations remain |
| Operations | Database-plus-artifact backup, verify/restore, retention, logs and diagnostics | Available; broader observability remains |

Remaining P1 scope includes hooks/plugins/MCP (M1.3), memory/code intelligence/browser verification (M1.4), migration/editor integration (M1.5), the local web client and the other P1-tagged APIs. Workflows, goals, loops, remote control and cloud runners belong to later phases. The [capability map](local-core-guide.md#specification-capability-map) covers the full planned product; [OpenSpec](../openspec/specs) defines the contracts by phase.

