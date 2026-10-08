# P1 acceptance audit

Snapshot: 2026-10-08. The full goal covers M1.1–M1.5 and all P1-tagged contracts. No milestone has complete acceptance evidence. This inventory uses the first phase tag of each canonical requirement, matching `scripts/spec_inventory.py`; mixed-phase extensions still need review against the full goal.

## Remaining delivery areas

| Milestone | Existing evidence | Required next work |
|---|---|---|
| M1.1 Sandbox and modes | Windows launch/ACL/job-owner primitives and native tests; mode UI, evaluator, production tool dispatch and confirmed one-shot replay | Complete tool confinement, recursive roots/exclusions, credential-file isolation, proxy-only transport, crash recovery, service shutdown, reviewed orphan/unknown recovery and full ancestor mode semantics |
| M1.2 Subagents and worktrees | Managed checkouts, child execution, durable owners, local stop and reviewed reopening | Complete public enter/exit/cleanup/recovery, cross-process cancellation, reviewed unknown recovery, client/budget cancellation adoption, remaining orchestration and native acceptance |
| M1.3 Extensibility | P0 MCP foundation; typed hook configuration validation; global/project/local group accumulation and selected-profile indexed file origins; handler digest/approval storage | Managed/plugin collection, managed-only authority and execution scope order; hook dispatch, trust review/exec integration and every required lifecycle event, all four handler types; plugin protocol/package; MCP OAuth and deferred search |
| M1.4 Memory and intelligence | P0 context/instruction foundation | Auto-memory lifecycle, LSP/formatter integration and diagnostics feedback; isolated browser verification and revision-linked artifacts |
| M1.5 Migration and editors | P0 compatibility/configuration foundation | Three importers, ACP integration, VS Code extension and required editor capabilities |
| Additional P1 contracts | Some shared runtime/API foundations | Messaging, costs, providers, compaction, SDK/server/TUI/exec surfaces and every remaining contract below |

Hook configuration validation, layer accumulation and selected-profile provenance have implementation and real loading tests. Contributions from loaded global/project/local files retain indexed group and handler origins at top level and after profile selection, including identical definitions; empty event arrays preserve earlier groups. Profile selection preserves defining file paths rather than generic profile labels for handlers. Managed/plugin collection, execution scope sorting and managed-only authority remain open. Canonical effective-handler digests and separate durable handler approvals now have local tests, including concurrent processes, legacy files, canonical checkout isolation and revocation. Invocation approvals are in-memory digest sets; CLI/TUI review, exec flag binding and dispatch-time checks remain open. Hook execution remains unimplemented. Plugin/memory/LSP/browser/import/ACP delivery still needs implementation. These boundaries do not establish a precise completion percentage. Existing passing tests establish their tested boundaries only. Check each scenario and public surface before changing an entry to accepted.

