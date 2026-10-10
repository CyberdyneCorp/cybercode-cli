# P1 acceptance audit

Snapshot: 2026-10-09. The full goal covers M1.1–M1.5 and all P1-tagged contracts. No milestone has complete acceptance evidence. This inventory uses the first phase tag of each canonical requirement, matching `scripts/spec_inventory.py`; mixed-phase extensions still need review against the full goal.

## Remaining delivery areas

| Milestone | Existing evidence | Required next work |
|---|---|---|
| M1.1 Sandbox and modes | Windows launch/ACL/job-owner primitives and native tests; mode UI, evaluator, production tool dispatch and confirmed one-shot replay | Complete tool confinement, recursive roots/exclusions, credential-file isolation, proxy-only transport, crash recovery, service shutdown, reviewed orphan/unknown recovery and full ancestor mode semantics |
| M1.2 Subagents and worktrees | Managed checkouts, child execution, durable owners, local stop and reviewed reopening | Complete public enter/exit/cleanup/recovery, cross-process cancellation, reviewed unknown recovery, client/budget cancellation adoption, remaining orchestration and native acceptance |
| M1.3 Extensibility | MCP config trust gating, typed/framed protocol, configured local launch/discovery, independent durable ownership and managed checkout pins, shared concurrent runtime startup/tool dispatch and shutdown, shared MCP/client schema-permission-hook admission and output settlement, separate server approvals and definition review CLI (Location-scoped host close now fences admission and preserves unknown ownership; authenticated HTTP and generated SDK close are implemented; committed local status events are delivered independently; configured snapshots are implemented without startup or adoption of stored actors; idle leader/ready-EOF loss is monitored with explicit settlement; withheld definitions, notification pumping, remaining status transitions, CLI/TUI close, reconfiguration, reconnect and full native acceptance remain open); typed hook configuration validation; global/project/local group accumulation and selected-profile indexed file origins; handler digest/approval storage; ordered scope catalog and CLI list/trust/untrust; selector/decision contracts, immutable envelopes, command transport, result interpretation and noninteractive Unix launch and durable recorded command execution; Unix built-in command pre/post dispatch and live notices | Managed/plugin collection, managed-only authority and execution scope order; remaining lifecycle/remote MCP-hook dispatch and remote client-handler cancellation/recovery, scheduling, context admission, withheld handler/TUI review and exec trust integration, all four handler types; plugin protocol/package; MCP OAuth, remote transports and full native acceptance |
| M1.4 Memory and intelligence | P0 context/instruction foundation; shared memory document/write validation, deterministic indexes, bounded UTF-8 snapshots, typed configuration and directory-bound read/scope ownership | Auto-memory lifecycle, LSP/formatter integration and diagnostics feedback; isolated browser verification and revision-linked artifacts |
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

### Local call-owned MCP sampling (2026-10-09)

Opt-in local stdio sampling now advertises basic capability and uses transient native-call ownership, `mcp_sampling` permission for the server name, configured small-model selection, fresh authorization and Session/ancestor budget gates. Typed bounded requests exclude Session history/cache identity, provider body authority and unsupported tool/audio/ambient-context extensions. Retained nested tasks drain local provider transports and observed server-attributed billing before native shutdown. Actual client-request observations distinguish human waiting from immediate denial; malformed/denied callback loops cannot renew outer inactivity.

Local runtime evidence covers allow/deny/default-off, billing, pending-approval revocation, Location-close request cleanup, Session/ancestor exhaustion during approval and soft-budget overrun withholding after usage recording. Actual sockets across OpenAI-compatible, OpenAI Responses and Anthropic cover timeout/close and private-input/credential omission from Session events. Portable protocol evidence covers basic-only capability and exact-identity startup/active unowned refusals. Native platform CI and remote MCP sampling remain open; the complete Roots and sampling requirement and all P1 milestones remain unaccepted.

## Synchronous local MCP-tool hooks (2026-10-09)

Recorded tool/permission hook dispatch and synthetic CLI testing now support configured local `mcp_tool` handlers. Each receipt owns a separate native connection; it never borrows the shared RPC connection or a Session's callback authority. Fresh hook and server authorization precede launch and calls and withhold changed-definition results. Filtered discovery and schema validation precede tool effects. Event JSON and typed argument templates, strict unambiguous textual decisions, default-private receipts and per-Session/per-test once claims use existing hook contracts.

Required hook scope confinement overrides ordinary MCP sandbox opt-out and full-access settings. One absolute handler deadline covers initialization, discovery and calls. Startup cancellation joins initialization/listing ownership; call cancellation or timeout closes the native tree and local proxy before durable receipt settlement and checkout/scratch cleanup. Unacknowledged owners retain unknown evidence. Actual tests prove built-in write denial, separate native connections, typed synthetic input and once behavior without Session admission, schema/filter refusal, malformed/error replies under both fail-closed settings, cancellation and live server/hook-definition changes. A reentrant sampling permission callback successfully calls the same configured server through its dedicated hook connection without a deadlock or human prompt. The real CLI exercises MCP effects and receipt creation without Session startup.

Local validation passes 197 Rust cases across twelve hook/MCP and CLI integration suites, workspace all-target Clippy with warnings denied, all 62 TypeScript SDK tests/typecheck, generated SDK consistency, formatting and all 58 strict OpenSpec items. Cross-spec lint reports zero errors and 21 warnings. Remote MCP hooks, async scheduling, remaining lifecycle delivery, plugins and full native platform acceptance remain open. This is local implementation evidence; the canonical MCP-hook requirement and all P1 milestones remain unaccepted.

## M1.4 memory document foundation (2026-10-09)

The authorized `implement-m1-4-memory-intelligence` change covers the complete memory, code-intelligence and browser-verification milestone. The first core increment adds validated Markdown/YAML documents, kebab-case names, known memory types, one-line descriptions and mandatory nonempty Why/How to apply lines for feedback/project notes. Write admission scans frontmatter and body for credential assignments, known key prefixes, private keys and high-entropy strings longer than 32 Unicode characters; diagnostics omit supplied text. Serialization revalidates directly constructed documents. Deterministic index construction sorts unique names, escapes descriptions and refuses duplicate entries. Index snapshots use the first 200 lines or 25,000 UTF-8 bytes with the canonical truncation notice.

Typed enabled/generate settings default true and honor EnvSource's CYBER_DISABLE_MEMORY flag. Actual configuration loading now rejects invalid memory object/boolean types. Path derivation uses the configured data directory and safe project/global identity components. These functions grant no filesystem authority, create no memory files and expose no memory tool or client route. The existing Paths initializer still creates the empty memory root. Private/recoverable storage, cross-process ownership, tool permissions and toggles, Session context/reconciliation, HTTP/SDK/CLI/TUI, LSP/formatters and browser verification remain required. No canonical requirement or milestone is accepted by this foundation.

Local validation passes 76 Rust cases across core unit, configuration and memory tests, workspace all-target Clippy with warnings denied, formatting, generated SDK consistency and all 59 strict OpenSpec items. Cross-spec lint reports zero errors and 21 warnings. Native CI and all memory runtime/storage acceptance remain open.

## M1.4 directory-bound memory reads and scope ownership (2026-10-09)

Core memory storage now admits project/global directories through retained capability directory handles and no-follow opens. Explicit create admission uses private Unix directories; existing review creates no directories or note/index files. Scope claims create a private lock file and report contention without blocking, including across processes. Actual process-death tests verify OS lock release without adopting stored execution as live. Symlinked scope/note/index/lock paths and Unix hard-linked files are refused; private-file checks and filename/frontmatter identity checks precede content return. Invalid note records report safe diagnostics, while valid metadata is sorted.

