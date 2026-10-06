# P1 implementation status

The active goal covers all P1-tagged product contracts, not only the five milestone titles. Implementation starts with M1.1. P0 local-model evaluation continues independently and remains an open release gate.

## Delivery sequence

- [ ] M1.1: Windows sandbox and network enforcement; classifier, permission ceilings, mode cycling and pending state.
- [ ] M1.2: Subagents, schema validation, worktree isolation, background results and concurrency limits.
- [ ] M1.3: Hooks, out-of-process plugin host, MCP authentication/search/resources/prompts and complete skills support.
- [ ] M1.4: Memory, code intelligence, formatters and isolated browser verification.
- [ ] M1.5: Setup import, ACP and VS Code integration.
- [ ] Remaining P1 contracts: local web client, providers/credentials, API and SDK surfaces, observability/budgets, configuration, exec and TUI functionality.
- [ ] Exit evidence: native Windows escape tests, reference import compatibility, versioned coding benchmarks and internal quality gate.

M1.1 design is in [the OpenSpec change](../../openspec/changes/implement-m1-1-sandbox-modes). Native Windows enforcement must be proven on Windows; compilation alone is insufficient. Comparative quality claims require published measurements.

## M1.1 progress

The evaluator runtime foundation is implemented: tool-free bounded inference, strict allow/block replies, a durable `permission.auto_decided.1` event, per-Drain consecutive-block tracking, and hidden usage/cost accounting in replay, SQL session totals and evaluation budgets. Runtime tests cover valid decisions, malformed responses, unavailable models, replay, forced event-write failure, repeated blocks, timeout, cancellation and untrusted context.

The first Bash critical-removal boundary is implemented. Literal removals, nested shell `-c`/`eval`, canonical aliases and unresolved targets are checked before automatic/saved allows. Manual prompts show a danger warning and cannot be approved by another request's always cascade. Tools, TUI and server validation passes 186 tests, with release Clippy passing. Wrapper parsing now consumes `env`/`exec` option values, preserves literal equals signs in operands, distinguishes `command -v`/`-V` lookups, and refuses dynamic or unsupported wrappers. Directory-changing `env` options make relative removals unresolved. `find -delete` preserves roots after global options. This extension passes 93 tools tests, release workspace Clippy and strict OpenSpec validation. This does not close critical-removal coverage: additional wrappers, advanced binding/flow cases, PowerShell and full inline-language coverage remain.

Shell stdin analysis now checks direct/wrapped heredocs and here-strings, including known shell consumers in a pipeline. It separates ordinary quoted data from executed source, preserves shell `-c`/script-file behavior, handles the grammar's adjacent here-string descriptor edge case, and marks expanded input or dynamic command names unresolved. This extension passes 94 tools tests, release workspace Clippy and strict OpenSpec validation. This is partial shell-context coverage; advanced binding/flow cases and stream transformations remain open.

Scoped Bash binding analysis resolves literal scalar targets and command names, follows function bodies at invocation, separates subshell/substitution state and preserves outer argument expansion before inline assignments. `eval` and child shells receive their appropriate assignment context. Branches, loops, reads, arithmetic, arrays, environment changes and indirect callback/source contexts cannot turn uncertain bindings into an automatic allow. Scalar growth and function fan-out are bounded across nested scopes; exhausted analysis remains unresolved. This extension passes 99 tools tests, release workspace Clippy and strict OpenSpec validation. This extends the guard; it does not close full Bash coverage or the remaining PowerShell/inline work.

The TUI cycles default → accept-edits → plan → auto and displays effective/pending modes with interrupt available. Modes are pinned durably at Turn start through tool settlement; a switch during inference cannot change that response's tool mode. Legacy event replay preserves start-time selection. Rapid TUI selections are serialized/coalesced, with stale-response protection and separate switching/pending labels. Command/picker bypass selection requires explicit confirmation. This increment passes 204 server/tools/TUI tests (one opt-in measurement ignored), 37 SDK tests, SDK type checking/generation checks, release workspace Clippy and strict OpenSpec validation. Org-policy mode restrictions remain open.

Accept-edits filesystem auto-approval now requires a whole-command proof of literal operands and known option semantics. Assignments, expansions, wrappers, redirections, background execution, recursive removals and unsupported options retain ordinary approval handling. Proven paths resolve existing symlinks before parent traversal, including copied filenames inside destination directories, so protected aliases and workspace escapes cannot obtain automatic approval. Recursive copies, directory moves and more option semantics remain open; this does not close the full accept-edits requirement. This increment passes 105 tool tests, release workspace Clippy and strict OpenSpec validation. The dynamic-path regression first failed against unchanged main by executing an unapproved temporary-file write.

Initial Python/JavaScript inline guards parse executable source with dedicated grammars, follow import/require aliases and bounded literal bindings, and apply shared critical-path checks. They cover standard removal APIs, Path constructors and joins, implicit parent removals, escaped literals and nested eval/shell execution. Printed code remains data. Unknown calls, imports, targets, execution options and mutation contexts require individual confirmation; uncertain branch/eval shadowing cannot restore builtin call proof. OS isolation is still tested after explicit runtime approval of an inline protected write. PowerShell, additional interpreters/stdin sources, higher-order dispatch, module-loading trust and full protected/irreversible mutation facts remain open. Classifier allows remain disconnected. This increment passes 117 tool tests, release workspace Clippy and strict OpenSpec validation. The direct Python-removal regression first failed against unchanged main without executing the dangerous command.

Initial PowerShell command-source guards now use a dedicated AST grammar. They resolve case-insensitive removal aliases, module-qualified Remove-Item, bounded scalar bindings, literal target arrays and supported static .NET deletion APIs, while quoted output remains data. Unknown source/options, profile loading, providers, globs, deferred blocks, indirect dispatch and uncertain control flow require individual confirmation. Common command parameters invalidate binding proof until their effects are established. Encoded/stdin sources, advanced bindings, pipeline forwarding, command/module trust, native shell dispatch and full protected/irreversible mutation facts remain open; classifier allows remain disconnected. The initial regression failed against unchanged main at `5351154` without executing the removal. Windows CI now includes native drive/verbatim-path analysis. Local validation passes 122 tool tests, workspace release Clippy and all 56 strict OpenSpec checks; the native Windows path guard also passes at `9f5adcc` ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37413476614)).

PowerShell encoded commands now decode bounded Base64 UTF-16LE before AST inspection, including documented short flags and startup flags following the payload. Literal stdin uses the PowerShell grammar for `-Command -` and `-File -`; overridden sources, other descriptors, profile/startup uncertainty and unproven forwarded input retain individual confirmation. A Bash grammar omission of a standalone stdin dash before a heredoc is recovered only from the exact literal gap. Malformed encoding, invalid UTF-16, duplicate source options and script-file arguments cannot become automatic allows. Local validation passes 126 tool tests and workspace release Clippy. At `6bdca22`, the native Windows test step passes both drive/verbatim-path analysis and harmless PowerShell encoding/stdin contracts ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37415921548/job/112114385433)); the overall run was still in progress when recorded. This does not complete advanced PowerShell flow, producer/module trust or protected/irreversible mutation facts.

Native Windows compilation and listener lifecycle now pass. The CI job at `a54aeb9` verifies workspace/test compilation, executable builds, CLI smoke, unsupported Unix-bridge refusal, TCP proxy transport and server registration/health, exclusive ownership and graceful shutdown. Unsupported Unix listener flags fail before startup, registration advertises no Unix socket, and cleanup preserves pre-existing socket-path files. This does not implement authenticated `service stop`.

The Windows-only `--job` helper establishes process-tree ownership before spawning. The native job at `5351154` passes all checks, including four helper tests covering live-grandchild cleanup after normal and forced termination, executable paths containing spaces, exit status, invalid arguments and missing executables ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37412103527)). Built-in dispatch integration, native tool cancellation, restricted-token/AppContainer launch, scoped ACLs and proxy-only networking remain open. Sandbox availability remains false. Local macOS sandbox/helper validation passes 12 tests; skipped Windows tests do not count as native evidence.

