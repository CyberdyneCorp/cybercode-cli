# Feature reference

Implemented behavior and remaining scope, reviewed on **2026-10-10**. Start with the [README](../README.md); use the [P1 status](implementation/p1-status.md) and [acceptance audit](implementation/p1-acceptance-audit.md) for detailed evidence.


“Available” describes implemented local-core behavior. “Partial P1” means working pieces exist and the full contract still needs implementation or acceptance.

| Area | What you can use today | Status |
|---|---|---|
| Models | Provider adapters, model catalog, credentials and usage pricing | Available; broader catalog work remains |
| Sessions | Durable inbox, streaming tool loops, interrupt/resume, crash recovery and compaction | Available |
| Tools | Repository inspection/editing, notebook cell editing, command execution, web tools and instruction/skill loading and durable path-triggered skill reminders with model-loaded Turn permission scopes | Available; static configured command templates also reach the public API; broader skills/commands remain |
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
| Migration | Read-only static source inventory and verified snapshots, Claude/OpenCode permissions, constant Codex rules, secret-safe provider/model conversion and static inline/Markdown command proposals and live static Markdown command discovery; `cyber import --detect` reports raw definition counts and `--dry-run` previews supported fields | Partial P1; complete detection, source adapters, reviewed writes and editor integrations remain |
| Operations | Database-plus-artifact backup, verify/restore, retention, logs and diagnostics | Available; broader observability remains |

Remaining P1 scope includes hooks/plugins/MCP (M1.3), memory/code intelligence/browser verification (M1.4), migration/editor integration (M1.5), the local web client and the other P1-tagged APIs. Workflows, goals, loops, remote control and cloud runners belong to later phases. The [capability map](local-core-guide.md#specification-capability-map) covers the full planned product; [OpenSpec](../openspec/specs) defines the contracts by phase.


Read-only migration previews are available with `cyber import <claude|codex|opencode|auto> --dry-run [--scope project|global] [--format json]`. They propose supported permission/model/provider and basic Codex approval/sandbox fields without creating configuration, logs or databases. Existing native keys win; auto fills missing keys in OpenCode, Codex, Claude order. Literal credentials become required environment references rather than copied values. Unsupported sources are listed explicitly. Native global/ancestor/`.cyber` file layers use runtime merge rules, without evaluating profiles or substitutions. Supported leaves and environment requirements carry source paths/pointers. Output is a redacted normalized JSON diff; original-byte/comment-preserving diffs, complete source adapters/reports and confirmed writes remain open. JSON reports keep `complete: false`.