Index reads consume at most 25,004 bytes, producing the first 200 lines or 25,000 UTF-8 bytes with the canonical truncation notice; they never open individual note bodies. Note reads are bounded to 1 MiB; catalog metadata is bounded to 4 MiB and enumeration to 4096 directory entries, with explicit refusal rather than partial success. A retained .memory-transaction entry fences read/list/index and is left intact. This is a read-side ownership/fencing foundation: no transaction creation, note/index writes or automatic reconciliation is implemented yet. Unix permissions are enforced locally; Windows ACL privacy and complete native acceptance remain open.

Local validation passes 94 Rust cases across core units, configuration, paths, memory documents and actual storage tests; workspace all-target Clippy with warnings denied, formatting, generated SDK consistency and 59 strict OpenSpec items pass. Cross-spec lint reports zero errors and 21 warnings. Windows CI now includes the portable memory/document/storage suites, including actual process-death lock release. Journaled mutation/recovery, memory tool permissions/toggles, Context Source reconciliation and HTTP/SDK/CLI/TUI integration remain required; all P1 milestones remain unaccepted.

## M1.4 journaled Unix memory mutations (2026-10-09)

Exclusive memory scope owners now prepare bounded note/index writes, updates and deletes in a private synced journal. Commit and explicit recovery verify before/after fingerprints and other valid notes, preflight both destinations before effects and use create-only installation. Exact same-inode two-link staging pairs can be normalized; third aliases, corrupt manifests and changed content retain evidence and refuse acknowledgement. Original note/index inodes remain named in private history, preserving late editor writes. Pending journals fence ordinary reads. Disposal does not imply rollback or successful completion, and process-death recovery acquires fresh scope ownership.

Local macOS validation passes 110 Rust cases across core units, configuration, paths, memory documents, storage and mutation tests, including abrupt owner death, partial installation, user edits, corrupt evidence and alias refusal. Windows mutation admission remains explicitly refused until ACL privacy and durability are implemented. Reviewed conflict-resolution controls, history retention, memory tools/context/client integration and full native acceptance remain required; no canonical memory requirement or P1 milestone is accepted.

Linux CI at 5b2ef2c failed eleven storage tests because capability directory handles use O_PATH, which rejects fchmod. The new mutation sync path would also reject fsync on that descriptor. Both operations now use a readable directory handle opened relative to retained authority. The added regression verifies the original directory remains authoritative after namespace replacement. The correction passes locally; native Linux acceptance is pending the next CI run.

Workspace all-target Clippy with warnings denied, formatting, generated SDK consistency and all 59 strict OpenSpec items pass for this increment. Cross-spec lint reports zero errors and 21 warnings.

## M1.4 built-in memory tool admission (2026-10-09)

The actual built-in host now exposes memory list/read/write/update/delete with managed project/global scope selectors. Scope permission defaults are limited to those selectors and remain subordinate to configured, agent, Session, inherited and hook rules. Content, names and secrets are validated before storage admission. Disabled settings remove the definition and refuse direct dispatch; read-only settings and Plan Mode project list/read definitions and refuse stale mutation calls again at execution. Settings are revalidated after permission approval and inside the blocking worker. Write replaces an existing name; update requires an existing note. Normal output budgets retain bounded model output and managed overflow files.

Blocking storage workers retain scope ownership and are joined through the actual result; cancellation is checked before admission, and completed effects return their receipt. Pending storage journals and cross-process contention refuse dispatch without automatic recovery or tool replay. Actual runtime tests verify permission-time configuration revocation and hook-denial receipts without memory effects. The tool has its own golden and a native Windows CI admission step; Windows mutations still refuse before memory storage creation pending privacy/durability implementation.

Local macOS validation passes 88 Rust cases across memory, golden, built-in tools, ordered/inherited permissions and hook dispatch. All 59 strict OpenSpec items, generated SDK consistency, formatting and workflow YAML validation pass; cross-spec lint reports zero errors and 21 warnings. Memory Context Source baseline/Safe Boundary integration, CLI/TUI/HTTP/SDK/event delivery, reviewed unknown-effect reconciliation, Windows mutation support and full native acceptance remain required. The canonical memory tool contract and all P1 milestones remain unaccepted.

Workspace all-target Clippy with warnings denied also passes for this revision.

## M1.4 owned memory Context Source (2026-10-09)

Runtime Epoch initialization and Safe Boundaries now await an owned host observation. Existing custom hosts retain their synchronous behavior by default; the application host uses its effective tool catalog. Built-in memory observation resolves ancestor deny rules and Plan ceilings, joins a blocking worker and reads only directory-bound bounded project/global index prefixes. It deduplicates global fallback, supplies generation/deduplication/staleness guidance and omits generation guidance in read-only contexts. Missing scopes create no directories. Disabled memory withdraws core/memory; busy, pending or unsafe indexes are unavailable, leaving initial input retryable and retaining an existing Epoch snapshot. Updates use the existing durable ContextUpdated event and preserve the immutable baseline.

The memory tool's canonical-scope regression fails against committed f918310: outside Git, project authorization alone can write the global directory despite a global denial. Tool dispatch now obtains separate global authorization for that fallback, and index observation withholds the alias when global scope is denied or requires approval. Actual runtime tests cover the global approval request, ancestor denial, parent Plan guidance, two-Session delivery, replay, index-only loading, truncation, withdrawal and explicit recovery after interrupted preparation. The older ancestor dispatch regression disables memory so it continues testing tool-time denial rather than the new earlier context gate.

Local validation passes 132 Rust cases across memory/tool/context/permissions/hooks/goldens, server runtime/compaction and application host tests. Formatting, generated SDK consistency, workflow YAML and all 59 strict OpenSpec items pass; cross-spec lint reports zero errors and 21 warnings. Linux CI for ad6d54d completed its workspace Rust job successfully, confirming the directory-handle/storage correction. Full native acceptance for the new context/tool revision remains pending. CLI/TUI/API/SDK memory controls/events, Windows privacy/durability, reviewed conflict/unknown-effect resolution, LSP/formatters/browser and complete M1.4 acceptance remain required. All P1 milestones remain unaccepted.

Workspace all-target Clippy with warnings denied also passes for this revision.

## M1.4 memory CLI and editor review (2026-10-09)

The actual binary now provides memory list/show/path/edit/delete with --global and debug memory. Commands start no models and open no database. Edit/delete honor current memory settings and platform mutation admission. Editor reviews retain scope ownership and the original target/index fingerprints. Fresh capture must match that review before any transaction creation; stale reviews refuse without leaving a replayable journal. A private retained draft outside memory supports repairing invalid frontmatter. Editor commands use parsed executable/arguments without implicit shell evaluation. No-follow, regular-file, bounded UTF-8 reads and hard-link rejection precede validation and the shared journaled commit. Draft directories explicitly use mode 0700 and files 0600; failed, unchanged and successful drafts remain retained.

Actual editor subprocess tests cover quoted arguments, note/index/delete effects, secrets/malformed/changed-identity refusal without raw diagnostics, original/index external edits, editor failure, symlink/hard-link/FIFO refusal, unchanged templates and malformed-note repair. Core tests verify stale target/index review refusal before journal creation. Portable native CLI tests also cover empty review, safe names, settings and unsupported-platform refusal. Draft retention controls, abrupt editor-owner acceptance, reviewed unknown-effect resolution, TUI/API/SDK/events and full Windows memory privacy/durability remain open.