Built-in tool authorization is not connected to classifier allows yet. The next work is protected/irreversible ceilings and critical-path guards, then host integration, configuration, override/statistics commands and mode-disabling policy enforcement. Windows enforcement and its native runtime evidence remain unfinished. This runtime foundation passes 11 dedicated tests; full workspace validation passes 341 tests (one opt-in measurement ignored), release Clippy and strict OpenSpec validation.

The native Windows job at `d4d8416` passes workspace compilation and Clippy, parent-owned process-tree lifecycle, host-level live-descendant timeout/cancellation and normal-exit cleanup, output/environment/closed-stdin preservation and missing-helper refusal ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37417507450/job/112119282936)). The owner-cleanup test now relies on retained process handles rather than requiring a nonzero termination status. Foreground shell execution, including explicit full access, uses the parent-owned job boundary; configured Unix-shell `.exe` names and helper discovery are supported. Native PowerShell tool dispatch and AppContainer/ACL/network confinement remain open; sandbox availability is still false.

PowerShell-native permission analysis now collects simple-command resources and literal filesystem operand facts during the same bounded AST walk as removal risks. Quoted output remains data; value/common-parameter arguments are excluded from path facts, named destinations bind their actual slots, and arrays, redirections and derived item names participate. Binding/cwd uncertainty cannot restore literal proof. Local validation passes all 132 tool tests, workspace release Clippy and all 56 strict OpenSpec checks, including six new cross-platform parser tests; the native drive-path facts test also passes at `d205ded` ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37419236344/job/112124636685)). Parameter abbreviations, advanced mutation semantics, module trust and complete protected/irreversible facts remain required before classifier allows or the full native tool contract can be accepted.

Native `powershell` tool dispatch is implemented for Windows with installed-interpreter discovery, modern `pwsh.exe` preference and legacy fallback. Discovery excludes checkout executables and relative PATH entries. Calls use original-source UTF-16LE encoding, disable profiles, retain the `bash` permission action and share foreground capture, output limits, timeout/cancellation and parent-owned process cleanup. Local validation passes 134 tools tests, workspace release Clippy and all 56 strict OpenSpec checks. Native registration, rule filtering, golden/Unicode output, closed stdin, legacy source encoding and individually approved timeout/cancellation tests pass Windows CI at `99edbe9` ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37421652087/job/112132114794)). Cancellation correctly settles side-effecting calls as outcome unknown; tests retain the interpreter process handle to prove termination. Every registered Windows tool has a golden fixture. Default confinement remains unavailable and refuses launch. Legacy output encoding, advanced permission facts and AppContainer isolation remain open; this increment does not accept the full tool requirement.

Registration-bound service shutdown now uses an authenticated `POST /api/v1/service/stop` request carrying the server ID, rather than an external signal command using an unchecked PID. The route uses transport authentication and Origin checks, rejects mismatched IDs and exists only for applications that own listeners. Socket-only Unix service stops use the peer-authenticated socket; failed requests preserve registration. The stale-PID regression failed against unchanged main at `de7b23a`: the old implementation reported stop while the server continued running. HTTP validation passes 15 tests; generated SDK checks, type checking and 37 SDK tests pass. All 16 local application tests pass, including TCP/socket-only stop, identity/authentication refusal and credential replacement. All 17 CLI tests also pass, including an actual foreground server stopped by `cyber service stop`. Seven native Windows application lifecycle tests pass at `4a9a0f0` ([test-step evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37422202219/job/112133821941)); the job fails in the separate CLI process test before registration. After retaining the Windows system-root environment and capturing startup diagnostics, the actual native CLI service-stop test passes at `4868aa5` ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37424017821/job/112139477827)). Password replacement authenticates shutdown with the existing password before writing the replacement; refusal preserves credentials and registration. Active inference/tool cancellation, stream closure and verified stale/version replacement remain required before accepting the complete service lifecycle.

The runtime shutdown boundary closes admission, cancels all session drains and waits for durable settlement, while queued inbox rows survive for restart. It tracks and joins background title work, cancels blocked provider opening/streaming, preserves outcome-unknown settlement for side-effecting tools and clears settled approval waiters. Three new runtime scenarios pass within all 27 runtime tests. Listener ownership invokes this boundary after stopping and joining retention. All 50 selected runtime, HTTP and application lifecycle tests, workspace release Clippy and 56 strict OpenSpec checks pass. The actual CLI service-stop check also passes locally. All three native runtime shutdown scenarios and the actual CLI service-stop test also pass at `4868aa5` ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37424017821/job/112139477827)). HTTP streams, WebSockets, snapshot/compaction shutdown proof and the complete service deadline remain open.

HTTP event streams now scope producers to runtime shutdown and receiver disconnection, and close their response bodies during shutdown. WebSockets track and join pending requests, release channel-owned tool registrations and bound the close-frame write to one second. A real-server test keeps both event-stream types and a registered WebSocket tool connected while calling authenticated service stop; the original regression failed against unchanged main at `4868aa5` because attached clients held graceful shutdown open. All 51 selected application, HTTP and runtime tests pass, including channel-owned registration release. Workspace release Clippy, generated SDK checks and all 56 strict OpenSpec checks pass; specification lint reports zero errors. The actual attached-client shutdown regression also passes on native Windows at `e8dda31` ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37425523204/job/112144167466)). Active-tool/snapshot/compaction interruption and backpressured-client deadline evidence remain open.

Drain cancellation now scopes pre-turn snapshot collection and the entire optional post-turn snapshot/changed-path/diff sequence, while provider and tool settlement stays outside that cancellation scope. A four-stage regression failed against unchanged main at `e8dda31` because a blocked pre-turn snapshot held shutdown open; all four cases pass after the fix and verify dropped operations and absent completed post-turn snapshots. All 41 runtime and context/compaction tests, workspace release Clippy and all 56 strict OpenSpec checks pass. The four-stage snapshot regression also passes on native Windows at `4883ac6` ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37425944048/job/112145499872)). Native Git process cleanup, compaction interruption, active native tools and complete backpressured-service deadline proof remain open.

Compaction shutdown now has eight explicit cases: idle manual, queued manual during a running tool, automatic and context-overflow compaction, each blocked during provider opening and streaming. The existing cancellation paths pass without production changes. The tests verify dropped summary requests, idle caller shutdown errors, unchanged pre-existing conversation entries after a fresh runtime reload, absent completed summaries and no extra provider turns after interruption. All 42 runtime/context-compaction tests, all 56 strict OpenSpec checks and workspace release Clippy pass. All eight compaction cases also pass on native Windows at `9416d1c` ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37428019595/job/112152037071)). The backpressured service deadline also passes on native Windows at `39d44aa`; native Git process cleanup remains open.

A dedicated native active-tool service test now drives real built-in Bash and PowerShell dispatch from scripted inference, retaining handles to live child and grandchild workers before authenticated HTTP stop. It measures the complete stop deadline and reads outcome-unknown settlement through a fresh runtime handle cache, with failure cleanup limited to retained worker identities. PowerShell receives individual command approval. The new Windows CI step is added; native compilation and execution are pending. Local validation passes nine server lifecycle tests, workspace release Clippy and all 56 strict OpenSpec checks. The previous native run at `ad1a37b` stopped before compaction execution when the unchanged legacy PowerShell encoding fixture exceeded its ten-second guard ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37426583080/job/112147495711)). That fixture now captures stderr and uses a bounded thirty-second encoding-test guard; the active-service deadline stays at five seconds. Native workspace/test compilation passes at `53fdfce`, but Clippy rejects the service helper at complexity 18/15 ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37427420866/job/112150143449)). After extracting settlement assertions, the full native Windows job passes at `9416d1c`, including both active Bash/PowerShell cases, persisted outcome-unknown settlement, retained live worker termination and the complete five-second stop deadline ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37428019595/job/112152037071)). The legacy encoding fixture also passes. The new listener ownership implementation requires its own native follow-up. Local non-Windows validation cannot accept this fixture as native evidence or prove AppContainer confinement.

