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
