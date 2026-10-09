# P1 acceptance audit

Snapshot: 2026-10-08. The full goal covers M1.1–M1.5 and all P1-tagged contracts. No milestone has complete acceptance evidence. This inventory uses the first phase tag of each canonical requirement, matching `scripts/spec_inventory.py`; mixed-phase extensions still need review against the full goal.

## Remaining delivery areas

| Milestone | Existing evidence | Required next work |
|---|---|---|
| M1.1 Sandbox and modes | Windows launch/ACL/job-owner primitives and native tests; mode UI, evaluator, production tool dispatch and confirmed one-shot replay | Complete tool confinement, recursive roots/exclusions, credential-file isolation, proxy-only transport, crash recovery, service shutdown, reviewed orphan/unknown recovery and full ancestor mode semantics |
| M1.2 Subagents and worktrees | Managed checkouts, child execution, durable owners, local stop and reviewed reopening | Complete public enter/exit/cleanup/recovery, cross-process cancellation, reviewed unknown recovery, client/budget cancellation adoption, remaining orchestration and native acceptance |
| M1.3 Extensibility | MCP config trust gating, typed/framed protocol, configured local launch/discovery, independent durable ownership and managed checkout pins, shared concurrent runtime startup/tool dispatch and shutdown, shared MCP/client schema-permission-hook admission and output settlement, separate server approvals and definition review CLI (Location-scoped host close now fences admission and preserves unknown ownership; authenticated HTTP and generated SDK close are implemented; committed local status events are delivered independently; configured snapshots are implemented without startup or adoption of stored actors; idle leader/ready-EOF loss is monitored with explicit settlement; withheld definitions, notification pumping, remaining status transitions, CLI/TUI close, reconfiguration, reconnect and full native acceptance remain open); typed hook configuration validation; global/project/local group accumulation and selected-profile indexed file origins; handler digest/approval storage; ordered scope catalog and CLI list/trust/untrust; selector/decision contracts, immutable envelopes, command transport, result interpretation and noninteractive Unix launch and durable recorded command execution; Unix built-in command pre/post dispatch and live notices | Managed/plugin collection, managed-only authority and execution scope order; remaining lifecycle/MCP-hook dispatch and remote client-handler cancellation/recovery, scheduling, context admission, withheld handler/TUI review and exec trust integration, all four handler types; plugin protocol/package; MCP OAuth, remote transports and full native acceptance |
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

Local stdio MCP form elicitation now uses transient call-owned questions, typed schema validation and explicit final consent, with fresh authorization before/after user interaction. Focused runtime tests cover unattended refusal, all three consent actions, definition revocation, native Location-close cleanup and human-timeout/interrupt handling. Broader local validation passes 229 Rust cases, workspace all-target Clippy and 62 SDK tests, with all 58 strict specs valid. Remote transport, URL elicitation and full native acceptance remain pending. The complete elicitation requirement remains unaccepted.

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


Owned MCP startup errors now carry retry-safety evidence only after native/proxy acknowledgement, durable terminal persistence, actual lease settlement and scratch cleanup. Injected failed terminal persistence remains Unknown and refuses retry despite native acknowledgement. Prepared failures explicitly join proxies and retain scratch for caller commit. These tests cover the local settlement boundary; automatic reconnect/backoff and full native acceptance remain unaccepted.


Retained local MCP reconnect now has evidence for exponential delays capped at sixty seconds and ten attempts per loss sequence, fresh configuration/authorization and independent identities, transient startup failure only after complete settlement, no interrupted-call replay, backoff close, failed settlement refusal and Unknown fencing during cancelled reconnect initialization. These local tests do not accept remote reconnect, manual controls, reconfiguration, managed recovery or the full canonical status/M1.3 requirements.


The read-only wait_for_mcp built-in now covers named connecting servers with a zero-to-sixty-second timeout, ordinary schema/agent/hooks/permission/cancellation boundaries and redacted status results. Portable terminal/unknown/invalid-input/deny cases, a per-tool golden and real delayed startup, expiry, cancellation and following-step materialization tests accept this local wait boundary. Waiting never starts servers or grants invocation authority. Deferred tool loading/search, remote transports and full native acceptance remain unaccepted.