TCP and peer-checked Unix listeners now own their connection tasks. Shutdown stops admission to the listener, requests graceful drain, limits each connection to two seconds and joins those tasks after blocked connection futures and IO are dropped. A tiny-window client test reads the prefix of an eight-megabyte admitted event, then remains connected without reading while authenticated service stop runs. It fails against unchanged production code at `9416d1c` because registration survives beyond the deadline, then passes after the ownership change. All 67 selected application, HTTP, runtime and context/compaction tests and all 56 strict OpenSpec checks pass, including ordinary WebSocket and socket-only Unix behavior. Workspace release Clippy and generated SDK checks also pass. Truncated event responses can replay committed events from durable history. Native Windows backpressure, active Bash/PowerShell service shutdown and all compaction cases also pass with the new listener at `39d44aa` ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37428779060/job/112154457537)). Native Git process cleanup and abandoned-owner/HTTP2 lifecycle proof remain open.

The built-in host now stores a weak runtime callback, breaking the runtime → tool host → runtime reference cycle. An idle/stopped application regression fails against unchanged main at `e83a601` because the host remains alive after application disposal; both cases pass after the fix and verify release of host and database owners. All 78 selected application, HTTP, runtime, plan-tool flow and tool recovery tests, workspace release Clippy and all 56 strict OpenSpec checks pass. Plan tools still upgrade their callback while a runtime owner remains. The actual ownership-release test and full native Windows job pass at `2bfca16` ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37431885961/job/112164421160)). Abandoned active-owner lifecycle proof remains open.

Windows profile and direct-object ACL ownership now have five passing native tests at `1ec59c3` ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37433709007/job/112170302797)). Each invocation creates a fresh AppContainer identity. ACL leases retain the original file handle, refuse leaf reparse points and same-identity overlapping grants, and revoke only their own SID entries from the current ACL. Tests verify masks, preservation of existing and later-added grants, cleanup after path replacement, owner lifetime and junction refusal. Cross-target Windows Clippy and local sandbox tests pass. These primitives do not launch commands or enable sandbox availability. Restricted-token launch, recursive root/exclusion policy, credential isolation, proxy-only networking and crash recovery remain open.

A pending HTTP2 handler regression passes against unchanged production code at `1ec59c3`: listener shutdown releases handler-owned resources in 2.01 seconds while the client remains connected. All 16 selected HTTP/transport tests, focused release Clippy and all 56 strict OpenSpec checks pass. The actual test and full CI matrix also pass at `9436cc7` ([native CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37434183912/job/112171833466)); this listener-level test does not establish abandoned application-owner cancellation.

The low-level Windows launcher creates suspended, capability-free AppContainer processes with atomic job assignment and exact token/SID verification before resume. At `043bbb1`, the complete native profile/ACL and launch test steps pass: scoped file allow/deny, private Temp data and cleanup, explicit environment/arguments, input EOF and separate Unicode output, exclusion of an unrelated inheritable handle with a valid stdin control, loopback denial, and retained-handle primary/grandchild termination after owner disposal and normal exit. Invalid-input, missing-executable and nonempty-temp refusal also pass ([native CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37447546932/job/112215898358)). The full CI matrix passes at `043bbb1`. Owned asynchronous waits also pass their unpolled-future disposal, task-abort and exit-code tests on native Windows at `48b5b3f`, with all ten launch tests and the full CI matrix passing ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37448453282/job/112218859924)). Platform-aware credential-name matching and DACL/owner-right probes pass native Windows and the full CI matrix at `1067868` ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37449797377/job/112223323006)). Single-use profiles and post-start grant sealing pass all ten native launch tests and the full CI matrix at `177694c` ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37452498459/job/112232133426)). Actual tool integration, recursive root/exclusion policy, credential isolation, proxy-only networking and crash recovery remain required.

Direct-object Windows ACL preparation additionally pins and checks all local-disk ancestors before touching the leaf ACL, refusing parent junctions, parent traversal and unsupported path namespaces. New native tests cover junction refusal with an unchanged target ACL, directory replacement/write-handle refusal during preparation, guard release and retained-object revocation after a file moves to another directory. The native profile/ACL and launch steps pass at `b0cb751` ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37454361291/job/112238207164)); the remaining CI checks are pending. This is preparation for recursive filesystem policy, which remains open.

Recursive Windows root work adds relocatable direct-object leases: capture file/volume identity, verify reopening before mutation and reopen the original object for cleanup after directory movement or path replacement. Identity-based ACL writes suppress automatic child propagation; inherited profile entries are removed explicitly without replacing unrelated ACL entries. Multiply linked files are refused by this single-object primitive. All twelve profile/ACL tests, all ten launch tests and the full CI matrix pass at `86d2a3a` ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37460796028/job/112259559875)). A follow-up limits identity handles to metadata access and adds a directory-pinning coexistence regression; native verification of that follow-up remains pending. Recursive traversal, exclusions, dynamic-child accounting, crash recovery and actual tool integration are still required.

## Requirement inventory

Unchecked means not yet audited and accepted under this goal; some functionality may already exist. Each milestone will link requirements to implementation and scenario evidence before marking them complete.

225 P1-tagged requirements across 32 capabilities.