Local macOS validation passes 95 Rust cases across core units/documents/storage/mutations and actual CLI tests. Workspace all-target Clippy with warnings denied, formatting, generated SDK consistency and all 59 strict OpenSpec items pass; cross-spec lint reports zero errors and 21 warnings. New-revision native CI remains required. No canonical memory command contract or P1 milestone is accepted by this local increment.


## Native CI follow-up: memory context and MCP close (2026-10-09)

Run 37925372473 exposed two issues in the preceding context revision. Windows lint rejected Unix-only test helpers that were compiled on Windows; those helpers now have explicit Unix configuration. Ubuntu's actual MCP runtime tests showed that the next model step reopened a Location immediately after an acknowledged close. The close regression reproduced locally against the committed MCP implementation before the lifecycle correction.

Explicit close now pauses admission during the current Turn. Catalog refresh preserves that pause; a newly promoted user input or a new Session may reopen with a fresh connection identity. Failed close retains its existing admission fence. The regression verifies durable/native settlement, absence of cancelled callback replies, and fresh identities on both subsequent Turn and Session admission. Local validation covers 64 MCP runtime cases, four authenticated MCP API cases, 51 server runtime/context cases and 41 memory/context/permission cases, alongside the 95 core/CLI cases above. Native CI for these corrections remains required; no milestone acceptance is inferred from local checks.


## M1.4 authenticated memory HTTP and SDK review (2026-10-09)

The application now serves GET /api/v1/memory?scope=project|global and GET /api/v1/memory/{scope}/{name} over the authenticated Location boundary. Bounded blocking list/read operations retain the shared directory-bound scope claim, resolve repository identity/global fallback and create no absent scopes. Existing manual review remains available when automatic model memory is disabled. Invalid names/scopes, missing notes, contention, pending transactions and aliases produce safe tagged failures without automatic recovery. Shared document/catalog schemas generate TypeScript memory.list/get and MemoryNotFoundError handling; custom hosts report the memory capability unavailable.

Local validation passes 71 Rust cases (three real application TCP tests, 56 server HTTP tests and 12 application host tests) and 65 SDK tests. Workspace all-target Clippy with warnings denied, generated SDK consistency, formatting, workflow parsing and all 59 strict OpenSpec items pass; cross-spec lint reports zero errors and 21 warnings. Native Windows gets a dedicated memory HTTP review test gate. HTTP mutations, durable admission/replay and memory.updated delivery, TUI controls, Windows memory privacy/durability and full canonical acceptance remain required.

For preceding b0c6e37, native Windows lint now passes. macOS CI failed at a five-second test-only MCP discovery polling deadline; the same actual host tests pass locally. The fixture now declares the server required and awaits the existing configured runtime readiness boundary, preserving effective-catalog assertions and reporting native status on failure. This change does not alter production timeouts or claim native acceptance; the next CI result remains required. No P1 milestone is accepted.


## M1.4 durable model memory mutation receipts (2026-10-09)

Running model-tool memory writes now admit an independent durable request before journal preparation. Request identity pins canonical Location/project, note identity, operation and a digest of the complete tool input; memory admission records and public change receipts contain no note body. Atomic writer projection and a unique unresolved-scope constraint prevent borrowing another admission. Completion requires the unpublished owner capability and pinned storage receipt. Completed identical requests replay their receipt without another mutation or notification; mismatched or unresolved requests refuse. Persisted ownership never creates a live writer.

Journal commit now has an acknowledgement boundary after synced note/index verification and the completed marker, before journal archival. Durable acknowledgement failure retains that completed journal and read/write fencing, including for direct CLI callers. Explicit storage recovery can reconcile the files but does not clear unresolved database ownership or grant replay authority. Reviewed database/file reconciliation controls remain required. Successful model-tool commits publish public memory.updated.1 with an independent durable aggregate, canonical Location, project identity and receipt; generated SDK schemas and stream coverage are included. HTTP mutation routes still remain required, along with complete CLI/server observer integration, reviewed unknown effects, TUI controls, Windows privacy/durability and full native acceptance.

Local validation passes 137 distinct Rust cases: three admission/capability/replay tests, 12 core mutation/recovery cases, 24 store migration/backup/event cases, 42 memory/context/ancestor-mode cases and 56 HTTP cases. SDK type checking and all 66 SDK tests pass, as do workspace all-target Clippy with warnings denied, formatting, generated SDK consistency, workflow parsing and all 59 strict OpenSpec items; cross-spec lint reports zero errors and 21 warnings. Native Windows receives a dedicated admission test gate; actual native mutation acceptance remains open.

The preceding bc91ff6 Ubuntu CI exposed an ancestor-mode fixture that expected tool-time refusal despite the new earlier memory-context gate. Its failure reproduces locally before the fixture correction. The tool admission case now disables memory loading, while a new portable actual-runtime regression proves that enabled memory with an unknown ancestor retains pending input without an Epoch, model request or tool call. Native CI for this correction remains required. A local link failed for disk exhaustion; only generated Rust incremental caches were removed, preserving models, backup artifacts and project data. No P1 milestone is accepted.


## M1.4 authenticated memory HTTP mutation and stream delivery (2026-10-09)

The actual application now exposes PUT and DELETE on /api/v1/memory/{scope}/{name}. PUT accepts only a complete Markdown content object and validates bounded, secret-free content with matching safe identity before storage admission; DELETE accepts no body. Fresh mutations honor enabled/generate settings again under the shared scope claim. Joined blocking workers own runtime lifecycle leases so shutdown waits for admitted work even after handler disposal. Completed mutations return the independent durable MemoryChange and deliver memory.updated.1 on the authenticated Location event stream without note bodies or owner capabilities.

HTTP request identity additionally pins the standard method/URI/Location/body digest into memory admission. The existing response cache remains, while missing-cache retries return the committed receipt without repeated note/index/history or notifications. Conflicting body or endpoint reuse refuses across cache gaps. Unknown admissions stay fenced without caching a temporary terminal response. Pre-admission contention returns retryable 503 with the same key. Actual TCP/SSE regressions verify CRUD, exact replay after response-cache disposal, endpoint/key conflict, private notification payloads, matching note/index/history effects, settings/secret/format refusal, busy retry and closed-runtime refusal. A detached blocking-owner regression verifies shutdown waiting and subsequent lease refusal.

Local validation passes 160 distinct Rust cases: six actual application memory HTTP cases, four independent admission cases, 57 server HTTP cases, 51 runtime/context cases and 42 memory/context/ancestor-mode cases. SDK type checking and all 68 SDK tests pass. Workspace all-target Clippy with warnings denied, formatting, generated SDK consistency, workflow parsing and all 59 strict OpenSpec items pass; cross-spec lint reports zero errors and 21 warnings. Windows receives an explicit shutdown-ownership gate alongside its existing actual memory HTTP admission tests. Native Windows mutations, reviewed database/file reconciliation, complete CLI/server observer integration, TUI controls and full canonical/native acceptance remain required. No P1 milestone is accepted.

For preceding d5d1a93, Linux and macOS workspace tests, Windows lint, OpenSpec, SDK checks and all six macOS/Linux builds passed in run 37933100647. Windows native build/tests remain in progress at observation. This confirms the ancestor-mode correction on Linux/macOS without inferring complete Windows confinement or memory mutation support.

## M1.4 shared storage recovery review (2026-10-09)

The shared core now supports read-only recovery inspection and explicit fingerprint-bound recovery with the existing acknowledgement boundary. Review exposes validated proposed content, the original mutation receipt and completion state; its digest binds storage path, scope/journal directory identities, known transaction artifacts, current note/index identities/content and catalog fingerprints. Recovery recaptures before any normalization, installation or acknowledgement. Stale note/index/catalog state, same-byte staging inode replacement and cross-scope review reuse refuse while preserving evidence. Unsafe aliases and corrupted desired content also refuse inspection without normalization. Failed acknowledgement retains the completed journal and requires a new completed-state review.

