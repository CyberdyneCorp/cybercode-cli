# P1 acceptance audit

Snapshot: 2026-10-08. The full goal covers M1.1–M1.5 and all P1-tagged contracts. No milestone has complete acceptance evidence. This inventory uses the first phase tag of each canonical requirement, matching `scripts/spec_inventory.py`; mixed-phase extensions still need review against the full goal.

## Remaining delivery areas

| Milestone | Existing evidence | Required next work |
|---|---|---|
| M1.1 Sandbox and modes | Windows launch/ACL/job-owner primitives and native tests; mode UI, evaluator, production tool dispatch and confirmed one-shot replay | Complete tool confinement, recursive roots/exclusions, credential-file isolation, proxy-only transport, crash recovery, service shutdown, reviewed orphan/unknown recovery and full ancestor mode semantics |
| M1.2 Subagents and worktrees | Managed checkouts, child execution, durable owners, local stop and reviewed reopening | Complete public enter/exit/cleanup/recovery, cross-process cancellation, reviewed unknown recovery, client/budget cancellation adoption, remaining orchestration and native acceptance |
| M1.3 Extensibility | MCP config trust gating, typed/framed protocol, configured local launch/discovery, independent durable ownership and managed checkout pins, shared concurrent runtime startup/tool dispatch and shutdown, shared MCP/client schema-permission-hook admission and output settlement, separate server approvals and definition review CLI (Location-scoped host close now fences admission and preserves unknown ownership; authenticated HTTP and generated SDK close are implemented; committed local status events are delivered independently; configured snapshots are implemented without startup or adoption of stored actors; idle leader/ready-EOF loss is monitored with explicit settlement; withheld definitions, notification pumping, remaining status transitions, CLI/TUI close, reconfiguration, reconnect and full native acceptance remain open); typed hook configuration validation; global/project/local group accumulation and selected-profile indexed file origins; handler digest/approval storage; ordered scope catalog and CLI list/trust/untrust; selector/decision contracts, immutable envelopes, command transport, result interpretation and noninteractive Unix launch and durable recorded command execution; Unix built-in command pre/post dispatch and live notices | Managed/plugin collection, managed-only authority and execution scope order; remaining lifecycle/MCP-hook dispatch and remote client-handler cancellation/recovery, scheduling, context admission, withheld handler/TUI review and exec trust integration, all four handler types; plugin protocol/package; MCP OAuth and deferred search |
| M1.4 Memory and intelligence | P0 context/instruction foundation | Auto-memory lifecycle, LSP/formatter integration and diagnostics feedback; isolated browser verification and revision-linked artifacts |
| M1.5 Migration and editors | P0 compatibility/configuration foundation | Three importers, ACP integration, VS Code extension and required editor capabilities |
| Additional P1 contracts | Some shared runtime/API foundations | Messaging, costs, providers, compaction, SDK/server/TUI/exec surfaces and every remaining contract below |

Hook configuration validation, layer accumulation and selected-profile provenance have implementation and real loading tests. Contributions from loaded global/project/local files retain indexed group and handler origins at top level and after profile selection, including identical definitions; empty event arrays preserve earlier groups. Profile selection preserves defining file paths rather than generic profile labels for handlers. Managed/plugin collection, execution scope sorting and managed-only authority remain open. Canonical effective-handler digests and separate durable handler approvals now have local tests, including concurrent processes, legacy files, canonical checkout isolation and revocation. Invocation approvals are in-memory digest sets; CLI list/trust/untrust now supports resolved definition review and exact-digest approval/revocation; the catalog preserves scope order and exposes sandbox requirements. Withheld raw handler inspection, TUI review, exec flag binding and dispatch-time checks remain open. Compiled subject/path/condition selection, event-aware decision parsing/merging and immutable common envelope assembly now have local contract tests. Reserved identity fields cannot be replaced by payloads, and rewrites feed later selector conditions without changing the envelope. Runtime identity capture, lifecycle emission and ownership for every handler type remain open. Owned command transport now pumps event stdin and both bounded output streams concurrently, retains exit codes and exposes explicit stop acknowledgement. Local pressure/truncation/early-exit/cancellation tests pass; native AppContainer event-input/stream/exit and pre-cancellation transport tests pass at `641749e`. Lifecycle receipt integration for all handlers, sequential dispatch and tool-schema revalidation remain open. Command result interpretation now validates complete JSON decisions, preserves exit-two reasons and other-code nonblocking errors, refuses truncated/malformed decision objects, applies timeout policy and exposes cancellation/unknown-termination stop flags. Four contract tests and an actual-process capture/interpretation test cover these boundaries; dispatcher decision admission and receipt privacy for the remaining handlers remain required. The ordinary Windows helper still consumes stdin for its private launch handshake and gives the user command null stdin; its event-input launch route remains required. The noninteractive Unix command runner now binds the event Location to loaded checkout provenance, rechecks durable checkout and handler trust, supplies stdin/environment and enforces required workspace-write sandboxing regardless of ordinary full access. Local actual-command tests cover invocation-only approval, home-write denial, credential masking, cancellation cleanup and scratch retention after caller disposal. Its proxy refuses unknown domains; interactive network permissions, Windows launch and runtime lifecycle dispatch remain required. Durable hook admission/terminal receipts now use independent execution aggregates with atomic projections, writer-side Session/ancestor fences and pinned IO policy. Recorded Unix commands use tracked native activity and launch markers; actual-process tests cover subtree cancellation acknowledgement, caller disposal and unknown receipts. Cross-process recovery reconciliation, lifecycle emission and transcript/API/SDK/TUI observability remain open. Unix built-in command PreToolUse/PostToolUse/PostToolUseFailure dispatch is now wired through App’s live provenance-aware resolver. Tests cover chained/schema-checked rewrites, bypass denial, permission deny precedence, explicit asks, project-handler approval, relative target matching, duplicate command execution and transient notices. The session and instance streams carry notices without durable transcript IO. Built-in command PermissionRequest dispatch now answers only the current ask without saved approvals, ignores rewrites and preserves ordinary hard denials/Mode ceilings. Command once-per-Session claims now have durable multi-runtime/restart/unknown-settlement evidence, and status/completion messages use transient user notices. Other event/MCP-hook dispatch and client-handler cancellation/recovery, full target extraction, workspace/project identity, concurrency/async scheduling, once for other handler types and context/continuation admission remain open. Plugin/memory/LSP/browser/import/ACP delivery still needs implementation. These boundaries do not establish a precise completion percentage. Existing passing tests establish their tested boundaries only. Check each scenario and public surface before changing an entry to accepted.