- [ ] `agents-subagents`: Agent tool spawns subagents
- [ ] `agents-subagents`: Structured subagent output
- [ ] `agents-subagents`: Background subagents and handback
- [ ] `agents-subagents`: Forked subagents
- [ ] `agents-subagents`: Resume subagents by name
- [ ] `agents-subagents`: Worktree isolation for subagents
- [ ] `agents-subagents`: Concurrency cap
- [ ] `agents-subagents`: Nesting depth
- [ ] `agents-subagents`: Subagent permission inheritance
- [ ] `agents-subagents`: Subagent result summarization
- [ ] `agents-subagents`: Cost attribution
- [ ] `agents-subagents`: Manual invocation by mention
- [ ] `agents-subagents`: Agent thread switching
- [ ] `agents-subagents`: Agent tool catalogue
- [ ] `background-tasks`: Background bash
- [ ] `background-tasks`: Job registry
- [ ] `background-tasks`: Completion notices
- [ ] `background-tasks`: Reading job output
- [ ] `background-tasks`: Monitor tool
- [ ] `background-tasks`: Tasks view
- [ ] `background-tasks`: Stop kills process trees
- [ ] `background-tasks`: Concurrency limits
- [ ] `background-tasks`: Jobs end with their session
- [ ] `background-tasks`: PTY sessions
- [ ] `background-tasks`: Model interaction with PTYs
- [ ] `background-tasks`: Notify tool
- [ ] `background-tasks`: Send file to user
- [ ] `background-tasks`: Background job events
- [ ] `background-tasks`: Background commands from the user
- [ ] `browser-verification`: Supported optional browser integration
- [ ] `browser-verification`: Verification artifacts and lifecycle
- [ ] `builtin-tools`: Capability-owned tool catalog
- [ ] `builtin-tools`: powershell tool
- [ ] `builtin-tools`: notebook_edit tool
- [ ] `builtin-tools`: monitor tool
- [ ] `client-sdk`: Hook callbacks
- [ ] `code-intelligence`: LSP enablement
- [ ] `code-intelligence`: Built-in servers
- [ ] `code-intelligence`: Custom and overridden servers
- [ ] `code-intelligence`: Lazy spawning and root detection
- [ ] `code-intelligence`: Diagnostics after edits
- [ ] `code-intelligence`: lsp tool
- [ ] `code-intelligence`: Read warms servers
- [ ] `code-intelligence`: LSP status and shutdown
- [ ] `code-intelligence`: Formatter enablement and detection
- [ ] `code-intelligence`: Custom formatters
- [ ] `code-intelligence`: Formatter execution
- [ ] `code-intelligence`: Formatter status
- [ ] `compaction`: Compaction hooks
- [ ] `compaction`: Tool output pruning
- [ ] `compaction`: Native provider compaction
- [ ] `compat-import`: Import command
- [ ] `compat-import`: Claude Code sources
- [ ] `compat-import`: Codex sources
- [ ] `compat-import`: OpenCode sources
- [ ] `compat-import`: Read-time skill compatibility
- [ ] `compat-import`: Read-time MCP compatibility
- [ ] `compat-import`: Mapping report
- [ ] `compat-import`: Secret safety during import
- [ ] `compat-import`: Source detection
- [ ] `compat-import`: Idempotent re-import
- [ ] `configuration`: Config inspection by agents
- [ ] `configuration`: References
- [ ] `cross-session-messaging`: Reachable session listing
- [ ] `cross-session-messaging`: Addressing
- [ ] `cross-session-messaging`: Send message tool
- [ ] `cross-session-messaging`: Message content limits
- [ ] `cross-session-messaging`: Delivery semantics
- [ ] `cross-session-messaging`: Inbound controls
- [ ] `cross-session-messaging`: Outcome reporting
- [ ] `cross-session-messaging`: Replies
- [ ] `cross-session-messaging`: Idle notifications
- [ ] `cross-session-messaging`: Rate limits
- [ ] `cross-session-messaging`: Permission boundaries stay per session
- [ ] `cross-session-messaging`: Untrusted content
- [ ] `cross-session-messaging`: Local transport
- [ ] `cross-session-messaging`: Non-interactive sessions
- [ ] `cross-session-messaging`: Transcript and audit
- [ ] `cross-session-messaging`: Usage attribution
- [ ] `cross-session-messaging`: Disable switch
- [ ] `cross-session-messaging`: Messaging API
- [ ] `editor-integration`: ACP command and transport
- [ ] `editor-integration`: ACP initialize and capabilities
- [ ] `editor-integration`: ACP session lifecycle
- [ ] `editor-integration`: ACP config options
- [ ] `editor-integration`: ACP prompting and streaming
- [ ] `editor-integration`: ACP tool calls and permissions
- [ ] `editor-integration`: ACP file write-through
- [ ] `editor-integration`: ACP client MCP servers
- [ ] `editor-integration`: VS Code extension
- [ ] `editor-integration`: Inline diff review
- [ ] `editor-integration`: Selection mentions and plan review
- [ ] `editor-integration`: IDE context source
- [ ] `editor-integration`: IDE detection and extension install
- [ ] `exec-mode`: Permission denial log
- [ ] `exec-mode`: Structured final output
- [ ] `exec-mode`: CI usage
- [ ] `exec-mode`: Streaming input and partial output
- [ ] `hooks`: Hook configuration
- [ ] `hooks`: Hook scopes and merge order
- [ ] `hooks`: Supported events
- [ ] `hooks`: Matchers
- [ ] `hooks`: Command handlers
- [ ] `hooks`: HTTP handlers
- [ ] `hooks`: Prompt handlers
- [ ] `hooks`: MCP tool handlers
- [ ] `hooks`: Decision schema
- [ ] `hooks`: Decision merging
- [ ] `hooks`: Interaction with permissions
- [ ] `hooks`: Stop hooks and loop prevention
- [ ] `hooks`: Timeouts and async hooks
- [ ] `hooks`: Parallel execution within a group
- [ ] `hooks`: Sandboxing of command hooks
- [ ] `hooks`: Trust for project hooks
- [ ] `hooks`: Hooks viewer and CLI
- [ ] `hooks`: Hook context injection on lifecycle events
- [ ] `hooks`: Hook observability
- [ ] `hooks`: Conditional, one-shot and annotated handlers
- [ ] `installation-upgrade`: Shell integration on install
- [ ] `mcp`: Tool search and deferred loading
- [ ] `mcp`: OAuth
- [ ] `mcp`: Elicitation
- [ ] `mcp`: Roots and sampling
- [ ] `mcp`: Organization MCP controls
- [ ] `mcp`: Server options
- [ ] `memory`: Memory locations
- [ ] `memory`: Memory file format
- [ ] `memory`: Index loading
- [ ] `memory`: Memory tool
- [ ] `memory`: Automatic memory generation
- [ ] `memory`: Memory toggles
- [ ] `memory`: Secret redaction
- [ ] `memory`: Deduplication and updates
- [ ] `memory`: Staleness notice
- [ ] `memory`: Memory command
- [ ] `memory`: Memory changes during a Session
- [ ] `memory`: Explicit remember requests
- [ ] `memory`: Memory HTTP API
- [ ] `observability-costs`: Usage commands
- [ ] `observability-costs`: Context window meter
- [ ] `observability-costs`: Status line data contract
- [ ] `observability-costs`: OpenTelemetry export
- [ ] `observability-costs`: Prompt content privacy in telemetry
- [ ] `observability-costs`: Debug traces
- [ ] `observability-costs`: Doctor health checks
- [ ] `observability-costs`: Performance diagnostics
- [ ] `observability-costs`: Usage data retention
- [ ] `observability-costs`: Diagnostics bundle
- [ ] `permissions-modes`: Critical-path removal guard
- [ ] `permissions-modes`: accept-edits mode
- [ ] `permissions-modes`: auto mode classifier
- [ ] `permissions-modes`: dont-ask mode
- [ ] `permissions-modes`: bypass mode
- [ ] `permissions-modes`: Auto-mode configuration and override
- [ ] `permissions-modes`: Rule dry run
- [ ] `plugins-marketplace`: Plugin manifest
- [ ] `plugins-marketplace`: Component discovery defaults
- [ ] `plugins-marketplace`: Install scopes
- [ ] `plugins-marketplace`: Plugin sources
- [ ] `plugins-marketplace`: Plugin CLI
- [ ] `plugins-marketplace`: Plugin host process
- [ ] `plugins-marketplace`: Host protocol
- [ ] `plugins-marketplace`: Restart and backoff
- [ ] `plugins-marketplace`: Scoped registrations
- [ ] `plugins-marketplace`: Capability declaration and enforcement
- [ ] `plugins-marketplace`: User configuration
- [ ] `plugins-marketplace`: TypeScript plugin kit
- [ ] `plugins-marketplace`: Pure mode
- [ ] `plugins-marketplace`: Trust and signing
- [ ] `provider-catalog`: Small model selection
- [ ] `provider-catalog`: Tool-call emulation for models without native tools
- [ ] `provider-catalog`: Additional native provider adapters
- [ ] `provider-catalog`: Model fallback chain
- [ ] `provider-credentials`: Multiple connections per provider
- [ ] `provider-credentials`: Provider OAuth flows
- [ ] `provider-credentials`: Command credentials
- [ ] `sandbox`: Windows enforcement
- [ ] `sandbox`: Escalation requests
- [ ] `sandbox`: Excluded commands
- [ ] `sandbox`: Container detection
- [ ] `sandbox`: Sandbox CLI
- [ ] `sandbox`: Sandbox events and audit
- [ ] `sandbox`: Named sandbox profiles
- [ ] `sandbox`: Environment policy
- [ ] `session-runtime`: Eager tool execution
- [ ] `session-runtime`: Structured output
- [ ] `session-runtime`: Side chat
- [ ] `skills-commands`: Remote skill sources
- [ ] `skills-commands`: Skill-scoped tool approvals and model
- [ ] `skills-commands`: Shell output injection
- [ ] `skills-commands`: Bundled skills
- [ ] `skills-commands`: Path-triggered and forked skills
- [ ] `snapshots-checkpoints`: Non-git fallback
- [ ] `snapshots-checkpoints`: Rewind targets
- [ ] `snapshots-checkpoints`: Undo and redo shortcuts
- [ ] `storage-events`: Retention and garbage collection
- [ ] `storage-events`: Transcript persistence switch
- [ ] `system-context`: Instruction imports
- [ ] `system-context`: Nested rule files on read
- [ ] `system-context`: Skills, references, and MCP instruction sources
- [ ] `system-context`: Context inspection
- [ ] `system-context`: Path-scoped rules and overrides
- [ ] `tool-registry`: Deferred tools and tool search
- [ ] `tool-registry`: Eager execution during streaming
- [ ] `tool-registry`: Annotations drive modes and hooks
- [ ] `tui`: Permission mode indicator and cycling
- [ ] `tui`: Rewind UI
- [ ] `tui`: Background tasks view
- [ ] `tui`: Subagent threads and side chat
- [ ] `tui`: Status line
- [ ] `tui`: Accessibility
- [ ] `tui`: Reasoning display and effort command
- [ ] `vcs-integration`: Local review command
- [ ] `vcs-integration`: Commit and PR text generation
- [ ] `vcs-integration`: PR checkout and linked sessions
- [ ] `web-client`: Local web client
- [ ] `worktrees`: Session worktrees
- [ ] `worktrees`: Worktree location and branch naming
- [ ] `worktrees`: Untracked file inclusion
- [ ] `worktrees`: Setup commands
- [ ] `worktrees`: Enter and exit tools
- [ ] `worktrees`: Cleanup on exit
- [ ] `worktrees`: Worktree management commands
- [ ] `worktrees`: Concurrency safety
- [ ] `worktrees`: Worktree events