Local validation passes the full core suite (228 passing Rust test executions, including a spawned child-test execution) and eight actual CLI memory editor tests. Six new recovery-review cases cover partial installation, external edits, catalog/index drift, unsafe/corrupt evidence, scope/inode binding and acknowledgement failure. Workspace all-target Clippy with warnings denied, formatting, strict OpenSpec validation (59 items) and specification lint (zero errors, 21 warnings) pass. This increment adds core interfaces only: user-facing recovery controls, database admission reconciliation, native Windows privacy/durability and complete canonical acceptance remain required. No P1 milestone is accepted.

## M1.4 explicit CLI local recovery (2026-10-09)

`cyber memory recovery [--global]` now exposes the shared read-only review, validated proposed content, original mutation identity, completion status and fingerprint. Absent evidence returns null in JSON and creates no scope/database. `cyber memory recover --review <fingerprint> [--global]` requires writable/enabled settings, retains the scope claim and refuses stale review before any recovery effects. It checks an existing database read-only with SQLite no-follow and immediate contention refusal. Pending scope admissions, corrupt/unknown schemas, database aliases and in-memory/unverifiable ownership refuse without clearing database records or journal evidence. Existing databases predating memory admission are supported.

Local validation passes all 13 actual CLI memory tests, including five new recovery cases and their database/settings/alias subcases, workspace all-target Clippy with warnings denied, formatting and all 59 strict OpenSpec items. Cross-spec lint reports zero errors and 21 warnings. README, roadmap and the local guide distinguish local file recovery from unresolved database/file reconciliation. Native CI for the preceding core increment is running; Windows lint, SDK, OpenSpec and four Linux build jobs have passed at observation. Windows native writes, database admission reconciliation, HTTP/TUI recovery controls, partial staging-link review and full canonical/native acceptance remain required. No P1 milestone is accepted.

## M1.4 durable admission-to-journal correlation (2026-10-09)

New model-tool and HTTP writes persist memory.mutation.journal_bound.1 after synced preparation and before installation. The binding pins the exact transaction receipt and complete intent digest, including before/after note/index hashes and the catalog. Its projector validates retained owner authority and atomically rotates the owner nonce; persisted evidence reveals only the superseded nonce and the new nonce hash. New completion requires the exact bound receipt at sequence two. Historical acknowledged unbound records retain their original projection/replay behavior. Interrupted preparation/binding leaves pending admission and journal evidence without reconstructed execution authority.

Recovery review now reports immutable journal identity separately from current-state review fingerprint. Tests prove correlated physical journal preservation after owner disposal, superseded/fabricated nonce refusal, unbound/mismatched receipt refusal, idempotent binding, public notification only after completion and historical completion compatibility. An actual model-tool flow compares the persisted intent digest to the archived journal bytes; real HTTP CRUD/SSE and cache-gap replay pass with the additional durable sequence.

Local validation passes 68 distinct focused Rust cases (eight admission tests, six actual HTTP cases, 28 memory tool/context cases, 13 core memory cases and 13 CLI cases), workspace all-target Clippy with warnings denied, formatting and all 59 strict OpenSpec items. Cross-spec lint reports zero errors and 21 warnings. Native CI for the preceding CLI increment is running; Windows lint, SDK, OpenSpec, four Linux builds and macOS arm64 build passed at observation. Full native acceptance, fresh reviewed database/file reconciliation, legacy pending-admission review, partial staging-link review, Windows mutations and TUI/HTTP controls remain required. No P1 milestone is accepted.

## M1.4 shared runtime reviewed memory reconciliation (2026-10-09)

The runtime can now review a pinned admission against its physical journal and explicitly reconcile matching evidence under retained scope ownership. A database snapshot binds the complete request, result and aggregate sequence. Recovery compares current storage and database reviews before effects, then atomically records memory.mutation.reviewed.1 with a newly minted owner hash. Persisted execution nonces are never adopted. Completion requires this fresh owner, the expected sequence and exact journal receipt. Stale database/file reviews, mismatched intent, copied journals in another project scope and unbound historical admissions refuse before file effects. Recovery-owner disposal retains unknown fencing and requires a fresh review.

If acknowledgement succeeds but archival fails, the completed database review can finish the retained journal without another completion event or public notification. Six new shared-runtime cases exercise physical journals and actual Store projection: fresh reconciliation, stale file/database state, repeated recovery-owner disposal and nonce refusal, acknowledgement/archive separation, foreign/legacy evidence and copied-scope refusal.

Local validation passes 48 distinct focused Rust cases (14 admission/recovery, six real HTTP and 28 tool/context cases), workspace all-target Clippy with warnings denied, formatting and all 59 strict OpenSpec items. Specification lint reports zero errors and 21 warnings. The native memory CI filter now includes the whole memory subtree. Preceding binding revision CI remains running; Windows lint, SDK, OpenSpec, four Linux builds and macOS arm64 build passed at observation. Host-facing HTTP/CLI/TUI reconciliation with lifecycle/settings gates, legacy pending-admission resolution, partial staging-link review, Windows mutations and complete native acceptance remain required. No P1 milestone is accepted.

## M1.4 authenticated HTTP/SDK pinned-journal recovery (2026-10-09)

GET/POST /api/v1/memory/recovery/{scope} now expose paired physical/database review and explicit confirmation for pinned durable journals. GET preserves Location, returns null for absent evidence, permits manual inspection with generation disabled and exposes no execution nonce. POST accepts only the two review fingerprints, validates bounded digest formats, recaptures under configured scope ownership, repeats fresh mutation settings and retains the owned runtime lifecycle lease through the blocking reconciliation worker. Static routing does not shadow valid project/global note reads. Foreign Location, unbound/unknown evidence, stale database/target reviews, disabled/read-only settings, busy scopes and closed runtime refuse before effects.

Real TCP tests verify authenticated paired recovery, exact public receipt/notification, response-cache replay, cache-gap stale refusal, settings revocation, user-edit preservation, busy same-key retry, shutdown admission and foreign/unbound fencing. The generated SDK adds typed memory.recovery and memory.recover methods and paired journal/admission schemas. Durable recovery-request receipt lookup after response-cache loss, local-only/legacy HTTP recovery, CLI/TUI reconciliation, partial staging-link review, Windows mutation privacy/durability and complete native acceptance remain required.

Local validation passes 82 distinct Rust cases (57 server HTTP/schema, 14 shared admission/reconciliation and 11 actual application TCP cases), all 71 SDK tests and SDK type checking, workspace all-target Clippy with warnings denied, generated SDK consistency, formatting and all 59 strict OpenSpec items. Cross-spec lint reports zero errors and 21 warnings. Preceding runtime reconciliation CI has passed Linux workspace tests, Windows lint, SDK, OpenSpec, four Linux builds and macOS arm64 build; macOS workspace/x64 build and Windows native build/tests remain running at observation. No P1 milestone is accepted.

## M1.4 durable keyed recovery receipts (2026-10-09)

The new memory_recovery_request migration materializes hashed request keys, full request digests and original mutation bindings. Reviewed takeover projects this correlation atomically with fresh ownership; acknowledged archival links a new request through a separate durable event without another completion. Ordinary memory admission and recovery projection reject cross-ledger key collisions. Both HTTP and the shared runtime resolve completed retained identities before additional filesystem effects, including after response-cache loss or journal archival. Unresolved identities remain fenced and never reconstruct a worker or nonce.