Explicit registration scopes and shared deferred materialization now have evidence for the sole typed threshold, actual configuration validation, four-character Unicode token estimates, exact threshold boundaries, loaded selections, schema-free summaries and MCP-looking client precedence. Runtime preflight also rejects changed scope before effects. These are tested registry foundations; Session-persistent selections, tool_search and model request projection are not yet implemented, so deferred-loading parent requirements remain unchecked.

### Session-persistent deferred search and model projection (2026-10-09)

Runtime model requests now apply the shared threshold to the effective catalog: unloaded MCP/plugin schemas are omitted from provider tools, and their names and normalized descriptions appear in a schema-free JSON context block. Calls to those unloaded names settle with the canonical tool_search-first error before executor dispatch. Builtin and higher-precedence Session tools retain full schemas.

The read-only tool_search built-in searches visible MCP/plugin registrations using case-insensitive query tokens or exact names, returns full schemas and durably loads selections for subsequent steps in the same Session. Results sort by name and use a default limit of five (maximum 1000). Exact selections reject unavailable names before committing anything. Ordinary schema, agent, permission, hooks, cancellation and output-budget boundaries apply; loading does not grant execution permission or preserve stale registrations. The new validated session.tools.loaded.1 event is additive and idempotent, survives restart and compaction, and is not inherited by fresh Sessions or forks. Missing Sessions and malformed events are refused in the writer; failed writes and cancelled admission leave selections unchanged.

Local evidence includes a per-tool golden, search scope/query/limit unit tests, durable replay/idempotency/isolation/fork/cancellation and injected writer failure tests, compaction preservation, actual MCP unloaded refusal followed by search/use with exactly one RPC, query-only discovery without RPC, independent Session refusal, pre-hook denial even in bypass mode, and a fresh permission deny hiding an already loaded schema before any RPC. Plugin registration delivery, remote transports and actual Windows MCP process acceptance remain open. No P1 milestone is accepted.

Validation for this increment: 206 distinct focused Rust tests passed across server/runtime/compaction, tool library/goldens/actual MCP runtime, and application library/public MCP tests. Workspace all-target Clippy, formatting, generated SDK consistency, 58 strict OpenSpec items, specification lint (zero errors, 21 warnings), workflow YAML and diff checks pass locally. For preceding 1a3c2b5, Windows lint, Ubuntu, SDK, specs and Linux builds pass; actual Windows job 113703566970 is still running at hook execution history after all earlier MCP and scope-aware deferred foundation gates passed, while macOS jobs remain queued. This does not establish acceptance of the new search increment on Windows.

### Captured local MCP roots (2026-10-09)

Configured local startup now captures the canonical Location and configured sandbox writable paths, deduplicated as percent-encoded file URIs. Read-only policy includes the Location alone. Missing or unsupported roots refuse before native admission; private scratch, unrelated readable paths and sibling worktrees are not implicit roots. Typed root construction bounds serialized data incrementally against half the frame budget, retaining room for callback identities/envelopes. Low-level authorized connection callers can install the same immutable roots object before initialization.

Initialization advertises roots with listChanged false. roots/list is answered during initialization, active RPCs and the bounded idle pump, preserving the server identity and active client request. Malformed parameter shapes receive -32602; unadvertised sampling remains -32601. Root metadata does not expand the configured process sandbox, execution permission or managed checkout ownership. A fresh connection captures current configuration; hot reconfiguration, sampling permission/model/cost routing, remote transports and full native acceptance remain open, and the canonical Roots and sampling parent remains unchecked.

Local evidence: 171 distinct Rust tests passed across the tool library, protocol, configured launch, discovery/shared runtime and public application MCP API. New tests cover canonical/relative/deduplicated URI encoding, unavailable roots, writable files and invalid Location types, initialization/active/idle callbacks, malformed parameters, late configuration refusal, sampling refusal, actual workspace-write/read-only/full-access launch, private/read-only omission and pre-admission failure. Portable roots construction is added to native CI; protocol callbacks already run in the existing native MCP gate. Actual Windows MCP process acceptance remains unproven. No P1 milestone is accepted.