## Provider credential environment integration

Built-in command environment preparation now combines provider credential names from the loaded catalog with names from the current location configuration. Disabled/unavailable providers remain covered, and location overrides cannot remove loaded catalog names from masking. Only names are supplied to filtering; credentials are not resolved or logged for command execution. Explicit environment exceptions and full-access execution retain their existing behavior. All 136 local tools tests, including three focused tool-boundary tests, tools/server release Clippy and all 56 strict OpenSpec checks pass. All three tool-boundary tests pass on native Windows and the full CI matrix also passes at `b467b03` ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37464526756/job/112272106902)); recursive credential-file isolation is still open.

## Recursive Windows filesystem preparation

`TreeInventory` now performs iterative, object-limited preflight before ACL mutation. It retains checked directory pins, captures verified object identities, refuses reparse points and multiply linked files, and revalidates recorded objects without accepting replacement paths. Native regressions compare stored file/directory ACLs and cover limit/error pin release, nested directory movement refusal, junctions, hard-link aliases and deleted-original replacement. Windows cross-target lint and thirteen local sandbox tests pass; native execution remains pending. This is preflight rather than complete recursive policy: it does not freeze child creation, install exclusion grants, track future inherited entries or enable Windows tool confinement.

## Existing Windows tree grant ownership

`Profile::grant_existing_tree` now grants inventoried existing objects under one preparation/start lock. Directory pins survive through setup; grants use verified identity records and suppress implicit propagation. A later failure rolls back earlier owned grants and reports rollback failure; all installed identities are revalidated before successful return. The owner attempts every revocation, retains failed leases for retry and releases successful profile ownership independently. It keeps no child handles after preparation. Three native ownership cases and a new direct AppContainer launch case cover rollback, directory moves/path replacement, unrelated-profile preservation, retry after denied DACL access, nested file writes, outside-write denial and security-right denial. Local checks pass; native execution is pending. This does not implement future-child inheritance, exclusions, crash recovery or built-in confinement.

The inventory's first native run at `83927ad` passes sixteen of seventeen cases. The remaining fixture attempts to create a hard link while directory pins remain alive and receives sharing violation 32. The updated regression requires that refusal, releases its preparation pins, then requires identical alias creation to succeed and retained-identity verification to reject the new hard link. Native follow-up remains pending.

Native inventory and existing-object ownership are accepted at `9ee3d5b`: all twenty profile/ACL tests, all eleven launch tests, the complete Windows job and the full CI matrix pass ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37467021521/job/112280511243)). This includes the corrected hard-link positive control, real revocation-denial/retry, rollback, directory movement and the new nested-file launch boundary.

Existing-object exclusion policy now resolves rule identities before mutation and applies unreadable/read-only precedence through recorded parents. Deny/allow rows install together, retaining unrelated ACLs and ordinary writable deletion while denying parent delete-child bypass. New native regressions cover case aliases, missing/out-of-tree refusal, descendant permissions, denied security rights and cleanup preserving another profile. Local Windows cross-target lint and sandbox tests pass; native policy acceptance is pending. Broad application-package positive controls, absent/future names, dynamic-child cleanup, forest overlap and actual tool integration remain required for complete Windows filesystem policy.

A broader native permission fixture now uses three fresh AppContainer identities against the same temporary objects: package-only allowances prove access first, owned exclusion policy denies it second, and permissive access after policy cleanup proves unrelated allowances survive. Parent-based deletion controls have no package ACE on the target file; ordinary writable deletion remains a separate allowed control. Fixture ACLs are restored through retained original handles. Windows cross-target lint and thirteen local sandbox tests pass; native execution of this fixture remains pending.

Basic existing-object exclusion policy is accepted at `3cfee75`: all twenty-two profile/ACL tests, twelve launch tests and the full CI matrix pass ([CI evidence](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37468843567/job/112286705180)). The broader package-allowance fixture fails at `d3803b4`: its permissive baseline succeeds, but its scoped-policy probe permits an excluded operation. The post-cleanup permissive probe is not reached. This contradicts full scope-enforcement acceptance; basic-policy evidence alone is insufficient.

Native broad-permission evidence at `d3803b4` is a failure ([Windows job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37470403715/job/112292090284)): twenty-two profile/ACL tests pass, but launch tests report twelve passes and one failure. The existing diagnostic does not identify which excluded operation succeeded. The next revision labels each denied operation without weakening assertions or permissive controls. Windows availability remains disabled.

Overlapping-root preparation is implemented but awaits native acceptance. All roots and exclusions are checked before ACL mutation. Duplicate objects merge base write access while explicit read-only/unreadable restrictions remain ceilings, independent of root order. The distinct-object bound applies across the forest; installation, rollback and final identity verification share one owner. Keys include the held volume root's GUID name, serial and full file ID, refusing unsupported volume identity resolution before mutation. Three native-only regressions cover order, limits/preflight and rollback; the existing launch fixture uses a readable outer root and writable inner root. Windows cross-target Clippy and thirteen local sandbox tests pass; local tests do not execute the Windows fixtures. Future children, credential files, crash recovery and tool integration remain open.

M1.2 now has an active change (`implement-m1-2-subagents-worktrees`) covering the full worktree/subagent milestone. Shared settings provide the documented HEAD, cyber/ and auto defaults; generated names have adjective-noun-four-hex shape, and parsing enforces the required ASCII name syntax before lifecycle operations. Configuration loading rejects malformed worktree settings after workspace-trust filtering. Tests prove untrusted setup remains inactive and invalid setup fails after approval. All 52 core tests, core Clippy and 57 strict OpenSpec checks pass locally. This does not prove Git creation, locking, cleanup, setup execution, child sessions or any native worktree behavior; all remain open.

Native Windows evidence at `ca06249` identifies the broad-permission bypass as `protected file write` ([Windows job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37472639636/job/112299837147)). All twenty-five profile/ACL tests pass, including three overlapping-root cases and live volume-GUID resolution. Twelve launch tests pass, including the readable-outer/writable-inner forest fixture; the broad package-allowance test fails. Its permissive baseline succeeds and post-cleanup probe is not reached. Full enforcement remains unaccepted; the next change must address this observed write bypass without weakening the fixture.

Default Windows launch now requests capability-free LPAC creation via All Application Packages opt-out. Attribute errors fail launch without ordinary-AppContainer fallback. The broad-permission fixture explicitly retains ordinary-AppContainer positive controls through the `windows-test-controls` feature, enabled by the native CI launch step; its protected probe uses default launch. Both default and all-feature Windows cross-target Clippy pass, as do thirteen local sandbox tests and 57 strict specification checks. Native launch/compatibility and the observed protected-write regression await the next CI run; no fix acceptance or full enforcement claim is made. All Restricted Application Packages and other ambient authority remain additional required enforcement cases.

At `5dc5c50`, the native Windows broad package-allowance regression passes, including the ordinary-AppContainer baselines before/after cleanup and default LPAC protected access. All twenty-five profile/ACL tests pass, but launch tests finish with eleven passes and two failures ([Windows job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37473774031/job/112303752001)). The loopback fixture fails during Winsock startup (10107); descendant creation fails with access denied (5). These are actual LPAC compatibility failures, not accepted denials or evidence of successful process-tree cleanup. Both existing assertions remain required; default launch and full Windows enforcement are not accepted. Capability-free registry/runtime support must be investigated without restoring wildcard authority.

M1.2 adds an owned, nonblocking cross-process repository lock at `cyber-worktree.lock` using Rust's standard file-lock API. The persistent file is neither truncated nor deleted, and a non-cloneable guard retains its handle. Independent-process regressions prove contention, release on normal disposal, release after forced termination and independent repositories; I/O failure remains distinct from contention. All 56 core tests, local/core Windows cross-target Clippy and 57 strict specification checks pass. Native Windows execution is added to CI and remains pending; Git lifecycle integration, shared-directory resolution, cancellation-aware acquisition and active-session refusal are still required.