GET /api/v1/memory/recovery/requests and SDK memory.recoveryRequest expose original mutation identity, immutable journal and nullable completed result, checked against canonical Location/project. A later explicit review can resolve the same pinned mutation and make its receipt available to earlier retained keys. Unkeyed/historical requests without recorded identity retain stale-review refusal.

Local verification passes 114 Rust test executions: 20 admission/recovery cases, 57 server HTTP/schema cases, 13 actual application TCP cases and 24 storage executions including its child-process case. Six new shared-runtime/ledger cases prove database-reopen persistence, unresolved fencing, normal-key collision rollback, acknowledged request linking, later explicit resolution and no-journal replay. Real TCP coverage verifies cache-loss replay, encoded key lookup, body/endpoint/scope conflicts and completed receipt replay while the first response is deliberately delayed. The delayed-response regression reproduces HTTP 409 before the middleware change and returns the identical acknowledged receipt afterwards. All 72 SDK tests, SDK type checking, workspace all-target Clippy with warnings denied, generated SDK consistency, formatting and 59 strict OpenSpec items pass; spec lint has zero errors and 21 warnings.

For preceding HTTP recovery revision, Linux/macOS workspace tests, Windows lint, SDK, OpenSpec and all six macOS/Linux builds passed; Windows native build/tests remain running at observation. Current-revision native acceptance, CLI/TUI reconciliation, unbound/legacy and partial-link resolution, Windows writes and complete canonical memory acceptance remain required. No P1 milestone is accepted.

## M1.4 CLI paired recovery through a running server (2026-10-09)

Explicit memory recovery --server now reports paired storage/admission reviews through the existing registered TCP server. Confirmation requires the exact storage and database fingerprints plus a retained --key; the CLI sends one authenticated request and never starts a server/model or opens its local database. memory recovery-request --server --key reads the original Location-bound durable result or unknown evidence. Missing registration/credentials and incomplete/invalid confirmation refuse before any memory scope admission. Default local storage review and unresolved-database refusal remain available.

All 15 CLI memory tests pass. New real-binary coverage uses an actual authenticated application TCP server and file-backed database, proving paired reconciliation, stale database review and read-only refusal before effects, wrong-password refusal, cache-loss receipt replay, encoded key lookup, foreign Location refusal and exactly one completion. Missing-registration/confirmation cases prove no database, password, registration or memory scope creation. Workspace all-target Clippy with warnings denied, formatting, generated SDK consistency and all 59 strict OpenSpec items pass; specification lint reports zero errors and 21 warnings. Existing Windows CI includes the portable CLI admission tests; the actual write/reconciliation test is Unix-only.

For preceding receipt revision f450709, Windows lint, SDK, OpenSpec, all four Linux builds and macOS arm64 build pass; Linux/macOS workspace tests, macOS x64 build and Windows native build/tests remain running at observation. Current-revision CI, socket-only CLI transport, TUI controls, legacy/unbound and partial-link reconciliation, Windows writes and full canonical acceptance remain open. No P1 milestone is accepted.

## M1.4 TUI memory review, deletion and paired recovery (2026-10-09)

/memory and /memory global now open a generation/Location-bound API panel with validated metadata, safe diagnostics and selected note reads. Deletion requires opening the note and a separate Y confirmation. V inspects a pinned journal even when ordinary reads are fenced; C/Y confirms the displayed content and paired storage/database fingerprints. Confirmed recovery retains a request key and sends one raw request; K performs read-only durable receipt lookup. Failed requests clear actionable reviews. Scope, Session/Location changes and dismissed overlays reject late responses. Completion must match the reviewed mutation and immutable receipt. Terminal control characters are escaped before rendering.

The embedded raw client now preserves explicit request headers and returns real status/body for one router request, including non-success. Its regression fails before the fix on HTTP 409 and passes with the retained key, Location and custom header afterwards. The real authenticated TCP/application/file-database TUI test exercises recovery, receipt lookup, refresh/read, cancelled deletion and confirmed deletion without model work. State/render/parser tests cover missing or foreign response envelopes, malformed metadata, control text, stale generations, scope/Location changes, dismissal and mismatched terminal receipts.

Local verification passes 95 client/TUI tests (three client and 92 TUI), workspace all-target Clippy with warnings denied, formatting, generated SDK consistency and all 59 strict OpenSpec items. Specification lint reports zero errors and 21 warnings. Native Windows CI now includes portable memory panel/state/parser and embedded raw identity tests; actual memory mutation remains Unix-only. Preceding CLI revision a146f85 passes the complete CI matrix, including native Windows. Current-revision CI remains required. TUI editing, persistent client request-key retention, legacy/unbound and partial-link reconciliation, Windows writes, complete native memory acceptance and the broader P1 goal remain open. No P1 milestone is accepted.


## M1.4 detached memory edit snapshots (2026-10-09)

The shared storage core now exposes a bounded, read-only `MemoryEditReview` containing the original Markdown and a fingerprint bound to the scope path/identity, note name, note/index content and file identities, and valid catalog note contents. The review survives release of the scope claim. Reviewed write/delete preparation reacquires authority through the caller's scope and checks the fingerprint before preparation and again before journal creation. Inspection rejects a note changed between fingerprint capture and original-text reading. Existing interactive CLI editing remains available through its retained claim.

Local verification passes all 66 core library, mutation and storage tests (38/16/12), workspace all-target Clippy with warnings denied, and all 59 strict OpenSpec items; specification lint reports zero errors and 21 warnings. Four new regression tests cover explicit reviewed write/delete, stale target/index/catalog preservation, identical-content replacement files, foreign scope/note reviews, read-only missing-note inspection, invalid fingerprints, secrets and oversized admission. Conditional HTTP/SDK routes and TUI editing are still unimplemented. Native Windows writes and full memory acceptance remain open; this increment does not accept M1.4 or close P1/P0.


## Native Windows TUI test-helper lint correction (2026-10-09)

CI run 37986423537 for 9267ad2 failed native Windows Clippy because `apply_memory_response` and `perform_memory_action` were unconditional test helpers used only by the Unix-only real memory mutation test. Windows build was then blocked by its explicit lint prerequisite; the other nine jobs passed. Both helpers now use the caller's `cfg(unix)` condition. Local verification passes all 92 TUI tests, TUI all-target Clippy with warnings denied, formatting and diff checks. The existing native Windows all-target lint job is the regression gate; its new-run result remains required before claiming Windows acceptance.


## M1.4 conditional HTTP/SDK memory edits (2026-10-09)

Authenticated Location-scoped GET `/api/v1/memory/edit/{scope}/{name}` now exposes shared bounded original Markdown and edit fingerprints without creating absent scopes. PUT accepts optional `review_fingerprint`; DELETE accepts a single optional `X-Cyber-Memory-Review` header. Invalid fingerprints refuse before scope creation. Fresh conditional mutations verify the current scope before durable admission and use reviewed preparation to check again before journal creation; changed note/index/catalog evidence or a foreign scope refuses without overwriting user content. Existing settings and lifecycle ownership remain enforced. Read-only review remains available when automatic memory is disabled. Identical completed requests replay their durable receipts after their reviews are consumed and the HTTP response cache is cleared. The DELETE review header is bound into request identity, including duplicate values, so changed or omitted reviews cannot reuse a prior request's key. Requests without that header retain their existing hash format.