Production auto-mode tool dispatch now has local evidence; see [current status](p1-status.md#production-auto-mode-tool-classification). Validated auto-mode configuration is implemented locally. Checkout-scoped statistics/reset are implemented locally. Confirmed one-shot replay is implemented locally through the TUI, API and SDK. Ancestor auto configuration/review is implemented locally with independent gates and parent context. Next priorities are native acceptance, remaining permission ceilings and complete Windows enforcement. Later milestone delivery areas remain in scope. P0 local-model evaluation remains open and its artifacts are preserved.

## Canonical contract checklist

Every entry starts unverified at the complete requirement scope; this does not imply its foundations are absent. Acceptance needs implementation links, scenario-level tests, public-client evidence and native gates where required. No percentage is inferred from this checklist.

### agents-subagents (14)

Canonical source: [agents-subagents](../../openspec/specs/agents-subagents/spec.md).

- [ ] Agent tool spawns subagents
- [ ] Structured subagent output
- [ ] Background subagents and handback
- [ ] Forked subagents
- [ ] Resume subagents by name
- [ ] Worktree isolation for subagents
- [ ] Concurrency cap
- [ ] Nesting depth
- [ ] Subagent permission inheritance
- [ ] Subagent result summarization
- [ ] Cost attribution
- [ ] Manual invocation by mention
- [ ] Agent thread switching
- [ ] Agent tool catalogue

### background-tasks (15)

Canonical source: [background-tasks](../../openspec/specs/background-tasks/spec.md).

- [ ] Background bash
- [ ] Job registry
- [ ] Completion notices
- [ ] Reading job output
- [ ] Monitor tool
- [ ] Tasks view
- [ ] Stop kills process trees
- [ ] Concurrency limits
- [ ] Jobs end with their session
- [ ] PTY sessions
- [ ] Model interaction with PTYs
- [ ] Notify tool
- [ ] Send file to user
- [ ] Background job events
- [ ] Background commands from the user

### browser-verification (2)

Canonical source: [browser-verification](../../openspec/specs/browser-verification/spec.md).

- [ ] Supported optional browser integration
- [ ] Verification artifacts and lifecycle

### builtin-tools (4)

Canonical source: [builtin-tools](../../openspec/specs/builtin-tools/spec.md).

- [ ] Capability-owned tool catalog
- [ ] powershell tool
- [ ] notebook_edit tool
- [ ] monitor tool

### client-sdk (1)

Canonical source: [client-sdk](../../openspec/specs/client-sdk/spec.md).

- [ ] Hook callbacks

### code-intelligence (12)

Canonical source: [code-intelligence](../../openspec/specs/code-intelligence/spec.md).

- [ ] LSP enablement
- [ ] Built-in servers
- [ ] Custom and overridden servers
- [ ] Lazy spawning and root detection
- [ ] Diagnostics after edits
- [ ] lsp tool
- [ ] Read warms servers
- [ ] LSP status and shutdown
- [ ] Formatter enablement and detection
- [ ] Custom formatters
- [ ] Formatter execution
- [ ] Formatter status

### compaction (3)

Canonical source: [compaction](../../openspec/specs/compaction/spec.md).

- [ ] Compaction hooks
- [ ] Tool output pruning
- [ ] Native provider compaction

### compat-import (10)

Canonical source: [compat-import](../../openspec/specs/compat-import/spec.md).

- [ ] Import command
- [ ] Claude Code sources
- [ ] Codex sources
- [ ] OpenCode sources
- [ ] Read-time skill compatibility
- [ ] Read-time MCP compatibility
- [ ] Mapping report
- [ ] Secret safety during import
- [ ] Source detection
- [ ] Idempotent re-import

### configuration (2)

Canonical source: [configuration](../../openspec/specs/configuration/spec.md).

- [ ] Config inspection by agents
- [ ] References

### cross-session-messaging (18)

Canonical source: [cross-session-messaging](../../openspec/specs/cross-session-messaging/spec.md).

- [ ] Reachable session listing
- [ ] Addressing
- [ ] Send message tool
- [ ] Message content limits
- [ ] Delivery semantics
- [ ] Inbound controls
- [ ] Outcome reporting
- [ ] Replies
- [ ] Idle notifications
- [ ] Rate limits
- [ ] Permission boundaries stay per session
- [ ] Untrusted content
- [ ] Local transport
- [ ] Non-interactive sessions
- [ ] Transcript and audit
- [ ] Usage attribution
- [ ] Disable switch
- [ ] Messaging API

### editor-integration (13)

Canonical source: [editor-integration](../../openspec/specs/editor-integration/spec.md).

- [ ] ACP command and transport
- [ ] ACP initialize and capabilities
- [ ] ACP session lifecycle
- [ ] ACP config options
- [ ] ACP prompting and streaming
- [ ] ACP tool calls and permissions
- [ ] ACP file write-through
- [ ] ACP client MCP servers
- [ ] VS Code extension
- [ ] Inline diff review
- [ ] Selection mentions and plan review
- [ ] IDE context source
- [ ] IDE detection and extension install

### exec-mode (4)

Canonical source: [exec-mode](../../openspec/specs/exec-mode/spec.md).

- [ ] Permission denial log
- [ ] Structured final output
- [ ] CI usage
- [ ] Streaming input and partial output

### hooks (20)

Canonical source: [hooks](../../openspec/specs/hooks/spec.md).

- [ ] Hook configuration
- [ ] Hook scopes and merge order
- [ ] Supported events
- [ ] Matchers
- [ ] Command handlers
- [ ] HTTP handlers
- [ ] Prompt handlers
- [ ] MCP tool handlers
- [ ] Decision schema
- [ ] Decision merging
- [ ] Interaction with permissions
- [ ] Stop hooks and loop prevention
- [ ] Timeouts and async hooks
- [ ] Parallel execution within a group
- [ ] Sandboxing of command hooks
- [ ] Trust for project hooks
- [ ] Hooks viewer and CLI
- [ ] Hook context injection on lifecycle events
- [ ] Hook observability
- [ ] Conditional, one-shot and annotated handlers

### installation-upgrade (1)

Canonical source: [installation-upgrade](../../openspec/specs/installation-upgrade/spec.md).

- [ ] Shell integration on install

### mcp (6)

Canonical source: [mcp](../../openspec/specs/mcp/spec.md).

- [ ] Tool search and deferred loading
- [ ] OAuth
- [ ] Elicitation
- [ ] Roots and sampling
- [ ] Organization MCP controls
- [ ] Server options

### memory (13)

Canonical source: [memory](../../openspec/specs/memory/spec.md).

- [ ] Memory locations
- [ ] Memory file format
- [ ] Index loading
- [ ] Memory tool
- [ ] Automatic memory generation
- [ ] Memory toggles
- [ ] Secret redaction
- [ ] Deduplication and updates
- [ ] Staleness notice
- [ ] Memory command
- [ ] Memory changes during a Session
- [ ] Explicit remember requests
- [ ] Memory HTTP API

### observability-costs (10)

Canonical source: [observability-costs](../../openspec/specs/observability-costs/spec.md).

- [ ] Usage commands
- [ ] Context window meter
- [ ] Status line data contract
- [ ] OpenTelemetry export
- [ ] Prompt content privacy in telemetry
- [ ] Debug traces
- [ ] Doctor health checks
- [ ] Performance diagnostics
- [ ] Usage data retention
- [ ] Diagnostics bundle

### permissions-modes (7)

Canonical source: [permissions-modes](../../openspec/specs/permissions-modes/spec.md).

- [ ] Critical-path removal guard
- [ ] accept-edits mode
- [ ] auto mode classifier
- [ ] dont-ask mode
- [ ] bypass mode
- [ ] Auto-mode configuration and override
- [ ] Rule dry run

### plugins-marketplace (14)

Canonical source: [plugins-marketplace](../../openspec/specs/plugins-marketplace/spec.md).

- [ ] Plugin manifest
- [ ] Component discovery defaults
- [ ] Install scopes
- [ ] Plugin sources
- [ ] Plugin CLI
- [ ] Plugin host process
- [ ] Host protocol
- [ ] Restart and backoff
- [ ] Scoped registrations
- [ ] Capability declaration and enforcement
- [ ] User configuration
- [ ] TypeScript plugin kit
- [ ] Pure mode
- [ ] Trust and signing

### provider-catalog (4)

Canonical source: [provider-catalog](../../openspec/specs/provider-catalog/spec.md).

- [ ] Small model selection
- [ ] Tool-call emulation for models without native tools
- [ ] Additional native provider adapters
- [ ] Model fallback chain

### provider-credentials (3)

Canonical source: [provider-credentials](../../openspec/specs/provider-credentials/spec.md).

- [ ] Multiple connections per provider
- [ ] Provider OAuth flows
- [ ] Command credentials

### sandbox (8)

Canonical source: [sandbox](../../openspec/specs/sandbox/spec.md).

- [ ] Windows enforcement
- [ ] Escalation requests
- [ ] Excluded commands
- [ ] Container detection
- [ ] Sandbox CLI
- [ ] Sandbox events and audit
- [ ] Named sandbox profiles
- [ ] Environment policy

### session-runtime (3)

Canonical source: [session-runtime](../../openspec/specs/session-runtime/spec.md).

- [ ] Eager tool execution
- [ ] Structured output
- [ ] Side chat

### skills-commands (5)

Canonical source: [skills-commands](../../openspec/specs/skills-commands/spec.md).

- [ ] Remote skill sources
- [ ] Skill-scoped tool approvals and model
- [ ] Shell output injection
- [ ] Bundled skills
- [ ] Path-triggered and forked skills

### snapshots-checkpoints (3)

Canonical source: [snapshots-checkpoints](../../openspec/specs/snapshots-checkpoints/spec.md).

- [ ] Non-git fallback
- [ ] Rewind targets
- [ ] Undo and redo shortcuts

### storage-events (2)

Canonical source: [storage-events](../../openspec/specs/storage-events/spec.md).

- [ ] Retention and garbage collection
- [ ] Transcript persistence switch

### system-context (5)

Canonical source: [system-context](../../openspec/specs/system-context/spec.md).

- [ ] Instruction imports
- [ ] Nested rule files on read
- [ ] Skills, references, and MCP instruction sources
- [ ] Context inspection
- [ ] Path-scoped rules and overrides

### tool-registry (3)

Canonical source: [tool-registry](../../openspec/specs/tool-registry/spec.md).

- [ ] Deferred tools and tool search
- [ ] Eager execution during streaming
- [ ] Annotations drive modes and hooks

### tui (7)

Canonical source: [tui](../../openspec/specs/tui/spec.md).

- [ ] Permission mode indicator and cycling
- [ ] Rewind UI
- [ ] Background tasks view
- [ ] Subagent threads and side chat
- [ ] Status line
- [ ] Accessibility
- [ ] Reasoning display and effort command

### vcs-integration (3)

Canonical source: [vcs-integration](../../openspec/specs/vcs-integration/spec.md).

- [ ] Local review command
- [ ] Commit and PR text generation
- [ ] PR checkout and linked sessions

### web-client (1)

Canonical source: [web-client](../../openspec/specs/web-client/spec.md).

- [ ] Local web client

### worktrees (9)

Canonical source: [worktrees](../../openspec/specs/worktrees/spec.md).

- [ ] Session worktrees
- [ ] Worktree location and branch naming
- [ ] Untracked file inclusion
- [ ] Setup commands
- [ ] Enter and exit tools
- [ ] Cleanup on exit
- [ ] Worktree management commands
- [ ] Concurrency safety
- [ ] Worktree events