The next native diagnostic revision checks Winsock initialization explicitly before Rust networking can panic, reporting its stage and return code without treating initialization failure as a successful network denial. Descendant creation separately checks executable resolution/readability before labelling process spawn failure. The original loopback success/denial and live-descendant cleanup assertions remain unchanged. This revision diagnoses compatibility; it does not fix it.

Native CI at `4fff705` fails before the lock and launcher fixtures run: twenty-two config tests pass, but the project-source attribution test compares different spellings of the same Windows path as strings ([Windows job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37474882763/job/112307586682)). The committed-main failure shows mixed slash/backslash spelling in the expected fixture path versus discovered source path. The regression now verifies the `project:` label and compares paths using platform path semantics. No production attribution behavior changes; lock and LPAC diagnostics still await native execution.

The managed repository API now discovers the common Git directory, acquires its lock, validates a complete branch and commit base, persists pending ownership before creation and marks ready ownership only after repository/branch verification. A required execution port delegates Git to the runtime's sandbox/process owner; there is no default host subprocess launcher. Reuse preserves user edits, unowned targets are refused and failure/cancellation preserve records, branches and files for recovery. Six real-Git regressions cover creation/reuse, custom roots/base/branch, refusal, failure, cancellation and changed-branch preservation. Reuse also checks Git's NUL-delimited worktree registration. Session/CLI/API wiring, sandbox execution-port integration, includes/setup, active-session refusal, cleanup and recovery settlement remain open.

A parallel cancellation regression caught contention immediately after creation-future disposal. Normal repository-lock disposal now explicitly unlocks before closing; the assertion remains immediate. Six repository cases pass again after this change. Lock-content preservation is checked before acquisition and after release, avoiding platform-dependent I/O against a held mandatory lock. Native verification remains required.

Current managed-creation checks pass all 62 core tests, core Clippy on macOS and the Windows GNU cross target, and 57 strict specification checks. Native CI now includes all six real-repository cases with the config and lock regressions. This is library groundwork: no CLI/session/runtime worktree feature is accepted until the execution port and full lifecycle contracts are integrated and natively verified.

Native evidence at `2f8f199` proves all twenty-three config tests and all four cross-process repository-lock cases, including forced termination ([Windows job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37477322566/job/112316030002)). The six repository tests finish with one pass and five failures: Git rejects the canonical Windows verbatim target prefix while creating leading directories. Launcher diagnostics are not reached.

The next revision normalizes only explicit Git target arguments to equivalent drive/UNC spelling, preserving canonical filesystem/ownership identities. Command-local `core.longpaths=true` avoids global configuration changes. Ambiguous DOS names, trailing-dot/space components and unsupported namespaces fail before pending ownership or branch creation. Two additional native-only regressions cover long targets and ambiguous-name refusal. All 62 local core tests, local and Windows cross-target Clippy and 57 strict specification checks pass; Windows execution of the argument fix remains pending. UNC behavior is not claimed from drive-path tests.

At `1aeb710`, native Windows passes all twenty-three config cases, four cross-process lock cases and seven of eight repository cases ([Windows job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37479004190/job/112321898888)). Ordinary canonical-path creation/reuse and ambiguous-name refusal pass. The long-path case fails with `$GIT_DIR too big`; the complete CI run fails and launcher diagnostics remain unreached.

The next revision separates worktree registration (`--no-checkout`), index loading and non-forcing initial checkout in the new worktree directory. Pending ownership covers all stages. A new real-Git regression proves that a file appearing before checkout is preserved, creation remains pending and the repository lock is released. All 63 local core tests, local and Windows cross-target Clippy and 57 strict specification checks pass. The Windows long-path assertion remains unchanged and native acceptance is pending; full lifecycle/runtime integration and LPAC compatibility remain open.

At `3405edc`, native Windows passes all twenty-three config cases, all four lock cases and eight of nine repository cases ([Windows job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37482159950/job/112332814780)). Long-path creation fails with process-launch error 267 (`The directory name is invalid`); the checkout-collision preservation case passes. Launcher diagnostics remain unreached. Microsoft documents that a current directory beyond MAX_PATH causes CreateProcessW failure ([SetCurrentDirectory documentation](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setcurrentdirectory)).

Windows Git execution now starts from a normalized ancestor shorter than 240 UTF-16 units, passing the actual requested directory through Git's `-C` option. Creation, verification and reuse share that preparation. Canonical ownership remains unchanged, and future runtime sandbox integration must scope access to the requested worktree rather than the launch ancestor. Native regressions retain long-path creation/reuse and assert short process working directories. All 63 local core tests, macOS and Windows cross-target Clippy and 57 strict specification checks pass; native acceptance is pending.

### Immediate P1 resumption task

Fix the remaining Windows long-path creation/reuse failure within M1.2, retaining the full M1.1–M1.5 goal and all P1 requirements. At `2e2c73c`, all twenty-three config cases, all four lock cases and eight of nine repository cases pass, but the long-path case fails inside Git with `cannot change to ...: Filename too long` ([Windows job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37483126524/job/112336189071)). Git now starts successfully. Inspection of the runner's Git version (`2.55.0.windows.5`) shows `-C` calls `chdir` during option parsing, while long-path conversion defaults to disabled until repository configuration is initialized; this explains why the command-local setting does not enable that early directory change. Avoid that early long-path change rather than weakening the regression or shortening its target. Require native creation/reuse acceptance and execute the downstream sandbox fixtures, whose LPAC compatibility failures remain unresolved. This task is part of the existing P1 goal, not a replacement objective or a phase-completion claim.

Long linked worktrees now use their verified short metadata directory for Git index/branch/common-directory operations, with an absolute non-forcing checkout destination. Canonical `.git` marker and metadata backpointer must agree in both directions; reuse retains the common-repository, branch and registration checks. Two new real-Git regressions prove metadata-only checkout writes to the intended worktree and preserves a collision, and changed backpointers refuse reuse while preserving user edits. All 65 local core tests, macOS/Windows cross-target Clippy and 57 strict specification checks pass. Native long-path acceptance remains pending; unsupported long metadata directories fail explicitly rather than accepting a different repository. Independent Windows native test steps now run after a prior test failure unless cancelled, retaining every failure in the job result so worktree errors no longer hide LPAC evidence.

At `4a5bf88`, all twenty-three config cases, four lock cases, eleven repository cases and four settings/name cases pass on native Windows, including the unchanged long-path creation/reuse regression ([Windows job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37487825539/job/112352409473)). All twenty-five profile/ACL cases pass. The launch suite reports eleven passes and two failures: Winsock initialization returns 10107, and descendant executable resolution/read succeed but process spawn returns access denied (5). Every subsequent independent native step passes, including shell/PowerShell and service/runtime shutdown. The Windows job is complete with failure. This accepts the tested Git boundary, not full managed lifecycle/runtime integration or Windows confinement.

The next LPAC diagnostic revision reports read-open status for system Winsock parameters/catalog and Image File Execution Options keys, plus query/duplicate access to the worker's own token. It reads no registry values, changes no ACLs and adds no capabilities. Host positive controls require successful Winsock initialization, the Winsock-key reads and token duplication. Original Winsock/network and live-descendant assertions remain required. Thirteen local sandbox tests, macOS Clippy, Windows cross-target Clippy with default/all features and 57 strict specification checks pass; native diagnostic execution remains pending. A denied probe narrows investigation but does not prove causation.

Native diagnostics at `07f6cae` pass their host controls. Both failing workers can query/duplicate their own tokens (0), but read-open attempts for all three tested system registry keys return access denied (5). Winsock initialization still returns 10107 and descendant spawn still returns 5 ([Windows job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37489456682/job/112358032950)). These results establish a registry access gap without proving that any one probed key causes either failure. No capability or permission change is accepted from these probes alone.