OpenAPI and the generated SDK expose `memory.editReview` and optional conditional PUT, with the DELETE header documented in OpenAPI and carried through SDK options. The guide and SDK README explain reviewed saves, conflicts and retained request keys. Local verification passes all 57 server HTTP tests, 17 application memory tests and 74 SDK tests, workspace all-target Clippy with warnings denied, formatting, generated SDK consistency and all 59 strict OpenSpec items. Specification lint reports zero errors and 21 warnings. Five new authenticated application tests cover read-only routing/authentication, stale user files without durable admission, consumed-review replay and changed header identity, missing scopes, malformed/duplicate fingerprints, foreign scope and disabled settings. Two SDK tests cover typed review and conditional request transport without conflict retries.

CI run 38000648342 for the preceding 11b4030 revision now passes native Windows lint; native Windows build and remaining jobs were still running at observation. This increment still requires its own CI results. TUI editing, persistent client request keys, Windows memory writes, complete native/canonical memory acceptance and the full P1 goal remain open. P0 baseline work is preserved.


## M1.4 reviewed TUI multiline memory editing (2026-10-09)

E now fetches the shared edit review for an opened note and enters a bounded multiline Markdown draft using the existing Unicode composer. Typing/paste supports navigation, line breaks and CRLF normalization; terminal controls are escaped for rendering. Ctrl-S validates the complete document, preserves its note name and opens a separate Y confirmation. The confirmed action sends one raw conditional PUT with the pinned fingerprint and explicit request key. Completion must match the request's Location, scope, note and write operation. The panel keeps controls/confirmation visible separately from scrolling content and follows the editor cursor.

Failed saves retain content and the displayed request key, clear actionable edit authority, and require Ctrl-R fresh review before another save. Fresh review displays the current original without replacing the draft. Esc pauses editing and retains the draft; dismissal and Session/Location/scope changes retain it without foreign save authority. E resumes only in the original Session/Location/scope; X/Y explicitly discards. Confirmed acknowledged saves clear the completed draft. This retention is in memory for this TUI process only; persistent draft/request-key retention remains required.

Local verification passes all 99 TUI tests, workspace all-target Clippy with warnings denied, formatting, generated SDK consistency and all 59 strict OpenSpec items; specification lint reports zero errors and 21 warnings. Seven new portable tests cover Unicode editing, bounded paste, secret/name refusal, escaped controls, explicit confirmation, failed-draft retention, fresh-review rebinding, dismissal/foreign-Location refusal, explicit discard, and malformed or foreign edit/write responses. The existing authenticated TCP/application/file-database test now also covers cancelled and confirmed saves, committed Unicode content, stale-save refusal preserving an external edit, retained drafts and subsequent explicit discard/deletion. The Windows memory test filter includes the portable editor/state/parser tests; actual memory mutation remains Unix-only.

The preceding API revision 05b375e passes native Windows lint, SDK, specification and Linux build jobs at the latest observation; remaining native jobs were still running. This editor revision requires its own CI evidence. Persistent client drafts/keys, full memory recovery coverage, Windows storage, complete canonical/native M1.4 acceptance and broader P1 requirements remain open. P0 baseline work is preserved and P0 is not declared closed.


## M1.4 private memory client checkpoint storage foundation (2026-10-09)

The shared core now provides `MemoryClientStore` for a separate private `<state>/memory-client` checkpoint. Existing absent storage remains absent; explicit creation creates only a private child of an existing state directory. One retained exclusive lock owner prevents another client from overwriting its checkpoint. Reads are bounded and refuse symlinks, hard links, non-private files and dangling aliases without permission repair. Saves use a private exclusive temporary file, file sync, atomic installation and child/parent directory sync. The retained checkpoint includes file identity as well as bytes, so identical-content replacements still refuse. Changed content or replaced checkpoint/lock/client/state directory identities preserve existing files and refuse further writes. Staged files remain evidence; reopening after abrupt process death reads only the installed checkpoint and never adopts staged intent.

Local verification passes all 74 selected core tests (38 library, eight client-checkpoint including the child worker, 16 mutation and 12 storage), workspace all-target Clippy with warnings denied, formatting, generated SDK consistency, workflow YAML parsing and all 59 strict OpenSpec items. Specification lint reports zero errors and 21 warnings. Seven client-storage regressions cover restart/exclusive ownership, changed contents, bounded writes, aliases/privacy, replaced binding identities, identical-byte inode replacement, dangling aliases and abrupt owner death with staged evidence preservation. Native Windows CI now runs the explicit unsupported-privacy refusal test; native checkpoint privacy/durability remains unimplemented and refuses before creation.

This is a storage foundation, not an enabled TUI persistence feature. The TUI still retains memory drafts/keys only within its process. Validated scoped checkpoint schemas, saving intent before dispatch, restoring drafts without review authority, durable outcome/key handling, and complete native integration remain required. The preceding TUI revision 7e85ad4 passes Windows lint, Ubuntu Rust, SDK, specifications and all six platform build jobs at observation; macOS Rust and Windows build jobs remain running. This revision still needs its own CI evidence. No P1 milestone is accepted; P0 baseline artifacts/models/services remain untouched and P0 remains open.


## M1.4 durable TUI memory drafts and confirmed request intent (2026-10-09)

The TUI event loop now restores and checkpoints validated memory client state through the private shared store. Its versioned schema retains bounded draft text, original Session/Location/scope, and bounded immutable save/delete/recovery request evidence. Draft review fingerprints, pending actions and confirmations are not restored. Confirmed requests must match the active generation/context, be recorded with their exact key, and receive a successful durable checkpoint before dispatch. Restored request evidence produces no automatic action. Recovery keys support scoped K receipt lookup; saved/deleted request outcomes still need complete lookup/management controls.

Every terminal/server event passes the checkpoint gate before command dispatch. Failed, unsafe, changed, malformed or unavailable checkpoints withhold fresh memory effects and orderly exit while unsaved local work remains, preserve original files/drafts, and clear refused save authority. Unchanged failed checkpoints do not repeatedly stage writes on background ticks; explicit activity can retry repaired storage. Parsed malformed state is never replaced automatically. Acknowledged save/delete intents are retired, acknowledged saves clear their drafts, and explicit discard persists a tombstone. Recovery request evidence remains retained. Only recent keys are rendered to keep confirmations visible. Multiple concurrent owners and unsupported native privacy refuse; complete multi-client/native retention remains open.

Local verification passes all 106 TUI tests, workspace all-target Clippy with warnings denied, formatting, generated SDK consistency and all 59 strict OpenSpec items; specification lint reports zero errors and 21 warnings. Seven new local tests cover schema/version/identity/size validation, real-file restart with no review/actions, storage-edit dispatch/exit fences, malformed saved state preservation, stale generation and durable discard, failed-checkpoint throttling with explicit repair retry, and restored recovery lookup without foreign authority. Native Windows also has an explicit no-creation retention refusal test. The authenticated TCP/application/file-database workflow now uses the same checkpoint gate before recovery/save/delete dispatch and after responses, covering cancellation, acknowledged effects and stale user-file preservation.

The preceding 7581888 storage revision passes SDK, specifications, Windows lint, Ubuntu/macOS Rust and all six platform build jobs at observation; Windows build remains running. This integration requires its own CI evidence. Complete save/delete outcome lookup, retained-record reconciliation, multi-client/Windows privacy and durability, canonical/native memory acceptance and the full P1 goal remain open. Cold-start budget revalidation remains required as the TUI integration settles. P0 baseline work is preserved and P0 is not declared closed.