Production auto-mode tool dispatch now has local evidence; see [current status](p1-status.md#production-auto-mode-tool-classification). Validated auto-mode configuration is implemented locally. Checkout-scoped statistics/reset are implemented locally. Confirmed one-shot replay is implemented locally through the TUI, API and SDK. Ancestor auto configuration/review is implemented locally with independent gates and parent context. Next priorities are native acceptance, remaining permission ceilings and complete Windows enforcement. Later milestone delivery areas remain in scope. P0 local-model evaluation remains open and its artifacts are preserved.

The ordinary Windows helper now has an explicit parent-owned event-input route with exact unbuffered permit consumption. Eligible global full-access hooks select PowerShell and retain event stdin; required sandbox hooks still refuse before effects. Native pressure/EOF/exit/invalid-permit/owner-drop and loaded-config launch tests are added to existing CI suites; acceptance is pending. Full Windows confinement and complete hook lifecycle delivery remain required.

Direct-store CLI execution history now shares Session-scoped receipt pagination without constructing Runtime owners or initiating reconciliation. Its real subprocess test verifies privacy/status/order/errors, malformed-config independence and unchanged durable events while an owner remains running. Native execution is pending; complete transcript/TUI/recovery and history controls remain required.

Native CLI hook-history execution passes at `273ab22` in Windows job `113568488946`. Configuration, individual trust, CLI review, selectors and envelopes also pass; the overall job remains running. This accepts those tested native boundaries without accepting the full hook requirement or Windows confinement.

TUI `/hooks history` now pages committed Session receipts through the public API, with refresh/scroll/dismissal and unresolved-state labels. Local rendered/HTTP/runner cases cover raw-IO exclusion, escaping, cursor encoding and stale/foreign-response guards. Native execution is pending; the definition/trust viewer, transcript integration and full recovery controls remain required.

Authenticated Location-scoped hook catalog access now uses the live full resolver, original digests and uncached approvals with header/secret redaction and withheld-path metadata. Real App TCP and typed SDK cases prove inspected boundaries without Session/hook admission or command effects. Native catalog execution, withheld raw review and the complete TUI definition/trust controls remain required.

## Canonical contract checklist

The MCP source audit at `7f13329` found configuration trust gating and the separate remote-tool JSON-RPC interface, but no MCP initialization or `tools/call` client in the committed Rust sources. The earlier “P0 MCP foundation” label did not establish runtime MCP support. Typed parsing/framing, configured local process ownership and discovery, independent durable receipts/managed checkout pins, shared concurrent runtime startup and local tool dispatch through permissions/hooks are now implemented locally. Public live status/events, Location close/reconfiguration, reconnect, MCP hook handlers, compatibility files, remote transports, OAuth and deferred search remain open.

Synchronous prompt handlers now have local evaluator/small-model selection, tool-free strict bounded judgements, real built-in denial, synthetic CLI provider calls and hidden Session/ancestor billing of observed usage. Native HTTP provider transports now acknowledge local proxy shutdown before nonblocking/fail-closed timeout settlement and subtree cancellation; custom adapters without that capability retain unknown outcomes. Remote processing/final billing, remaining lifecycle dispatch and native acceptance remain open. No complete hook contract is accepted by these local cases.

Synchronous HTTP hooks now have Session and synthetic execution through the existing tool/permission and CLI dispatchers, explicit scoped network policy, strict bounded decisions and owned local proxy shutdown. Real TCP tests cover headers, denial, timeout, cancellation, once claims, privacy and unknown disposal. Prompt/MCP handlers, async scheduling, remaining lifecycle events and full native acceptance remain open; local transport acknowledgement does not prove remote POST rollback.

Native hook acceptance remains pending. At `641749e`, Windows job `113500542574` passes hook configuration, individual handler trust/concurrent storage, CLI hook review, selectors/decisions, envelope tests and AppContainer command stdin/owned transport. Result interpretation passes its native Windows gate at `7d46896`, job `113507866156`; that overall job is still running. At `71eec54`, Windows job `113513135733` passes command trust/launch-refusal, result and AppContainer transport gates; that overall job remains running. The durable receipt gate passes at `1a2c586`, Windows job `113524213736`; complete native dispatch acceptance remains open. The overall Windows job is still running. Earlier configuration steps at `0e893f9` failed and their logs were unavailable while the job ran. Current configuration fixtures normalize nested path components; current native success supersedes the old configuration gate without establishing the previous failure cause. Full hook dispatch and native execution acceptance remain open.

At `1a2c586`, Linux job `113524213819` fails the command-hook home-write outcome assertion against the private credential-directory overlay. The host credential file stays unchanged, but the writable overlay reports success. A read-only overlay remount and dedicated actual-command Linux regression are added; native acceptance remains pending. Local built-in dispatch validation passes 1,140 Rust executions, workspace Clippy, 53 SDK tests and all 58 strict specs without accepting whole hook requirements.

At `80587be`, Linux job `113541744124` completes successfully, including durable once claims and corrected command/credential-directory confinement. Windows job `113541743939` reports native lint failure and remains running with logs unavailable. CI splits that lint command into a dedicated short job and retains its mandatory outcome in Windows build, so subsequent diagnostics need not wait for all native tests. The lint cause and full Windows acceptance remain open. Local PermissionRequest integration passes 1,148 Rust executions and all-target Clippy, preserving auto always-block gates even for an explicit pre-hook ask; it does not accept all hook lifecycle requirements.

At `9094310`, standalone Windows lint job `113547414708` completes with an unused mutable DirBuilder binding in command scratch preparation. The fix confines mutability to Unix and retains its private directory mode; a portable scratch ownership regression and native CI gate are added. Bounded fixed-input command concurrency now has actual-process pool/refill/order/cancellation tests. Complete pre-tool concurrency, async contexts, remaining event/handler delivery and native acceptance remain open.

Windows lint at `7e6aede`, job `113555145769`, completes successfully. Authenticated Session receipt pagination and generated SDK access now expose durable hook outcomes, IO policy, optional call/tool correlation and unresolved observations. HTTP/SDK tests cover scope, cursor, status and privacy boundaries; full recovery, transcript/review clients and hook lifecycle acceptance remain open.

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

Authenticated hook approval/revocation APIs now have exact current-digest and checkout-trust refusal coverage, malformed-config obsolete revocation and typed Location-preserving SDK methods. These tests accept the tested API boundaries; full TUI definition/trust controls and native API acceptance remain open.

TUI `/hooks` now reviews loaded definitions and exact project/local approvals through authenticated public APIs, with confirmation, fresh post-write metadata and Session/Location/generation guards. This covers loaded review and tested trust controls locally; native acceptance, raw withheld inspection, last-run integration and synthetic testing remain required.

Raw withheld top-level/profile sections now have a distinct catalog/API/SDK/CLI/TUI representation, retaining literal placeholders and origins with credential redaction. Review does not substitute, validate or compile handlers and provides no executable approval digest. Native acceptance, last-run integration and synthetic testing remain required.

Loaded hook review now includes last-run summary metadata correlated by effective digest/event/scope and current canonical checkout using captured receipt Locations. Summaries exclude raw IO/decisions and preserve unresolved states. This does not verify live ownership or historical checkout incarnations; native acceptance and synthetic testing remain required.

Synthetic hook executions now have independent durable receipt ownership/projection and canonical events, with no Session creation/binding, pinned identity/IO policy, isolated once claims and unknown disposal. Tests cover those storage boundaries. The CLI test surface and actual synthetic execution/scheduling/native ownership remain unimplemented; this does not accept the synthetic-testing requirement.


Bounded local MCP notification processing and list-change refresh now have unit and actual-process evidence for partial/cancelled framing, idle/active notifications, changed-definition refusal before listing, metadata-bound stale registrations, blocked-refresh close and notification-buffered EOF. These tests accept those local boundaries only; reconnect/backoff, resources/prompts, remote transport, complete status behavior and native platform acceptance remain open. The canonical parent requirements remain unchecked.