M1.2 adds `.worktreeinclude` copying to managed creation. Gitignore patterns select untracked files, including gitignored environment files, from the primary checkout even when creation starts in a linked checkout. Confined directory handles bound file access, destination index entries are skipped, exclusive creation preserves other existing files, and symlinks/special files/unsafe paths fail. Permissions are applied before copying content; copied-byte SHA-256 records support later cleanup accounting. Inclusion failures leave pending ownership, and reuse does not repeat copying. Six new local real-Git regressions cover patterns/negation, ignored files, tracked files, reuse edits, collisions, unsafe enumeration, symlinks, permissions and linked-source selection. All 71 local core tests and 57 strict specification checks pass. Native tests additionally require inclusion into the existing long-path target and alternate-stream refusal; native acceptance, sandboxed setup and lifecycle/runtime integration remain pending.

Native CI at `ec8ba9f` passes all sixteen repository cases, twenty-three config cases, four lock cases and four settings/name cases. Long-target inclusion and alternate-stream refusal pass. All twenty-five profile/ACL cases pass; launch reports twelve passes and the same two failures (Winsock 10107 and descendant spawn 5), with token access succeeding and all three registry read-open probes denied. Every subsequent independent native step passes. The Windows job completes with failure ([Windows job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37507000869/job/112418076658)). Inclusion has native library evidence; full runtime/lifecycle and Windows sandbox acceptance remain open.

The core setup dispatcher now verifies persisted ready ownership and repository registration under the shared lock, runs ordered commands through a required sandbox execution port and streams byte events without collecting command output. Nonzero or signal exits stop subsequent commands; failure/cancellation preserve worktree contents. Three injected-port regressions verify ordering, failure/signal preservation, live output before cancellation, immediate lock release and ownership refusal. All 74 local core tests, macOS and Windows cross-target Clippy, and all 57 strict specification checks pass. These tests do not execute sandboxed setup processes: runtime process-tree ownership, Session delivery and durable interruption/retry settlement remain open, and native execution of the new dispatcher tests is pending.

The built-in host now adapts worktree setup to real sandboxed processes. Admission requires matching Session Location and the `worktree` permission; configuration comes through the existing trust-filtered callback. Ownership verification uses a sandboxed Git port with ambient overrides removed and bounded output. Setup streams both byte channels, filters configured credentials, handles proxy permission questions and retains process ownership through failure, cancellation and future disposal. A strict setup scope excludes the shell wrapper's ambient macOS user-temp grant: the unchanged sibling-write denial regression now passes without granting that unrelated directory. Five macOS real-process tests pass, covering Location/permission refusal, actual write denial, credential masking, failure preservation, output-delivery failure and live descendant cleanup on cancellation/disposal/normal exit. All 141 local tool tests (including golden and recovery suites) and macOS all-target Clippy pass; all 57 strict specification checks pass. Local Windows cross-Clippy cannot build C dependencies because `x86_64-w64-mingw32-gcc` is unavailable, so native Windows compilation/admission remains a CI gate. Windows still refuses enforced execution while LPAC compatibility is unresolved. Automatic post-creation setup, durable attempt settlement and Session event delivery remain open; this API alone does not close setup or M1.2.

CI at `f10ec2d` passes the complete macOS and Linux Rust jobs, SDK/spec checks and all six macOS/Linux platform builds. The Windows job passes native lint, worktree core tests and setup admission; the job completes with failure solely in the LPAC launch step (twelve pass, two fail: Winsock 10107 and descendant spawn 5). Token query/duplication succeeds and the three registry read-open probes remain denied; all later independent native steps pass ([Windows job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37511229702/job/112432587844)). This proves native compilation and setup admission, not Windows enforced setup execution or full sandbox acceptance.

Setup now journals command intent before dispatch and results after owned process-tree settlement through the existing database writer. Dedicated typed events use a stable worktree aggregate and a recipe digest; source commands are not stored verbatim. Paginated replay reuses recorded outcomes, rejects pending unknown outcomes and conflicting recipes (including clearing setup), and optimistic concurrency permits one dispatch authorization. New ownership records persist a creation ID before Git mutation, so a later worktree at the same path has an independent journal; legacy ownership without that ID requires recovery for journaled setup. Six journal tests cover database reopening, concurrency, successful/failed/signalled replay, settlement conflicts, pagination and distinct incarnations. Runtime process tests also verify repeat requests do not repeat side effects and disposal cannot silently rerun an unknown command. All 74 core tests, 29 runtime tests, six journal tests and 57 strict specification checks pass. All 141 local tool tests and all-target macOS Clippy across core/server/tools pass. Native Windows compilation and the new journal cases remain pending in CI; the local Windows C-compiler limitation is unchanged. Automatic creation dispatch, recovery resolution, Session output integration, worktree cleanup and crash/orphan process recovery remain open.

CI at `1cff224` passes full macOS/Linux Rust checks, SDK/spec validation and all six platform builds. Native Windows passes all six durable setup journal cases, setup admission and the other independent native steps; the job completes with failure in the launch suite (twelve pass, two fail) ([Windows job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37513017046/job/112438763331)). This accepts the tested journal boundary, not Windows enforced setup or full sandbox integration.

The attached-runtime setup wrapper now publishes `session.worktree.setup` updates on the existing Location instance event stream. Each update identifies Session, worktree creation ID, call and command index; stdout/stderr bytes are carried as bounded base64 chunks, and command source is excluded from metadata. Attempts may reuse journaled results and do not claim a process started. Finished updates carry exit/signal status; execution failures produce failure updates. A runtime lifecycle guard remains held while shutdown cancels the operation's private token and waits for acknowledgement or bounded disposal. Graceful cancellation settles durably; unacknowledged disposal retains pending intent. Real-process coverage observes both channels before command completion and verifies cancellation/shutdown stopping descendants. HTTP coverage verifies actual authenticated SSE delivery and exclusion of other Locations. An explicit symlink-alias regression fails on committed main `1cff224`; normalizing the cached Session directory fixes Location filtering without changing the assertion. All 142 local tool tests, 29 runtime cases, 17 HTTP cases, three output/lifecycle cases and 57 strict specification checks pass. All-target macOS Clippy for server/tools, formatting, generated SDK consistency and diff checks pass. Native checks remain pending in CI. Automatic creation/enter setup dispatch, snapshot-safe Location transitions, TUI rendering, durable output artifacts and recovery resolution remain open.

The setup stream uses `GET /api/v1/event` with normal Location scoping (or `scope=all`), like other live-only fragments. Its envelope's `data` contains `session_id`, `worktree_id`, `call_id` and `update`. Update phases are `attempted`, `output`, `finished` and `failed`; output includes `index`, `stream` (`stdout` or `stderr`) and `base64`. Durable Session replay is unchanged; the setup command journal records settlement separately.

CI at `5eb7ad1` passes the complete macOS/Linux Rust jobs, SDK/spec validation and all six platform builds. Native Windows passes the setup journal/output lifecycle step and setup admission; its LPAC launch step fails again while later independent steps are still running ([Windows job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37533366964/job/112508052896)). These results accept the tested streaming/lifecycle boundary, not enforced Windows setup execution.

The host now offers `create_worktree_session` to connect owned sandboxed Git creation/reuse, fresh Session admission at the ready target and automatic journaled setup. Creation uses the persisted source Session's mode/rules, rejects a mismatched source Location or existing Session ID, and refuses denied permission, read-only sandbox policy or prior cancellation before recording ownership. Discovery has no repository write grant; provisioning grants only verified common Git metadata and the target, plus managed temp/output and explicit configured roots. The core creates the reserved target exclusively after persisting intent so Linux can bind that exact directory without its parent. Provisioning can create tracked configuration/metadata, while setup keeps its normal protected-path policy. Git failures retain pending ownership/target and refuse blind retry; setup failures return the retained Session/worktree and outcome. A separate real sandbox test confirms metadata/target writes succeed while source checkout and sibling writes are denied. Real creation tests verify primary inclusion, tracked configuration checkout, automatic setup output, failed setup retention and reuse preserving edits without repeating effects. All 74 core tests, 146 tool tests, all-target macOS Clippy, 57 strict specification checks, formatting, generated SDK consistency and diff checks pass. Creation admission coverage is enabled on Windows; new native acceptance remains pending. Public CLI/API routes, snapshot-safe enter/exit, cleanup/recovery, TUI/artifacts and subagent orchestration remain open. P0 baseline state is unchanged.