Retained memory HTTP save/delete outcomes are now available through authenticated scoped read-only API/SDK lookup. Local evidence passes 23 memory runtime, 57 server HTTP, 19 real application memory and 76 SDK tests, workspace Clippy and 59 strict specification items. Corrupt and well-formed mismatched ledger evidence refuses; actual cache-loss/disabled-memory lookup emits no new update and creates no scope. TUI fingerprint-bound reconciliation and record controls, native Windows mutation/checkpoint privacy, multi-client retention and full canonical memory acceptance remain open. This increment accepts no complete P1 contract or milestone.


TUI retained memory request lookup/management now has local evidence from 113 TUI, 38 server library and 57 server HTTP tests. The real authenticated TCP workflow discards successful save/delete acknowledgements, restores checkpoint state after restart and reconciles their exact durable identities without another mutation. Unknown/foreign/malformed/late outcomes preserve records; newer draft edits survive completed save reconciliation; canceled forgetting preserves records and confirmed forgetting persists across restart. Shared fingerprint framing has a fixed regression and native Windows CI selection. Windows private durable storage, multi-client retention, legacy recovery and complete canonical/native acceptance remain open; no complete P1 contract or milestone is accepted.


Windows memory handle evidence now has a shared core verifier and CI-selected native tests for private ACL/owner checks, bounded SID/token inputs, full file identity, replacements, hard links and junctions. The regular-file guard is wired into actual memory reads and directory identity checking. Local evidence is 74 selected Unix core executions, workspace Clippy, strict specifications and a Windows-target type/lint harness for the actual backend source and native unit tests. The harness is not native execution or a full Windows core build. Private creation, durable mutation/checkpoint integration and native acceptance remain open; no complete P1 contract or milestone is accepted.


Windows private child creation now has create-only handle-relative directory/file primitives with protected creation-time security and explicit type/parent/name checks. Ten native backend unit tests type-check/lint in the Windows-target source harness; three creation regressions exercise collisions/user-byte preservation, moved parents/reparse refusal and invalid names without child effects. Local Unix core verification passes 74 tests, workspace Clippy and strict specs. Focused native execution is now selected immediately after Windows lint and remains required through its dependency result. Storage/client creation integration, durability/path binding and whole native acceptance remain open; no complete P1 contract or milestone is accepted.

## M1.4 Windows existing private child opening (2026-10-09)

Existing private directories/files now open relative to retained parent handles with FILE_OPEN and no creation security descriptor. Missing children cannot be created; existing bytes and permissions are not repaired or truncated. Successful opens must report existing-object disposition and pass full private owner/DACL, type, reparse and regular-file alias verification before caller access. Read handles grant no data writes; write handles preserve bytes until explicit caller writes. Four native regressions cover read-only/write behavior and case-insensitive lookup, moved-parent identity and missing children, broad permissions/wrong types/hard links with retained ACL and byte evidence, and junction-child refusal. Invalid-name coverage also exercises open admission.

The preceding `8ca56ef` revision's [Windows lint and native backend job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/38007446836/job/114079403310) now passes all ten backend tests, establishing native execution for handle verification and private creation. That run's full Windows build and several other jobs were still running at observation. The new opening tests require their own native execution; Windows-target source-harness lint is narrower evidence. Storage/startup integration, caller path binding, durable namespace installation and client checkpoint support remain required before Windows mutations are enabled. No complete P1 requirement or milestone is accepted, and P0 local-model artifacts/services remain preserved.

Local validation for opening passes 74 selected core/memory tests, workspace all-target Clippy with warnings denied, formatting, and all 59 strict specification items. Specification lint reports zero errors and 21 warnings. The Windows-target harness includes the actual backend and all fourteen native unit tests; it type-checks/lints successfully but does not execute Windows APIs or compile the full core integration.

## M1.4 Windows retained namespace sources and create-only rename (2026-10-09)

Private file/directory installation now has an exclusive retained source guard opened synchronously relative to a private parent. Admission compares the caller's expected full identity and validates private security/type/alias evidence; the guard denies competing write/delete sharing and exposes its borrowed handle for content review. Handle-relative rename uses one bounded component, a pointer-aligned counted native buffer and ReplaceIfExists false. Existing destinations refuse without replacement; successful installation verifies private security and exact full identity through the destination handle. Unknown postconditions retain the source guard/evidence without rollback or cleanup. Six native tests cover write/delete/rename exclusion and release, collisions and unsafe names, stale or aliased sources, junction destinations, moved destination handles, directory history moves and native buffer layout/bounds. These are namespace primitives; they do not yet integrate journal recovery or prove durable directory installation.

The preceding `f3259e9` [native backend run](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/38007901124/job/114080851393) passes twelve tests and fails two test setup steps: a directory move with an open descendant note returns access denied, and mklink junction setup fails without command diagnostics. The directory fixture now closes the note after capturing its full identity, retains the directory handle through the move, and compares the later opened note with the stored identity. Junction arguments now use native path-component joins rather than an embedded slash; setup captures stdout/stderr on failure. The junction cause and corrected native execution remain unproven until fresh CI. No checks were disabled, and no complete P1 contract/milestone is accepted. Windows mutation/checkpoint admission remains disabled pending durability, path/content binding and full lifecycle integration. P0 local-model services/artifacts are preserved.

Local validation passes 74 selected core/memory tests, workspace all-target Clippy with warnings denied, formatting, workflow YAML parsing and all 59 strict specification items; cross-spec lint reports zero errors and 21 warnings. The actual Windows backend source and all twenty native unit tests pass Windows-target harness type/lint checks. This is not native execution or full core Windows compilation; fresh native acceptance remains required.

## M1.4 Windows private startup and memory read admission (2026-10-09)

Windows startup now creates/adopts the memory root through native private admission rather than generic default-security directory creation. Project/global scope admission uses private handle-relative directory creation/opening; existing review creates no scopes or lock files. Scope ownership privately creates the lock or opens an existing verified lock without truncation or permission repair, then retains the OS exclusive claim. Native read-only note/index opening verifies private ACL/owner, regular type, no reparses and a single file link before bytes are returned. Unsafe existing roots, scopes, locks, notes and indexes refuse without repair, and catalog diagnostics do not include note contents. Seven added Windows integration tests cover startup privacy/no scopes, unsafe startup-root preservation, existing scope/root/lock refusal, existing-review no effects, note/index refusal and junction confinement. Existing cross-process ownership, killed-owner release, budgets, document parsing and hard-link read coverage are preserved, using protected native note fixtures on Windows.

The preceding `ea39a4e` [native Windows backend job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/38008368575/job/114082347703) passes all twenty unit tests: handle/ACL/privacy verification, creation, existing opening, retained source sharing and create-only rename. Both previously failed fixture setups now pass with their intended checks. That evidence establishes these backend boundaries, not the new storage/startup integration or full native durability. The new focused storage/startup step follows the required early Windows lint/backend gate. Actual memory, env/ID/version/paths sources and real storage/startup integration tests pass a Windows-target type/lint harness without a substituted memory error; the harness is not Windows API execution or full core compilation. Windows mutation and private client checkpoint workflows remain gated until durable namespace installation, full path/content binding and journal/client integration are complete. No P1 requirement or milestone is marked fully accepted; P0 services/evaluation artifacts remain preserved.

Local integration validation passes 152 selected Rust tests: 90 core/document/storage/mutation/client/startup cases and 62 actual tool/context/CLI/application memory cases. Workspace all-target Clippy with warnings denied, Windows-target actual-memory/startup source lint, formatting, workflow parsing and all 59 strict specification items pass; cross-spec lint reports zero errors and 21 warnings. Fresh native storage/startup execution remains required.

## M1.4 Windows native flushing, durable rename and reliable native gates (2026-10-09)

