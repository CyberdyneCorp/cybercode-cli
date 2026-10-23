# P1 sign-off plan (2026-10-23)

This plan takes M1.1–M1.5 and same-machine cross-session messaging from "done locally" to sign-off. It has two parts: an evidence pass over a green `main` CI run, and a prioritized plan for every P1 item that is still open. Requirement classes are those of the [acceptance plan](p1-acceptance-plan.md#classes). Scope and the 57 requirements outside the historical 214-row snapshot are tracked in the [reconciliation](spec-doc-reconciliation.md).

## Evidence source

Main CI run [`39905096927`](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/39905096927) (push of `5283825`, merge of #18) passed every job. The nextest output of three jobs was read test by test:

| Job | ID | Ran | Skipped |
|---|---|--:|--:|
| Rust (ubuntu-latest) | `120255174711` | 4,148 | 11, all `#[ignore]` |
| Rust (macos-latest) | `120255174695` | 4,147 | 11, all `#[ignore]` |
| Windows build (`ci-windows` profile) | `120255174678` | 3,015 + 31 in its extra native steps | 438: 422 by the profile filter, 16 `#[ignore]` |

Rule: a test counts as evidence for a platform only if its `PASS` line is in that job's output. Tests the `ci-windows` filter leaves out, tests gated by `#[cfg(unix)]`, and tests `#[cfg_attr(windows, ignore)]` are not evidence for Windows. A test that passes trivially when the code under test cannot run is not evidence either.

## Evidence closed (2026-10-23)

These OpenSpec tasks were ticked. Each one's named tests passed natively in the run above.

| Milestone | Task | Tests that ran and passed |
|---|---|---|
| M1.1 | Overlapping existing-root preparation (dedup, base union, exclusion precedence, order independence, distinct-object limits, transactional cleanup) | Windows: `cyber-sandbox` lib `windows_container::tests::existing_forest_unions_base_access_and_preserves_exclusions_in_both_orders`, `existing_forest_distinct_limits_and_later_root_failure_do_not_mutate`, `existing_forest_late_installation_failure_rolls_back_earlier_objects` |
| M1.1 | Classifier allow/block, irreversible actions, malformed output, three-block fallback, unattended denial | All three platforms: `cyber-tools::auto_dispatch` (8 named cases) and `cyber-server::auto` (2), listed in the task |
| M1.2 | Long-root inspection backend | Windows: `cyber-core::worktree_repository` `library_status::library_status_reads_long_worktrees_without_changing_repository_files`, `library_status::owned_inspection_helper_reads_long_worktree_changes_and_ignored_files`; `cyber-tools::isolated_subagents` `long_root_child_reports_changes_lists_status_and_resumes_through_owned_execution` |
| M1.2 | Native long-path creation/reuse failure fixed | Windows: 59 `cyber-core::worktree_repository` cases, including `creates_real_worktree_and_reuses_without_overwriting_user_edits` and `windows_long_paths_are_preserved_at_the_git_argument_boundary` (8 Unix-only symlink/mode cases are not built there) |
| M1.2 | Cyclic descendant graphs refused before deletion | All three: `cyber-server::runtime` `delete_refuses_cyclic_child_graph_without_mutation` (failed on unchanged `c5d7e56` when written) |
| M1.3 | Remote MCP client suites on Windows (split from the stdio-fixture work, which stays open) | Windows: `cyber-app` `mcp` 74 (all 34 `remote_*`), `cyber-cli` `mcp_remote_exec` 5, `cyber-tools` `mcp_remote` 44, `mcp_protocol` 23 |
| M1.4 | `--config memory.generate=false` read-only recovery | All three: `cyber-cli::memory` `stale_recovery_and_fresh_settings_preserve_files_and_journal`, `disabled_readonly_and_unsupported_mutations_refuse_editor_start` |

Status text corrected without ticking a task:

- **Cross-session messaging.** "Linux and native Windows CI have not run this work" is stale. `cyber-app` `messaging` (19), `cyber-server` `messaging` (19), `runtime::messaging` unit tests, `cyber-cli` `messages_cli` (3), `cyber-tools` `messaging_tools` (5) and the two MCP-serve messaging cases pass on all three platforms. Tasks 14.1–14.4 stay open: the end-to-end file `messages_acceptance.rs` is not written.
- **Auto classifier and MCP OAuth (e).** Their native Windows suites pass (`auto_dispatch` 37 of 38, `cyber-server::auto` 12; `mcp::oauth::keyring::tests` 26, `mcp_oauth` 14). Both rows stay (e): every keyring test uses an in-memory vault, and the classifier still needs a live evaluator run.
- **Linux.** Every suite that is not ignored ran under bubblewrap, including `cyber-sandbox::linux` (9) and the escape exit criterion `cyber-app::sandbox_escape` (7). The manual batch A–C scenarios were not repeated on Linux.

## Not evidence

The tasks below are open, but the docs said they were waiting only for a CI run. That was wrong. The tests are in the Windows CI step and are skipped there by design:

| Test | Why it is not evidence |
|---|---|
| `cyber-tools/tests/windows_powershell.rs::native_sandboxed_powershell_write_outside_roots_is_denied` | `#[cfg_attr(windows, ignore)]`: PowerShell 7 cannot register ETW inside the AppContainer ([issue #9](https://github.com/CyberdyneCorp/cybercode-cli/issues/9)) |
| `cyber-cli/tests/exec_hooks_windows.rs::exec_sandboxed_powershell_write_outside_roots_is_denied` | Ignored on Windows, issue #9 |
| `cyber-cli/tests/exec_hooks_windows.rs::exec_project_post_tool_hook_cannot_write_home` | Ignored on Windows, issue #9 |
| `cyber-sandbox/tests/windows_confine.rs::a_killed_helpers_grants_and_profile_are_revoked_by_the_next_launch` | Ignored on Windows, issue #9 (the other three `windows_confine` cases pass) |
| `cyber-tools/tests/hook_command_launch.rs::windows_global_command_receives_event_input_and_required_sandbox_refuses` | Ignored on Windows, issue #9 |
| `cyber-tools/tests/windows_powershell.rs` background-Job case | Ignored on Windows, issue #9 (denied with no interactive approver; cause unknown) |
| `cyber-tools/tests/windows_shell.rs::missing_helper_refuses_and_sandboxed_bash_cannot_write_outside_roots` | Passes, but only asserts that the outside file is absent. That also holds when Git Bash cannot start, and there is no in-root positive control |
| `cyber-app` `mcp_stdio::*` (14), `cyber-app::browser`, `cyber-app::plugins`, `cyber-app::sandbox_escape` | `#[cfg(unix)]`: not built on Windows |

As a result, Windows enforcement and the `powershell` tool move from (b) to (c) in the acceptance plan. Sandboxed shell commands do not run on Windows today. The plan totals become 158 (a), 3 (b), 27 (c), 23 (d), 3 (e) of 214.

## Open-task counts

Open `- [ ]` tasks in the six change task lists:

| Change | Before | After |
|---|--:|--:|
| `implement-m1-1-sandbox-modes` | 21 | 19 |
| `implement-m1-2-subagents-worktrees` | 26 | 23 |
| `implement-m1-3-extensibility` | 21 | 21 (one ticked, one split out) |
| `implement-m1-4-memory-intelligence` | 12 | 11 |
| `implement-m1-5-migration-editor` | 12 | 12 |
| `implement-cross-session-messaging` | 5 | 5 |

## Gap plan

Gap types: **code** (behavior to build), **test** (behavior exists, its proof does not), **external** (a live model, real OS keyring, real browser or account), **spec** (a decision or a spec/doc mismatch). Sizes: S is up to 2 days, M up to 2 weeks, L more than 2 weeks. The order puts safety, recovery and reliable editing first, since those decide whether a first release can be trusted with a repository. Everything else follows.

### Tier 1: safety and recovery (blocks a first release)

| # | Milestone | Requirement | What is missing | Type | Size | Depends on |
|--:|---|---|---|---|:-:|---|
| 1 | M1.1 | sandbox: Windows enforcement; builtin-tools: powershell tool | Git Bash and PowerShell 7 crash at startup in the AppContainer, so `bash`, `powershell` and sandboxed hooks fail on Windows. Choose a confinement both survive (restricted token or low integrity level plus Job object and the existing ACL grants), or a clear `SandboxUnavailableError` plus escalation. Then remove every `issue #9` ignore and `support::BASH_SANDBOX`, and add an in-root positive control to the `windows_shell` case | code + spec | L | A `sandbox` spec decision (OpenSpec change, security) |
| 2 | M1.3 | plugins-marketplace: Capability declaration and enforcement | `fs.read` and `shell` are declared but not enforced; a plugin host can read anything a sandboxed command can and start subprocesses | code | M | Sandbox read allowlist and exec filter (Seatbelt, bubblewrap; Windows after #1) |
| 3 | M1.3 | mcp: elicitation | Clear the remote bridge's `Authorization` header when `cyber mcp logout` is seen during a form wait (the `cancel` reply is still sent once with the removed bearer) | code + test | S | None |
| 4 | Other P1 | snapshots-checkpoints: Rewind targets; tui: Rewind UI, Undo and redo shortcuts | `cyber sessions rewind` is still a reserved name (`commands/sessions.rs` `LATER`); there is no TUI rewind picker and no `/undo` or `/redo`. The HTTP route exists | code | M | None |
| 5 | Other P1 | snapshots-checkpoints: Non-git fallback | Snapshots outside a Git checkout are unimplemented | code | M | None |
| 6 | M1.1 | permissions-modes: accept-edits mode (full requirement) | Accept-edits proof for recursive copies, directory moves and the remaining filesystem options; unresolved cases ask today, which is safe but noisy | code + test | M | None |
| 7 | M1.1 | permissions-modes: auto mode / critical-path guard | PowerShell analysis: legacy output encoding, protected/irreversible boundaries, advanced bindings and pipelines, higher-order dispatch. Until then, PowerShell calls need individual confirmation | code + test | M | #1 for native proof |
| 8 | M1.2 | worktrees, agents-subagents: recovery | Process-death recovery, unknown-activity resolution, idle client attachment and reattachment for worktree removal and isolated children (tasks *Connect the transactional core removal protocol…* and *Finish complete recovery…*) | code + test | M | None |
| 9 | Messaging | cross-session-messaging (18 rows) | The automated end-to-end suite `crates/cyber-cli/tests/messages_acceptance.rs` (tasks 14.1–14.4), including the `SIGKILL` restart case; today these scenarios were only run by hand | test | M | None |
| 10 | M1.1 | sandbox: Windows enforcement (network) | Windows allowlist proxy: the container cannot reach the loopback proxy, so `proxy` confines like `off` and raises no domain prompts | code | M | #1 |
| 11 | M1.1 | sandbox: service lifecycle | Healthy service reuse and version or stale-registration replacement; native Git process cleanup and abandoned active-owner cancellation within the service deadline | test (+ code if a case fails) | M | None |
| 12 | M1.4 | memory: storage and recovery | Windows private recoverable storage (the Unix journal has no Windows privacy/durability equivalent), reviewed unknown-effect reconciliation across platforms, multi-client retention | code + test | M | None |
| 13 | M1.2 | agents-subagents: budgets, request identity | Conservative reserved dispatch, uncertain-reservation reconciliation and daily caps; generic pre-admission deadlines; reconciling unknown native effects without redispatch | code + test | M | None |
| 14 | Other P1 | flaky CI tests | Timing-sensitive tests and two suspected product races ([issue #8](https://github.com/CyberdyneCorp/cybercode-cli/issues/8)); CI retries once and reports them FLAKY | test + code | M | None |

### Tier 2: complete the milestones

| # | Milestone | Requirement | What is missing | Type | Size | Depends on |
|--:|---|---|---|---|:-:|---|
| 15 | M1.3 | mcp: roots, cancellation (Windows) | A Windows local-server equivalent of the Unix-only `support/mcp_stdio.rs` cases (roots, writable-root replacement, removed local tool, local cancellation) | test | S–M | A Windows stdio fixture (no python3 assumption) |
| 16 | M1.3 | mcp: sampling; cancellation | Remote sampling through the production path; server `notifications/cancelled` for a pending nested sampling request; `notifications/cancelled` when an interrupt lands mid-write of a stdio frame | code + test | S | None |
| 17 | M1.3 | hooks: Supported events | Producers for `DirectoryAdded`, `FileChanged`, `CwdChanged`, `Setup`, and `WorktreeCreate` for isolated children | code | M | P0 *Additional working directories* for `DirectoryAdded` |
| 18 | M1.3 | hooks: Windows hooks profile | Sandboxed Windows command hooks natively (`exec_project_post_tool_hook_cannot_write_home`) | test | S | #1 |
| 19 | M1.3 | plugins-marketplace: Host protocol | Core dispatch of `hook/<event>`, `provider/stream`, `catalog/transform` and `session/prompt` (logged and ignored today) | code | L | None |
| 20 | M1.4 | code-intelligence: lifecycle | Recheck live LSP generation config/trust before admission, fence revoked generations, pin managed checkouts, CLI/TUI lifecycle controls; enforced Windows LSP launch | code + test | M | #1 for Windows |
| 21 | M1.5 | compat-import: Source detection; permission conversion | Resolve config-referenced custom sources; count OpenCode SQLite sessions; convert Codex `:`-prefixed filesystem keys, `include_only`/`filters`, `mcp_elicitations = false` (reported today) | code | M | None |
| 22 | M1.5 | compat-import: Read-time MCP compatibility | The TUI prompt `Enable MCP server "<name>" from .mcp.json?` (approval is `cyber mcp trust` today) | code | S–M | A TUI/HTTP MCP digest approval surface |
| 23 | M1.5 | compat-import: Claude Code sources; Secret safety | Map `apiKeyHelper`, copy `.claude/rules`, store literal credentials in the OS keyring | code | M each | provider-credentials *Command credentials* (d) and *Secure storage*; system-context *Path-scoped rules* (c) |
| 24 | M1.1 | permissions-modes: auto-mode overrides | Remaining one-shot override, ancestor auto configuration/review and protected/irreversible proofs | code + test | M | #7 |

### Tier 3: other P1 contracts (block the P1 phase, not M1.1–M1.5)

Taken from the [acceptance plan](p1-acceptance-plan.md#other-p1-not-owned-by-an-m1x-milestone) (2026-10-19); the exec, observability, rewind and rules keys were spot-checked against `5283825`.

| # | Area | What is missing | Type | Size |
|--:|---|---|---|:-:|
| 25 | exec-mode | Structured final output (`--output-schema`), streaming JSONL input, `--color`, the `action`/`policy` fields of the denial log, `--fail-on-deny` stopping after the Turn | code | M |
| 26 | session-runtime, compaction | Per-prompt structured output for top-level Sessions; PreCompact `deny` is ignored; tool output pruning; eager start of concurrency-safe calls | code | M |
| 27 | system-context, configuration | `.cyber/rules` and `.claude/rules` loading with `paths`; `core/references`; context inspection (`/context`); configuration references | code | M |
| 28 | observability-costs | OpenTelemetry export and prompt privacy, debug traces, status-line data contract and command, `debug perf`, diagnostics bundle, doctor checks | code | L |
| 29 | provider-catalog, provider-credentials | Fallback chain, small-model selection, tool-call emulation, Gemini/Bedrock/Vertex/Azure adapters, multiple connections, command credentials, provider OAuth | code (+ external baselines) | L |
| 30 | storage-events, tui | Transcript persistence switch (`storage.persist_sessions`, `CYBER_EPHEMERAL`), retention exemptions, status line defaults, accessibility, reasoning display and `/effort` | code | M |
| 31 | web-client | The local web dashboard ([`implement-local-web-dashboard`](../../openspec/changes/implement-local-web-dashboard/proposal.md)) | code | L |
| 32 | (b) rows | Instruction imports, Usage data retention, Background work owned by the run: the named tests in the acceptance plan | test | S each |

### Tier 4: external evidence and archives

| # | Item | What is needed | Size |
|--:|---|---|:-:|
| 33 | M1.1 auto classifier | A live `model_roles.evaluator` run over a fixed, versioned suite (see the plan's class (e) list) | S |
| 34 | M1.3 MCP OAuth, plugin secrets | Real macOS Keychain and Windows Credential Manager round trips | S |
| 35 | M1.3 bundled `review` skill | Real-model review quality | S |
| 36 | M1.4 browser verification | One run of the real pinned `@playwright/mcp@0.0.41` against an approved local URL | S |
| 37 | Spec | Classify the 57 canonical P1 requirements outside the 214-row snapshot ([reconciliation](spec-doc-reconciliation.md)); decide URL-mode elicitation and `completion/complete` (not in the canonical contract) | M |
| 38 | Archives | Archive `implement-cross-session-messaging` after `add-mcp-session-server` (open: 2.2 error mapping, 3.1 non-interactive MCP clients, 3.4 recursion-guard deferrals); archive `defer-editor-integration` (P1 227 → 214 in the ROADMAP table) | S–M |

## Next step

Item 1 unblocks or completes items 7, 10, 18 and 20, and both reclassified rows. It needs a `sandbox` spec decision first, so it should start as its own OpenSpec change.