CI at `5e2218c` passes the complete macOS/Linux Rust jobs, SDK/spec validation and all six platform builds. Native Windows passes creation/reuse/inclusion and the new creation admission regression; the job completes with failure only in LPAC launch (twelve pass, two fail: Winsock 10107 and descendant spawn 5). Token query/duplication succeeds and all three registry read-open probes remain denied; later independent steps pass ([Windows job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37534363814/job/112511447574)). This accepts the tested sandboxed creation boundary on macOS/Linux and Windows admission, not enforced Windows creation/setup.

Fresh managed-worktree startup is now exposed through TUI/exec `--worktree [name]`, `POST /api/v1/worktrees` and the generated TypeScript SDK. An explicit authenticated startup request authorizes only its creation/setup operation; denied rules, plan mode and read-only sandbox policy remain enforced, and the selected Session mode/rules remain unchanged. It creates no additional source Session. The API returns the actual Location, normal Session view, public ownership identity and completed/failed/error setup status; existing idempotency wraps the request. Startup clients subscribe before creation and correlate live setup bytes by call ID, printing to stderr while preserving structured stdout. Resume/fork and ephemeral combinations are rejected before worktree creation.

Startup captures the resolved source recipe and configured credential environment names, including disabled providers. It runs approved source setup without approving the new checkout; unapproved source setup stays inactive. Target sandbox/permission configuration remains trust-filtered, while captured source credential names add restrictions to the existing environment filter. Explicit env allow rules and user-selected full access retain their behavior. Real app tests cover global/approved/unapproved setup, live delivery before completion, no extra source Session, retained failure, no target trust approval and idempotent replay. A real embedded CLI test verifies owned checkout/setup files, clean JSON stdout and stderr progress without a provider call. All 148 local tool tests, 22 app tests (including backup/retention), 31 CLI tests, 18 HTTP tests, all-target macOS Clippy, 37 SDK tests/typecheck, 57 strict specification checks, formatting and generated-source consistency pass. Native startup flag/API admission steps are added to CI; new native acceptance is pending.

The first-frame regression measurement uses the same MacBook Pro Mac14,6 / M2 Max / 32 GB / macOS 27.0 reference machine, with 20 fresh-app samples in each launch mode. Full samples and compiled-binary/source checksums are retained in [the startup artifact](../measurements/p1-worktree-startup-m2-max.json); OS file caches remain retained. All 40 samples are below 150 ms: service-cold p95/max 90.65/99.99 ms; embedded p95/max 89.23/116.45 ms. Original P0 measurement and local-model artifacts are unchanged. Automatic exit cleanup, management GET/DELETE and CLI commands, snapshot-safe enter/exit, recovery resolution, in-TUI setup rendering, durable output artifacts and subagent orchestration remain open.

Core ownership listing now holds the repository lifecycle lock, returns ready/pending/invalid entries in name order and verifies ready entries against Git repository/branch/registration. Malformed, oversized, mismatched or symlinked records receive individual diagnostics; a symlinked ownership directory is refused. Cancellation propagates and releases the lock rather than marking the record invalid. Six focused cases cover these boundaries and preservation of user edits/pending evidence. All 80 local core tests, all-target core Clippy and all 57 strict specification checks pass. Native acceptance is pending; public list routes/commands, dirty/ahead/behind and owning Session summaries, removal/prune and automatic cleanup remain open.

CI at `39b9e04` passes the complete macOS/Linux Rust jobs, SDK/spec checks and all six platform builds. Native Windows passes startup flag/API admission, worktree creation/admission, setup journal/output lifecycle and all other independent steps. The job completes with failure only in LPAC launch (twelve pass, two fail: Winsock 10107 and descendant spawn 5); token duplication succeeds and all three registry read-open probes remain denied ([Windows job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37538965583/job/112526982883)). This accepts native startup compilation/admission, not enforced Windows execution.

Management listing is exposed through `cyber worktree list`, `GET /api/v1/worktrees` and SDK `v1.worktree.list`. The host uses lifecycle-owned sandboxed Git with no repository writable roots. Ready status reacquires the repository lock, verifies the persisted creation identity and registration, reports porcelain dirty state without optional index locks, and counts commits ahead/behind the original creation base. HTTP enrichment includes canonical matching Locations across every Session page, with children and archived rows included. Pending and invalid entries retain distinct diagnostics, and the CLI supports JSON and text. These informational snapshots do not authorize removal.

Real Git tests verify clean/untracked/diverged status and creation-identity refusal. App tests verify listing from primary/linked checkouts, failed-setup dirty state and all 202 associated Sessions across pagination, including an archived child. A real CLI integration lists ready/invalid records in both output formats through a local service, stops it through authenticated shutdown and verifies setup files remain unchanged. All 81 core tests, 148 tool tests, 22 app tests, 33 CLI tests and 18 HTTP tests pass locally; all-target Clippy, 37 SDK tests/typecheck, all 57 strict specification checks, formatting and generated-source consistency pass. Native status/long-path listing acceptance remains pending; Windows CI now includes management flag/rendering assertions and extends the existing long-target regression to clean/dirty status. Removal/prune, active Session refusal, automatic cleanup, enter/exit, recovery resolution and subagents remain open. P0 evidence is unchanged.

CI at `f2789bb` passes all 24 native Windows repository cases, full macOS/Linux Rust checks, SDK/spec and all six platform builds. Windows still completes with failure only in LPAC launch (twelve pass, two fail: Winsock 10107 and descendant spawn 5); other independent checks pass ([Windows job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37539554743/job/112528910464)). This accepts core ownership-listing evidence, not enforced Windows runtime execution.

CI at `3c93bc7` passes macOS/Linux Rust checks, SDK/spec and all six platform builds. Windows passes 24 repository cases but fails the new long-target clean-status assertion, plus the two LPAC launch cases; later independent steps pass ([Windows job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/37540856710/job/112533238474)). The status regression fails on committed main `3c93bc7`; the original long-target creation/inclusion/reuse assertions remain intact. A portable real-Git probe reproduces `--git-dir` treating the metadata launch directory as its working tree, reporting tracked files deleted and metadata untracked. Working-tree reads now bind the verified checkout explicitly with `--work-tree`, while checkout keeps its explicit destination and short launch directory. [Git documents this required explicit working-tree binding](https://git-scm.com/docs/git/2.45.1.html). New native acceptance is pending; long-path status and enforced Windows sandbox gates remain open.

Transactional core removal now holds the repository lock and requires a runtime activity-admission port whose owned guard remains held across Git mutation and settlement; force cannot bypass that port. Exact ready ownership/creation identity is required. Non-forced removal preserves uncommitted/ahead work, edited included ignored files and new ignored files; unchanged ignored inclusions are compared to their persisted hashes. Commands run from common Git metadata, allowing removal initiated from the selected linked Location without deleting their launch directory.

A synced removal journal persists tree/branch deletion intents and settled phases, with conditional branch deletion against the admitted object ID. Retries only reconcile missing artifacts through read-only checks; unknown existing targets, changed/checked-out branches, replacement paths and conflicting identity/force requests refuse continuation. A recreated same-OID branch is preserved after lost acknowledgement. Old completed identities cannot touch a later creation. Pending removal blocks create/reuse/setup/status and stays visible in listing after ownership unlink. Twelve focused real-Git/injected-port cases cover these boundaries, guard release on future disposal, ignored files and symlinked journal refusal. All 93 core tests, 148 tool tests, 22 app tests and 33 CLI tests pass locally, with all-target macOS Clippy, core Windows cross-Clippy, all 57 strict specification checks, formatting and generated SDK consistency passing. Native removal/status acceptance remains pending.

The injected activity-port cases prove the core protocol, not actual runtime running-Session refusal. Cross-process activity leases/Drain-client admission, a removal-specific sandbox/filesystem adapter, public DELETE/remove/prune, lifecycle events and automatic exit cleanup remain open. The creation adapter's exact-directory scope cannot be assumed sufficient to delete that directory itself. Process-crash/orphan resolution and native power-loss durability are not accepted by these tests. P0 baseline and startup measurement artifacts are unchanged.
