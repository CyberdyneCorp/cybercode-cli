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