The backend now requests normal synchronous native flushing of data, metadata and storage cache through the exact retained private file/directory handle. Private full identity is verified before/after; both native return and completion status must succeed. Read-only, unsafe, aliased or unsupported objects fail without no-op/data-only/no-sync acknowledgement. Retained namespace sources also retain their original parent handle. Durable create-only rename preflights object/source-directory/destination-directory flushing, performs exact-source rename without replacement, then flushes object and both directories before success. Six native tests cover file/directory normal flushing, read-only/unsafe/aliased refusal, durable file/history installation, preflight and collision preservation, and injected post-rename flush failure with installed evidence retained and no rollback. Shared Windows transaction directory syncing now uses the native primitive; mutation/client checkpoint gates remain closed pending full integration and native durability acceptance.

Detailed logs for the preceding `8138c4e` [Windows job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/38008979337/job/114084301520) contradict its successful storage/startup step status: twenty backend tests and three startup tests pass, while storage reports nine passes and eight failures at fixture ambient-directory reopening (Windows sharing violation 32). The later startup command returned success and hid the earlier native failure. cap-primitives source confirms ambient directory handles omit delete sharing, conflicting with active native scope handles carrying DELETE access. Fixtures now reopen using explicit read/write/delete sharing; privacy checks and all storage cases remain intact. Corrected native execution remains required.

Storage/startup now run as independent steps. Every Cargo invocation in multi-command Windows gates has an explicit exit-code propagation check, covering the other native P1 batches too. New tests inspect the actual workflow and, on Windows, execute its batch guards with an initial failing native command followed by would-be successful commands. The structural regression fails on the unchanged main `8138c4e` workflow at the masked storage command. Native behavior still needs the new CI run; green aggregate status alone is not accepted as test evidence. No full P1 contract/milestone or P0 exit is declared complete; local-model services/evaluation artifacts remain preserved.

Final local validation passes 91 selected core/document/storage/mutation/client/startup/CI-contract tests, workspace all-target Clippy with warnings denied, formatting, workflow parsing and all 59 strict specification items. Cross-spec lint reports zero errors and 21 warnings. The actual Windows memory/startup source harness includes all twenty-six backend unit tests plus real storage/startup and CI-contract tests, and passes Windows-target type/lint checks; native flush, corrected fixture execution and behavioral failure propagation remain required.

Native Windows evidence update (2026-10-09): inspected `fb78e5a` job [114088173271](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/38010197409/job/114088173271) proves 26 backend, 17 storage, three startup and two CI-propagation tests pass. Normal private file/directory flushing, durable retained rename, corrected fixture sharing and native first-failure propagation are verified at these tested boundaries. Shared review/client identity now captures actual native handles with full Windows identifiers and same-handle bounded review reading; local replacement tests pass, fresh native identity execution remains required. Windows journaled mutation/client checkpoint lifecycle integration remains open. No complete P1 contract or milestone is accepted by this narrower evidence; all 225 contracts stay in scope and P0 local-model evaluation remains open.

Windows journal route update (2026-10-09): actual storage preparation/recovery now selects private native create/open/flush, retained hash/full-ID verified durable create-only target capture/installation, and exclusive create-only history archival. Windows rejects aliases rather than applying Unix link normalization. Five real-pipeline native tests are included in the early memory library gate and pass target type/lint checks; execution remains pending. The preceding `e60a420` job [114092272186](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/38011490066/job/114092272186) executes 28 library, 19 storage, three startup and two CI-propagation tests successfully. Public Windows mutations/recovery remain gated pending complete caller/path/intent binding, crash/lifecycle acceptance and private client checkpoint integration. All 225 canonical P1 requirements remain in scope; no complete milestone is accepted and P0 local-model evaluation remains open.

Scope/journal binding update (2026-10-09): retained data/root/scope handles and held-lock checks now guard read/review/preparation/recovery admission, with live named-journal/intent identity and intent-byte checks at commit boundaries. Six exact storage and live-journal regressions fail against unchanged main `f00682b` in an actual-memory Unix source harness and pass in the current core suite. Windows binding/intent tests pass source-target lint; fresh native execution is required. Prior `f00682b` native job [114093545718](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/38011900589/job/114093545718) proves all five real private journal cases execute successfully within 33 library, 19 storage, three startup and two CI-contract passes. Full native namespace race protection, persisted target identity, crash acceptance and client checkpoint integration remain open; public Windows mutations stay gated and no full P1 contract/milestone or P0 exit is accepted.

Windows namespace pin update (2026-10-09): claimed data/root/scope and lock handles now exclude delete sharing; ordinary private content capabilities exclude delete access so independent scopes/readers/claims can coexist. Live journals similarly pin their names until disposal/exact-source archival. Three actual-pipeline native exclusion/compatibility/release cases and strengthened held-lock assertions pass Windows target lint; native execution remains pending. Prior `cf46650` [job 114095501559](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/38012531328/job/114095501559) executes 35 library, 21 storage, three startup and two CI-contract tests successfully. Persisted target identity, immutable intent/crash boundaries, private client checkpoints and complete native lifecycle acceptance remain required before activation. No complete P1 contract/milestone or P0 exit is accepted.

Immutable Windows intent update (2026-10-09): prepared/recovered capabilities now retain the exact intent object under an exclusive full-ID checked source guard, with admission revalidated under that guard and release before archival. Existing replacement/content tests now prove blocked attacks, unchanged evidence, disposal and fresh recovery; a new pre-existing-writer case proves admission refusal/release. Native execution remains pending for this revision. Prior `1a7e611` [job 114097493765](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/38013156858/job/114097493765) proves all 36 library, 23 storage, three startup and two CI-propagation tests pass, including directory/lock/journal namespace pins and compatible normal flushing. Persisted source/target identity across crashes, client checkpoints and complete native lifecycle acceptance remain required; public Windows mutations stay gated, full P1 remains active and P0 local-model evaluation remains open.

Persisted Windows identity update (2026-10-09): version-2 intent records full directory/intent, original/staged note/index and catalog IDs; recovery validates exact original/captured/installed slots and source retention uses recorded IDs. Five native cases cover identical-byte file replacement, directory-context replacement with preserved files, legacy/incomplete refusal and owned killed-process recovery/helper. Windows source lint and 162 local tests pass; fresh native execution remains required. Prior `85defc5` [job 114098543465](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/38013483161/job/114098543465) proves 37 library, 23 storage, three startup and two CI-contract passes, including immutable intent admission and release. Unix version-1 compatibility remains supported. Terminal/crash ownership, legacy/relocation reconciliation, native checkpoints and full canonical acceptance remain open; public Windows mutations remain gated, no P1 milestone is accepted and P0 evaluation remains open.

Windows terminal ownership update (2026-10-09): exact persisted installed/catalog/original IDs and bounded hashes now admit readonly write/delete/rename-excluding handles through acknowledgement; the completed marker is frozen and journal descendants release only before exact-source archival. Four native pipeline cases cover attacks, pre-existing writers, failure/recovery release and catalog additions. Windows source lint, 162 local tests and workspace lint pass; native runtime acceptance remains pending. Absent filenames remain unreserved: post-callback changes preserve user files/completed evidence and fence disposal without rolling back a possibly acknowledged receipt. Prior `ec1efea` [job 114101493134](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/38014440938/job/114101493134) proves 42 library, 23 storage, three startup and two CI-contract passes, including persisted-identity replacement and killed-owner recovery. Full crash/disposal reconciliation, Windows client checkpoints and complete canonical acceptance remain open; public native mutations stay gated, no full milestone is accepted and P0 evaluation remains open.