Workspace all-target Clippy, formatting, SDK generation consistency, workflow YAML, 58 strict OpenSpec items, specification lint (zero errors, 21 warnings) and diff checks pass locally. For preceding 56b0e88, actual Windows job 113709940259 has passed the new scope-aware deferred materialization/Session persistence/search/golden gate and is running retained proxy settlement. Earlier protocol/configuration, passive observation and retained startup gates also passed. The complete job and actual Windows MCP subprocess acceptance remain pending.

### Scoped MCP instructions in Context Epochs (2026-10-09)

Retained local publication snapshots now include initialization instructions alongside native connection identity and tools. The mcp/instructions Context Source admits only fresh same-Location server authorization plus an exact effective MCP tool registration. Agent/Mode/permission omissions, disabled or changed definitions, removed discovery, and higher client shadowing all remove eligibility. Deferred definitions retain usage guidance. Server-name order is deterministic, labels are attributed, and server text is escaped to preserve mcp_instructions/server boundaries. Metadata reads require no RPC ownership, Session borrowing or startup.

Typed observations explicitly report Absent when no eligible source remains. The existing Context Epoch reconciler therefore appends source replacement/withdrawal system messages while retaining its byte-stable baseline. Actual loss, reconnection with changed initialization instructions, permission changes and Location close have local evidence for this boundary; historic baseline text remains followed by the explicit replacement or withdrawal message.

Local validation covers 169 distinct Rust tests across tool library/shared MCP runtime, application library/public MCP API and server library/context/compaction. New portable scope/registration/metadata-change/escaping checks and actual deferred visibility, permission withdrawal, agent and disabled-server filtering, reconnect replacement/close withdrawal, and higher client shadowing tests pass without tool RPC effects. Portable instruction selection is included in the existing native MCP pool test gate. Remote transports and full native MCP process acceptance remain open; no P1 milestone is accepted.

Workspace all-target Clippy, formatting, SDK generation consistency, 58 strict OpenSpec items, specification lint (zero errors, 21 warnings), workflow YAML and diff checks pass locally. For preceding 5eed2e6, actual Windows job 113715154631 has passed the passive observation/captured-roots and scope-aware deferred materialization gates, plus retained startup/proxy/ownership gates, and remains running at the public MCP Location-close API. The full job and actual Windows MCP subprocess/instruction acceptance remain unproven.

### Required MCP first-Turn readiness (2026-10-09)

Typed `required` flags now gate the first Turn before Epoch initialization, input promotion and model invocation. Default false preserves legacy approval digests; true changes the approved definition. The gate reads fresh configured status and verifies same-connection publication under each server's configured timeout. Terminal failures report `McpRequiredError: <name>` and leave the original inbox prompt pending. Cancellation stops only the wait, retaining independent Location startup ownership. Existing durable StepStarted events derive the once-per-Session state through replay and conversation rewind; forks start their own readiness check.

Tests cover default digest compatibility, invalid flag privacy, delayed actual local discovery, timeout with retry of the original prompt after acknowledged startup settlement, cancellation with one retained process, disabled/unsupported required servers, replay, rewind and fresh forks. Remote transports, headers_command/header refresh, per-server output caps, `cyber mcp get` and complete native acceptance remain open. The canonical Server options requirement and M1.3 milestone remain unchecked.

Local validation passes 237 Rust test executions across required/readiness, full runtime/configuration, shared tools and application-host suites. Workspace all-target Clippy with warnings denied, formatting, generated SDK consistency and all 58 strict OpenSpec items pass; cross-spec lint reports zero errors and 21 warnings. Portable required terminal-state coverage is included in the existing native Windows MCP test gate; new actual subprocess cases await CI.

### MCP per-server output caps and resolved review (2026-10-09)

Optional positive output_token_limit values now participate in exact server digests while omitted defaults preserve previous approvals. Native MCP registrations capture the cap before dispatch and share global settlement for success, error and structured text. Four-Unicode-character estimates bound the retained payload under global line/byte ceilings; the standard overflow notice follows it. Full original text is written once to managed storage, structured values remain unchanged and persistence failure refuses lossy success. This is estimated payload accounting, not provider tokenizer accounting.

cyber mcp get <name> now reuses definition inspection, provenance, digest and authorization observations without connecting, launching commands, starting model work or opening the database. It reports required=false defaults and inherited global output ceilings, redacts headers/environment/OAuth/URL query secrets and preserves the validated numeric budget through generic token-key redaction. Dynamic header commands/refresh and complete remote/native acceptance remain open; Server options and M1.3 remain unchecked.

Local validation passes 200 distinct Rust cases (283 executions including repeated shared-library checks), workspace all-target Clippy with warnings denied, formatting, generated SDK consistency, all 58 strict OpenSpec items and workflow YAML validation. Cross-spec lint reports zero errors and 21 warnings. Existing Windows MCP configuration and CLI review gates include the new portable tests; new revision native acceptance awaits CI.

### MCP call timeout overrides and progress validation (2026-10-09)

Regression tests fail against the main implementation before correction: duplicate progress extends inactivity, and the runtime ignores an explicit one-second server timeout for a blocked call. Explicit timeout presence is now retained in typed configuration. Omitted connect defaults remain 30 seconds with unchanged approval digests and global call fallback (300 seconds by default). Explicit overrides affect connection and call inactivity and add a distinct digest marker; these definitions require fresh approval. CLI review reports resolved call_timeout_seconds. Invalid null/zero/noninteger values remain refused.

Only matching finite increasing progress notifications reset request inactivity. Duplicate/regressive values, malformed optional total/message fields and request identities do not extend it; fractional values remain supported. Timeout retains unresolved RPC ownership until acknowledged native and durable settlement, with no automatic replay. The actual configured-runtime timeout regression verifies native terminal settlement. Remote/native acceptance and complete M1.3 delivery remain open.

Local validation passes 201 distinct Rust cases across configuration/trust, protocol, actual runtime/launch, shared tools, application host and CLI review. Workspace all-target Clippy with warnings denied, formatting, generated SDK consistency and all 58 strict OpenSpec items pass. Cross-spec lint reports zero errors and 21 warnings. Existing Windows MCP configuration/protocol and CLI-review gates include the portable regressions; actual subprocess timeout acceptance remains pending for the new revision.

### Graceful POSIX MCP Location shutdown (2026-10-09)

The actual runtime regression fails on main because immediate SIGKILL prevents a configured server's SIGTERM cleanup handler. Explicit Location/runtime close now uses a dedicated graceful route while startup failure, idle loss, cancelled calls and owner disposal retain immediate cleanup. On POSIX it closes client IO, signals the owned group with SIGTERM, retains the unreaped leader identity for five seconds, then sends SIGKILL and consumes native acknowledgement before proxy/durable/checkout/scratch settlement. Windows Job Object termination remains unchanged.

The native connection retains the original grace deadline across close-future disposal and retry. Unknown ownership or failed settlement preserves the admission fence and recovery evidence. Actual cooperative descendants get cleanup time after early leader exit; stubborn descendants are killed before delayed marker effects. The two-server regression first fails with sequential grace periods and now passes with concurrent retained-owner close and all results collected. Remote shutdown and full native platform acceptance remain open; M1.3 is not accepted.

Local validation passes 172 distinct Rust cases across shared tools, actual runtime/launch, retained deadline/disposal, application host and authenticated MCP APIs. All 58 strict OpenSpec items, generated SDK consistency and formatting pass; cross-spec lint reports zero errors and 21 warnings. New POSIX graceful tests await native CI; Windows Job termination retains its existing native gate. Complete MCP and M1.3 acceptance remain open.

Workspace all-target Clippy with warnings denied also passes for this revision.
