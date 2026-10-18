# P1 acceptance plan (2026-10-17)

This plan closes the [P1 acceptance audit](p1-acceptance-audit.md) so that ROADMAP milestones M1.1–M1.5 can be signed off. Every one of the 214 in-scope P1 requirements was re-checked against the code and tests on `main` at `3ed02a3`, not only against the docs. The 214 are the 227 canonical P1 requirements minus the 13 `editor-integration` requirements that the open change `defer-editor-integration` moves to P3. That change is not archived yet, so `openspec/specs/editor-integration/spec.md` still tags them P1. Nothing was built or run for this audit (no cargo): the classification rests on reading the source, the test bodies and the specs. Rows moved to (a) by increments 1–5 were later demonstrated on the real binary and server; see [Batch A acceptance run](#batch-a-acceptance-run-increments-15-2026-10-18).

## Classes

| Class | Meaning |
|---|---|
| **a** | Done and accepted locally. Implemented, and a production-path test (App/AppHost, HTTP server, `cyber` binary or the runtime Flow) covers every SHALL clause and scenario. Native Windows CI still has to pass where the suite runs there; that is a cross-cutting sign-off gate, not a class. |
| **b** | Implemented, but the acceptance evidence is missing. The table names the test that would prove it. |
| **c** | Partially implemented. The table lists the missing contract pieces. |
| **d** | Not implemented: config-key recognition, a reserved command name or nothing at all. |
| **e** | Every local piece is done. The only remaining evidence is external (a live model, an OS keyring, real hardware). |

## Corrections to the audit

These corrections were applied to [p1-acceptance-audit.md](p1-acceptance-audit.md) in a new "Corrections (2026-10-17)" section at its top, and in the lines named below. Older dated sections are kept as history. Where a dated section is superseded, the correction section says so.

1. **Five hooks rows were checked `[x]` without full scenario coverage.** They are now unchecked.
   - *Matchers*: the `/^mcp__github__.*/` scenario is covered only by the unit test `hook_decisions` `selectors_match_mcp_regex_paths_and_dotted_conditions`; no runtime test runs a regex matcher against an MCP call.
   - *HTTP handlers*: the fail-closed timeout (`hook <id> unavailable`) is tested only through synthetic `run_test` (`hook_http` `timeout_is_nonblocking_unless_fail_closed_and_settles_local_transport`), and the reason is never asserted.
   - *MCP tool handlers*: the cited `elicitation_hook_deny_declines_through_the_real_pool` uses a command handler, not an `mcp_tool` handler, and nothing covers the canonical PostToolUse `record` scenario.
   - *Parallel execution within a group*: no test covers dedup of the same command across global and project scope; every dedup test stays within one scope.
   - *Trust for project hooks*: no TUI test renders the notice (`crates/cyber-tui/src/app.rs:1860`).
2. **Exec "Background work owned by the run".** The note "SIGINT and `--max-tokens` have no dedicated regression" is wrong. `crates/cyber-cli/tests/exec_delegation.rs` has `interrupt_cancels_queued_admission_without_interrupting_parent` (SIGINT) and `descendant_tokens_reaching_budget_stop_the_owned_delegation` (`--max-tokens`). Both run against a mock server on the delegation path. What is actually missing is real-server coverage on the forked-command and owned-Job paths.
3. **Skills table row "agents-subagents: Forked subagents | Partial"** is stale. Every clause has runtime-path tests: `forked_subagents.rs` (6), `cyber-server` `fork_context.rs`, `subtasks.rs` `user_subtask_forks_without_spawn_approval_and_preserves_child_approvals`, and `exec_subtask_command_runs_as_a_forked_reviewer_child`.
4. **Skills table, other rows.**
   - *Skill-scoped tool approvals and model*: the evidence is unnamed; it is `skill_scopes.rs` (6) and `skill_commands.rs` (7), and every scenario is covered.
   - *Bundled skills*: the only open item, real-model review quality, is not a SHALL clause.
   - *Path-triggered and forked skills*: the open item "aborted/unresolved reminder handling" is not a canonical clause.
   - *Shell output injection*: it is marked done locally, but the clause "a forked skill SHALL expand before its text is handed to the child" has no test.
5. **The "Unix-only suites" list in the 2026-10-13 hooks section is incomplete.** `lifecycle_hooks`, `subagent_hooks`, `hook_test_cli` and most `hook_command_*` cases are also Unix-gated.
6. **The "Remaining delivery areas" table is stale.**
   - *M1.3*: "MCP OAuth, remote transports" are implemented and tested (`cyber-app/tests/mcp.rs` `remote_*` and `oauth::*`).
   - *M1.4*: "LSP/formatter integration and diagnostics feedback" are implemented (`edit_feedback.rs`, `formatter_execution.rs`, `cyber-app/tests/intelligence.rs`).
   - *M1.5*: `cyber import --detect` and `cyber import <tool> --dry-run` exist (`crates/cyber-cli/src/commands/import.rs:33,132`); only writes are missing.
   - *Additional P1 contracts*: costs now have production-path tests (context meter, `cyber stats`, usage folding).
7. **The canonical-checklist intro is stale.** "Public live status/events, Location close/reconfiguration, reconnect, … remote transports, OAuth and deferred search remain open" is wrong for status/events (`GET /mcp`, `mcp.status.changed.1`), Location close, reconnect, remote transports, OAuth and deferred search (`mcp_runtime.rs` `deferred_search_loads_only_selected_schema_on_following_steps`).
8. **The "Next priorities" paragraph leaves out the zero-code M1.1 requirements.** Six sandbox requirements (Escalation requests, Excluded commands, Container detection, Sandbox CLI, Sandbox events and audit, Named sandbox profiles) and *Rule dry run* have no code beyond, at most, a reserved command name. *Environment policy* has only the exact-name `sandbox.env.allow`.
9. **Superseded dated statements, still readable as current.** These are kept as history and flagged in the corrections section:
   - "expose no memory tool or client route" (M1.4 memory document foundation). The tool, HTTP, CLI and TUI now exist.
   - "Windows mutation admission remains explicitly refused". Later sections and native jobs `56eff67`/`dbae0ad` supersede it.
   - "This formatting is not yet connected to edit results". It is connected (`crates/cyber-tools/src/tools/intelligence.rs:46-59`).
   - "Owned LSP runtime is not yet implemented". It is implemented.
   - "HTTP LSP process status, … sandboxed formatter execution remain open". These are implemented.
   - The `mcp/instructions` source key is now `core/mcp-instructions`; the legacy key is withdrawn (`renamed_source_withdraws_the_legacy_key`).
10. **MCP serve table, "Local client trust model".** The router-level LAN-peer refusal is already tested (`cyber-server/tests/mcp_serve.rs` `a_tcp_request_from_a_lan_peer_is_refused_before_any_mcp_session`). Only a real-interface run remains.
11. **Outside the audit.**
    - `ROADMAP.md:75` still says the audit lists 212 requirements (225 before the deferral). The figures are 214 and 227 (fixed 2026-10-18 in batch A).
    - `p1-status.md:5` says the TUI overhaul is "Uncommitted"; it landed in `9d606e3`.
    - `p1-status.md:18` says `cyber stats` prints "1 days"; this is fixed (`stats.rs:171-177`, `one_day_is_singular`).
    - Open task checkboxes are stale in `implement-m1-1-sandbox-modes/tasks.md` (one-shot replay and ancestor review, which later items record as done) and in `implement-m1-3-extensibility/tasks.md` lines 83 and 88 (observability, `/hooks` CLI).

## Per-milestone summary

The milestone mapping follows the ROADMAP milestone text and the `implement-m1-*` change proposals. Requirements of P1 capabilities that no M1.x milestone owns are counted under "Other P1". They do not block M1.1–M1.5 sign-off, but they do block the P1 phase.

| Milestone | Capabilities (P1 rows) | a | b | c | d | e | Total |
|---|---|--:|--:|--:|--:|--:|--:|
| M1.1 Sandbox + modes | sandbox 8, permissions-modes 7, builtin-tools catalog + powershell 2, tui mode indicator 1 | 1 | 2 | 7 | 7 | 1 | 18 |
| M1.2 Subagents + worktrees | agents-subagents 14, worktrees 9, background-tasks 15, builtin-tools monitor 1, tui tasks/threads 2 | 23 | 0 | 8 | 10 | 0 | 41 |
| M1.3 Extensibility | hooks 20, client-sdk 1, plugins-marketplace 14, mcp 6, skills-commands 5, tool-registry 3, cli-commands review 1, vcs-integration 3 | 34 | 0 | 4 | 14 | 1 | 53 |
| M1.4 Memory + intelligence | memory 13, code-intelligence 12, browser-verification 2, builtin-tools notebook_edit 1 | 26 | 0 | 0 | 2 | 0 | 28 |
| M1.5 Migration | compat-import 10 | 0 | 0 | 8 | 2 | 0 | 10 |
| Other P1 | cross-session-messaging 18, observability-costs 10, provider-catalog 4, provider-credentials 3, exec-mode 5, session-runtime 3, system-context 5, configuration 2, compaction 3, snapshots-checkpoints 3, storage-events 2, tui 4, installation-upgrade 1, web-client 1 | 1 | 3 | 18 | 42 | 0 | 64 |
| **Total** | | **85** | **5** | **45** | **77** | **2** | **214** |

Highlights per milestone:

- **M1.1.** Auto-mode configuration and override is the only row that is complete locally. Six sandbox requirements and Rule dry run have no code. `exec --yolo` turns bypass on unconditionally (`crates/cyber-cli/src/commands/exec/setup.rs:157-160`), so the eligibility gate is missing. Production dispatch on Windows still answers `SandboxUnavailableError` (`crates/cyber-sandbox/src/lib.rs:128-139`): the AppContainer primitives exist, but no tool reaches them.
- **M1.2.** Subagents are complete locally: 14 of 14 rows are (a) (increment 3). Background tasks are the gap: the only Job kind ever written is `subagent` (`crates/cyber-server/src/runtime/jobs.rs:348`), and `bash background: true` is refused (`crates/cyber-tools/src/tools/bash.rs:47`). Worktrees are complete locally: all 9 rows are (a) (increment 5).
- **M1.3.** Hooks, MCP, skills, SDK hook callbacks, org MCP controls and VCS commit/PR text are done locally (increments 1 and 4). The plugin system has no code at all (14 rows), and that also blocks the plugin hook scope and plugin-tool deferral. Supported events and command-hook sandboxing on Windows stay (c).
- **M1.4.** Memory and code intelligence are done locally (increment 2): every memory and code-intelligence row is (a). Browser verification has no code.
- **M1.5.** `cyber import` is dry-run only: writing fails with "migration writes are not implemented" (`crates/cyber-cli/src/commands/import.rs:37-41`). Agents, skills, rules, hooks and profiles are reported `not imported`. `.mcp.json` is not read at runtime. The read-time skill precedence is wrong. The ROADMAP exit criterion ("`cyber import claude` reproduces a reference Claude Code setup") is unmet.

## Batch A acceptance run (increments 1–5, 2026-10-18)

The 38 rows that increments 1–5 moved to (a) were each demonstrated end to end with the real debug `cyber` binary, a real `cyber serve` (and, for the SDK, `cyber serve --stdio` started by `Cyber.start`) against a scripted local OpenAI-compatible provider, with real stdio MCP, HTTP-hook and LSP fixtures. No live model and no internet were used. TUI scenarios ran in a pty and were captured as screens. All 41 scenarios passed; no row was reclassified, so the counts above are unchanged by the run. This is local evidence only: native Windows CI has not run on this tree.

| Milestone | Requirements demonstrated | Observed on the production path |
|---|---|---|
| M1.3 (inc 1) | Matchers, MCP tool handlers, Command handlers, HTTP handlers, Parallel execution within a group, Trust for project hooks (CLI and TUI), Decision schema | A PreToolUse matcher `/^mcp__acc__.*/` ran for both `mcp__acc__lookup` calls and not for `bash`; a PostToolUse `mcp_tool` handler called `acc.record` once per lookup with `${tool_name}`/`${tool_input.q}` filled; an exit-2 bash hook blocked the call and gave the model its stderr, an exit-1 read hook warned `hook audit exited with code 1: audit log offline` and the Turn continued; a fail-closed 1 s HTTP hook against a 4 s endpoint blocked a write with `hook policy unavailable` while a 500 on bash did not block; the same command in global and project scope ran once; untrusted project hooks were skipped with the `hook trust: 1 untrusted hook changed — review with /hooks` notice in exec and as a TUI toast, and ran after `--trust-project-hooks`; `continue:false` stopped after one request with `hook guard: ticket closed`, and `updated_input` rewrote a read with a transcript notice |
| M1.4 (inc 2) | All 12 memory and code-intelligence rows | `cyber debug memory` reported `missing-type.md` invalid; `cyber memory path|list|show` worked without creating the directory; generation guidance with its exclusion clause and the staleness notice were in the provider request (guidance absent in read-only); `PUT /api/v1/memory/settings` and TUI `/memory readonly|off|on` changed the next Turn's memory operations and the global `cyber.jsonc`; remember/forget created and removed `use-pnpm.md` and its index line; an API key was rejected with `Memory rejected: content looks like a secret`; memory HTTP CRUD published `memory.updated.1`; a read warmed `rust-analyzer` in the background (didOpen, no didSave); an `.rs` write carried fixture diagnostics and `notes.txt` started no server; `cyber doctor` reported `lsp:gopls: warn not installed; auto_install does not download servers`; `GET /lsp` showed connected and `POST /lsp/close` sent shutdown/exit; a failing formatter kept the edit and logged `exit_code: 2` without its stderr |
| M1.2 (inc 3, 5) | Session worktrees, Worktree location and branch naming, Cleanup on exit, Worktree events, Worktree management commands, Concurrency safety, Enter and exit tools, Agent tool catalogue, Concurrency cap, Manual invocation by mention | `cyber exec --worktree from-cli` and an unnamed run (adjective-noun-hex) with one JSON object on stdout; `cyber --worktree tui-spike` in a pty wrote into the checkout and on exit printed `worktree kept at <path> (branch cyber/tui-spike): …`; an unchanged run's checkout and branch were removed; `POST /worktrees` honoured the default and the custom `../wt`/`team/`/base settings and answered 400 for `Bad_Name`; the SSE stream carried `worktree.created|removed|kept.1`; `cyber worktree remove` refused a dirty checkout, `prune` removed only the old clean one, `--force` removed it, an unknown name gave `WorktreeNotFoundError`; removal during a held Turn failed with `Worktree in use by ses_<id>`; `enter_worktree`/`exit_worktree` moved `pwd` and back with `session.worktree.entered|exited.1`; the `agent` description listed `explore` and `reviewer` but not the denied `general`; `max_concurrent: 1` showed `queued` then `running`; `@explore …` ran an `explore` child without the mention in its prompt |
| M1.3 (inc 4) | Shell output injection, Eager execution during streaming, Roots and sampling, Annotations drive modes and hooks, Organization MCP controls, Hook callbacks (client-sdk), Commit and PR text generation, PR checkout and linked sessions, Local review command | A `context: fork` skill's `` !`cmd` `` ran once and only its output reached the child; a complete bash call was durable while the stream was still open (after the fix below); sampling went to the small model and was billed `mcp_sampling:acc`, and when disabled no capability was advertised and the request failed `-32601`; an open-world tool raised a `network` ask on `mcp:acc` with the network off, hook envelopes carried `tool_annotations`, and in `auto` mode a destructive tool was classified and blocked despite an allow rule; a denied URL server was never contacted and a managed server replaced the user's and connected in two projects; SDK `client.hooks.on` allowed a `docs/` write and the `src/` write still asked; `cyber git commit-msg` refused an empty index without a model call, stripped the model's trailer and added exactly the configured `git.co_author`; `cyber pr` resumed the linked Session and checked out the branch; TUI `/review` rendered the findings overlay |

Bugs found by the run and fixed, each with a regression test that fails without the fix:

1. **Eager execution on OpenAI-compatible providers.** The Chat Completions adapter completed every tool call only when the stream ended, so the requirement held only for the native adapters. `crates/cyber-llm/src/adapters/openai_chat.rs` and `mod.rs` (`ToolCalls::has`, `finish_whole`) now complete a call when the next call or text begins, and only once its arguments are already one whole JSON value, so no input is cut short. Tests: `cyber-llm` `adapters.rs::openai_chat_completes_each_call_when_the_next_one_or_text_starts`; production App with a real streaming endpoint `cyber-app` `server.rs::openai_compatible_call_is_durable_while_its_stream_is_still_open` (portable; both tests are added to the Windows job's existing filtered `cyber-app --test server` and `cyber-llm --test adapters` steps).
2. **Command-hook warnings showed Rust `Debug` text** (`hook audit exited with Some(1): …`). `crates/cyber-tools/src/hook_commands/results.rs` now says `exited with code N` or `ended without an exit code`. Tests: `cyber-tools` `hook_command_results.rs::nonblocking_exit_diagnostics_name_the_hook_and_plain_exit_code`; `tool_hook_dispatch.rs::post_tool_command_exit_one_warns_with_hook_id_and_continues` now asserts the exact text.

Observed and not fixed (none breaks a SHALL clause; each is a follow-up):

- A project memory directory that is not mode `0700` makes every Turn fail with `ContextInitializationBlocked: core/memory`. Failing closed is intended, but the message does not say why.
- Hook diagnostic notices are prefixed with the handler digest (`hook sha256:…: hook audit exited …`) instead of the hook id.
- The last tool call of an OpenAI Chat stream still completes at stream end; the protocol gives no earlier signal.
- The TUI renders a background handback as raw JSON in the transcript.

Still open after batch A: every (b), (c), (d) and (e) row above, including *Supported events* (M1.3), the plugin system, background process Jobs, browser verification, all of M1.5 and most of M1.1. The TUI scenarios (`--worktree` exit, untrusted-hook toast, `/memory`, `/review`) were driven manually in a pty; only the toast, `/memory` and `/review` also have automated TUI tests.

## Requirement table

The order is milestone, then spec order. "Native Windows CI pending" applies to almost every (a) row and is not repeated.

### M1.1 Sandbox + modes

| Requirement | Capability | Class | Evidence | Gap |
|---|---|---|---|---|
| Windows enforcement | sandbox | c | AppContainer/LPAC, ACL leases, Job Object: `crates/cyber-sandbox/src/windows_container.rs`, `windows_launch.rs`, `windows_job.rs`; native tests `cyber-sandbox/tests/windows_container_launch.rs`, `windows_job.rs`; `platform_wrap` returns unavailable at `cyber-sandbox/src/lib.rs:128-139` | Production tool dispatch never selects the AppContainer backend. Missing: recursive writable-root policy and exclusions for descendants, and the Windows proxy. Proof: native `cyber-tools/tests/windows_powershell.rs::native_sandboxed_powershell_write_outside_roots_is_denied` |
| Escalation requests | sandbox | d | No `Sandbox denied:` text, no `sandbox: "escalate"` parameter, no `sandbox.escalate` action | Whole contract |
| Excluded commands | sandbox | d | `cyber-sandbox/src/policy.rs:53-100` has no `excluded_commands` | Whole contract |
| Container detection | sandbox | d | No `.dockerenv`/`.containerenv`/cgroup/`CYBER_CONTAINER` probe | Whole contract, including the `isolation: container` header |
| Sandbox CLI | sandbox | d | `sandbox` exists only as a reserved name (`crates/cyber-cli/src/tree.rs:58,79-82`) | `test`, `explain`, `run` |
| Sandbox events and audit | sandbox | d | No `sandbox.denied.1`/`sandbox.escalated.1` in `cyber-server/src/runtime/events.rs` | Events and telemetry export |
| Named sandbox profiles | sandbox | d | `--sandbox` accepts only the three policies (`policy.rs:69-73`) | Profiles, `extends`, `deny_read` (OS plus read/glob/grep hiding), `min_profile` |
| Environment policy | sandbox | c | Only exact-name `sandbox.env.allow` (`policy.rs:99`, `env.rs:7-16`, `cyber-tools/src/sandboxing.rs:174`) | `mode` inherit/none, `set`, `deny`, globs, `login_shell`, `explain` output |
| Critical-path removal guard | permissions-modes | c | `cyber-tools/src/permissions/removals.rs:25,59`, `removals/inline.rs`, `removals/powershell.rs`, `engine.rs:477-480`; `tools.rs::critical_bash_removals_are_refused_even_with_explicit_and_saved_allows`, `inline_removals.rs` (6), `auto_dispatch.rs::critical_removal_is_refused_before_classifier_or_native_dispatch` | Advanced bash bindings, wrappers and shell flow, plus PowerShell bindings and pipelines (tasks.md). The scenario command is covered only as a unit; add `tools.rs::bypass_refuses_rm_rf_of_git_toplevel_substitution_without_execution` |
| accept-edits mode | permissions-modes | b | `engine.rs:522`, `fs.rs:325`, `patch.rs:314`, `bash.rs:142-159`; `tools.rs::accept_edits_allows_edits_inside_the_location` plus 7 `accept_edits_*` | Add `tools.rs::accept_edits_allows_write_but_keeps_non_edit_and_recursive_rm_asks` (covers `write`, `curl`/`webfetch` staying ask, `rm -r` staying ask) |
| auto mode classifier | permissions-modes | e | `cyber-server/src/runtime/auto.rs:267-282`, `cyber-tools/src/auto_permissions.rs:80-90`; `cyber-server/tests/auto.rs` (11), `auto_dispatch.rs::force_push_classifier_block_is_returned_before_command_launch`, `three_blocks_use_interactive_fallback_for_the_fourth_call` | Live evaluator suite (see the (e) list) |
| dont-ask mode | permissions-modes | b | `engine.rs:462,526`; exec defaults to dont-ask (`exec/setup.rs:165`); `parent_modes.rs::dont_ask_refuses_protected_path_asks_without_creating_an_approval` | Add `cyber-cli/tests/exec_lifecycle_hooks.rs::exec_without_tty_denies_unapproved_curl_and_continues` (real binary: `Not pre-approved (dont-ask mode)`, second model turn, exit 0) |
| bypass mode | permissions-modes | c | `engine.rs:497-519`; `tools.rs::protected_paths_ask_even_in_bypass_mode`, `deny_rules_win_over_bypass_and_hide_fully_denied_tools` | No eligibility gate (sandbox, container or `--dangerously-bypass-permissions`), no refusal error, no org-policy deny. `exec --yolo` is unconditional |
| auto-mode configuration and override | permissions-modes | a | `cyber-core/src/config/auto_mode.rs:16,46`, `runtime/auto_override.rs`, route `http/sessions.rs:258`; `auto_dispatch.rs::auto_always_block_beats_explicit_allow_without_classifier_call`, `confirmed_override_replays_exact_blocked_input_once_without_another_model_turn`; `http.rs::approve_route_returns_before_confirmation_and_preserves_exact_call_identity`; `cli.rs::auto_statistics_show_and_reset_are_checkout_scoped_and_persisted` | Org ceilings are MAY and not implemented |
| Rule dry run | permissions-modes | d | `PermissionsCmd` has only `auto` (`crates/cyber-cli/src/commands/permissions.rs:8-18`) | `cyber permissions test` and `GET /api/v1/permissions/test` |
| Capability-owned tool catalog | builtin-tools | c | Registry `cyber-tools/src/tools/mod.rs:45-81`, unknown-tool settle `host.rs:934`; `flow.rs::session_rules_deny_tools_and_hide_fully_denied_ones` | Many listed tools don't exist (monitor, pty_*, notify, send_file, cron_*, worktree, team, task, messaging tools). Add golden `advertised_tool_names_are_a_subset_of_the_canonical_registry`. Needs either those tools or a spec note that phase-gated tools may be absent |
| powershell tool | builtin-tools | c | `tools/powershell.rs:110-126`, Windows-only registration `mod.rs:76-80`; native `windows_powershell.rs` (rules, golden, timeout, closed stdin) | Sandboxed launch refused (no Windows enforcement). Permission analysis gaps: encoding, parameter abbreviations, advanced mutation |
| Permission mode indicator and cycling | tui | c | `cyber-tui/src/app.rs:16,1143,1209-1232,794`; `tests.rs::shift_tab_cycles_modes_and_ctrl_c_quits_when_empty`, `mode_cycle_includes_auto_and_excludes_unattended_modes`, `pending_mode_keeps_the_effective_mode_visible_and_cycles_the_selection`, `bypass_requires_confirmation_from_slash_command_and_picker` | Org-policy `modes.disable` and its message are missing. Add `cyber-app/tests/server.rs::mode_switch_to_plan_is_durable_and_next_turn_denies_writes` |

### M1.2 Subagents + worktrees

| Requirement | Capability | Class | Evidence | Gap |
|---|---|---|---|---|
| Agent tool spawns subagents | agents-subagents | a | `cyber-tools/src/tools/agent.rs:19-31,112-210,484-496,845-875`; `subagents.rs::foreground_child_returns_only_its_final_answer_and_durable_identity`, `denied_or_unknown_agents_do_not_create_children` | — |
| Structured subagent output | agents-subagents | a | `agent.rs:1047-1095`; `structured_subagents.rs` (12), `cyber-server/tests/structured_results.rs` | — |
| Background subagents and handback | agents-subagents | a | `agent.rs:819-866`, `runtime/jobs.rs:548-565`; `background_subagents.rs::background_returns_before_completion_and_survives_parent_interrupt`, `busy_parent_keeps_handback_queued_until_its_safe_boundary` | — |
| Forked subagents | agents-subagents | a | `agent.rs:731,847`, TUI `/fork` and `/subtask`; `forked_subagents.rs` (6), `fork_context.rs`, `subtasks.rs::user_subtask_forks_without_spawn_approval_and_preserves_child_approvals` | — |
| Resume subagents by name | agents-subagents | a | `agent.rs:374-438,795-815`, `runtime/names.rs:90`; `subagent_resume.rs` (11), `subagent_names.rs` | — |
| Worktree isolation for subagents | agents-subagents | a | `cyber-tools/src/worktrees/child.rs:27-28,434-610`; `isolated_subagents.rs::concurrent_children_edit_the_same_filename_in_separate_checkouts`, `a_clean_isolated_child_removes_its_checkout_and_branch` | — |
| Concurrency cap | agents-subagents | a | `cyber-tools/src/subagents.rs:17-32`, `tools/agent.rs` `subagent_slot` (`session.tool.progress` `queued`/`running`); `subagents.rs::concurrency_waits_fifo_without_creating_queued_children`, `queued_foreground_spawn_reports_queued_progress`; App over HTTP `server.rs::agent_catalogue_and_queued_progress_reach_http_clients` | — |
| Nesting depth | agents-subagents | a | `agent.rs:404-418`; `subagents.rs::depth_limit_refuses_before_child_creation` | — |
| Subagent permission inheritance | agents-subagents | a | `cyber-tools/src/parent_permissions.rs`, `agent.rs:595-604`; `parent_permissions.rs`, `parent_modes.rs` (14), `requests.rs::parent_rejection_does_not_decline_a_child_request` | — |
| Subagent result summarization | agents-subagents | a | `agent.rs:996-1022`; `subagents.rs::oversized_final_answer_has_a_utf8_preview_and_full_managed_artifact` | — |
| Cost attribution | agents-subagents | a | `runtime/child_usage.rs`, `budget.rs:62-111`; `child_usage.rs::nested_live_usage_is_attributed_once_to_each_ancestor_without_changing_own_totals`, `budgets.rs` | — |
| Manual invocation by mention | agents-subagents | a | `cyber-core/src/agent_mentions.rs`, `cyber-tui/src/perform.rs:581-598,1073-1090`, `exec/delegation.rs:21`; `exec_delegation.rs::exec_agent_mention_spawns_explore_child_on_embedded_server` (real binary, embedded server, local provider, no `agent` rule), runtime `subtasks.rs::named_user_delegation_is_fresh_and_keeps_child_approvals_and_handback`, TUI `@` menu tests | — |
| Agent thread switching | agents-subagents | a | `cyber-server/src/http/children.rs:18-80`; `http.rs::child_threads_distinguish_running_waiting_and_failed`; TUI `child_thread_commands_open_existing_threads_and_preserve_steer_delivery` | — |
| Agent tool catalogue | agents-subagents | a | `BuiltinHost::with_agent_catalogue` (`host.rs`), `tools/agent.rs::with_catalogue`; `subagents.rs::agent_tool_description_lists_permitted_subagents_per_turn`; App over HTTP `server.rs::agent_catalogue_and_queued_progress_reach_http_clients` | — |
| Session worktrees | worktrees | a | `cyber-cli/src/commands/tui.rs:47-80`, `http/worktrees.rs:162`, `cyber-tools/src/worktrees.rs:431-540`; `cli.rs::exec_worktree_starts_in_owned_checkout_and_keeps_json_stdout_clean` (branch `cyber/from-cli`, default root, Session directory), `exec_worktree_without_name_generates_adjective_noun_hex`, app `public_worktree_creation_streams_setup_and_retains_failed_session` | — |
| Worktree location and branch naming | worktrees | a | `cyber-core/src/worktrees.rs:36-110`, `repository.rs:273,396-417`; `cyber-app/tests/server.rs::public_worktree_creation_honours_default_and_custom_root_prefix_and_base` (default `<data>/worktrees/<project_id>/<name>`, `../wt` root, `team/` prefix, `base` commit, invalid name 400), core `custom_root_branch_and_commit_base_are_applied` | — |
| Untracked file inclusion | worktrees | a | `cyber-core/src/worktrees/includes.rs:19-80`; `worktree_setup.rs::creation_automatically_attaches_setup_and_preserves_failed_session`, `inclusion.rs` | — |
| Setup commands | worktrees | a | `cyber-core/src/worktrees/setup.rs`, `cyber-tools/src/worktrees.rs:652-720`; app `public_worktree_creation_streams_setup_and_retains_failed_session`, `worktree_setup_output.rs`, `worktree_setup_journal.rs` | — |
| Enter and exit tools | worktrees | a | `cyber-tools/src/tools/worktree.rs`, `worktrees/lifecycle.rs` (`enter_worktree_tool`, `exit_worktree_tool`), `cyber-server/src/runtime/worktree_events.rs` (`session.worktree.entered.1`/`exited.1`, the Drain lease moves to the new Location), `TurnWorktree` offering; `worktree_setup.rs::model_enter_worktree_resolves_later_paths_in_spike_and_exit_returns`, `exit_worktree_is_not_offered_to_isolated_children`, `exiting_an_entered_existing_worktree_keeps_it` (runtime Flow, `worktree` permission); app `server.rs::enter_and_exit_worktree_through_the_app` (per-Turn tool offering through AppHost, same-step calls after a move are not dispatched, idle entered Session fences `DELETE /worktrees/{name}`, exit removes the created checkout; Windows job) | — |
| Cleanup on exit | worktrees | a | Child cleanup (`worktrees/child.rs`) reused for top-level Sessions (`ChildWorktree::top_level`, `cleanup_session_worktree`, `POST /sessions/{id}/worktree/cleanup`), called by `cyber exec --worktree` and `cyber --worktree` on exit; `cli.rs::exec_worktree_without_changes_is_removed_on_exit`, `exec_worktree_starts_in_owned_checkout_and_keeps_json_stdout_clean` (kept path and branch printed), app `worktree_lifecycle_publishes_created_removed_and_kept_events`; `ask` without a client to confirm keeps the checkout | — |
| Worktree management commands | worktrees | a | `commands/worktrees.rs` (`list`, `remove [--force]`, `prune [--older-than]`, default 14), `DELETE /api/v1/worktrees/{name}` (`http/worktrees.rs`, `lifecycle.rs::remove_worktree`), listing `created_ms`; `cli.rs::worktree_remove_refuses_dirty_without_force`, `worktree_prune_removes_only_older_clean_worktrees`, app `worktree_lifecycle_publishes_created_removed_and_kept_events` (409 dirty, 404 missing) | — |
| Concurrency safety | worktrees | a | Repository lock `cyber-core/src/worktrees.rs:137-163`, leases `activity.rs`; `cli.rs::worktree_remove_while_session_active_fails_in_use` (`Worktree in use by ses_<id>`, even with `--force`, against the shared service), `worktree_lock.rs`, `drain_lease_is_held_during_tools_and_settled_after_completion` | — |
| Worktree events | worktrees | a | `LiveEvent::Worktree` (`runtime/worktree_events.rs`, `http/events.rs`) on the requesting Location's stream, from creation (`POST /worktrees`, isolated children, `enter_worktree`), removal and exit cleanup; app `worktree_lifecycle_publishes_created_removed_and_kept_events` | — |
| Background bash | background-tasks | d | `bash.rs:47` and `powershell.rs:43` refuse `background: true` | Bash Job, log file, survival past the Turn |
| Job registry | background-tasks | c | `runtime/jobs.rs:20-49,127`, restart recovery (`cyber-app/src/lib.rs:248`); `jobs.rs::shutdown_settles_ownership_and_restart_delivers_one_interrupted_notice` | Only kind `subagent`; no `bash/monitor/pty/workflow` kinds, no `command` field |
| Completion notices | background-tasks | c | `notify_job` (`jobs.rs:550`) with 20-line tail; `busy_parent_keeps_handback_queued_until_its_safe_boundary` | No `notify: false`; `exit_code` is always null |
| Reading job output | background-tasks | d | Only the subagent overflow file (`agent.rs:1053-1060`) | Live log, `background.max_output_mb`, truncation marker |
| Monitor tool | background-tasks | d | No `monitor` tool | Whole contract |
| Tasks view | background-tasks | c | `/tasks`, `/ps`, `/stop` (`app.rs:1412-1413`, `perform.rs:1161-1235`); TUI `tasks_picker_opens_children_and_stops_selected_jobs`; `http.rs::background_job_routes_list_scope_and_stop_a_child` | No workflow, monitor or PTY items; no log view; no production App test |
| Stop kills process trees | background-tasks | c | `task_stop` (`tools/task.rs:12`), `cancel_job` (`jobs.rs:568`) | SIGTERM to the process group or Job Object, 5 s then SIGKILL; grandchild test |
| Concurrency limits | background-tasks | d | — | `background.max_jobs` and the limit message |
| Jobs end with their session | background-tasks | c | `cancel_session_jobs` (`jobs.rs:584`); `deleting_parent_cancels_background_children_without_handback` | Subagent Jobs only; no bash dev-server or client-disconnect case |
| PTY sessions | background-tasks | d | No PTY code | Whole contract |
| Model interaction with PTYs | background-tasks | d | No `pty_*` tools | Whole contract |
| Notify tool | background-tasks | d | TUI OSC 9 only (`cyber-tui/src/osc.rs:57`); no tool | Whole contract (Device push is external later) |
| Send file to user | background-tasks | d | — | Whole contract |
| Background job events | background-tasks | c | `job.started/ended/cancelled/notified.1` (`jobs.rs:13-16`), `JobEnded` hook; `lifecycle_hooks.rs::job_ended_observes_a_settled_background_job` | No live `job.output.1`; no exit code |
| Background commands from the user | background-tasks | d | No `&` prefix or Ctrl+B | Whole contract |
| monitor tool | builtin-tools | d | Not registered in `tools/mod.rs` | Whole contract (shared with background-tasks Monitor) |
| Background tasks view | tui | c | Same as Tasks view; `palette.rs:111,116` | Monitors, runs and loops missing; `x` is not bound; no production-path test |
| Subagent threads and side chat | tui | c | `/agent`, `/subagents` (`app.rs:1411`); `child_thread_commands_open_existing_threads_and_preserve_steer_delivery` | `/btw`/`/side` overlay is absent |

### M1.3 Extensibility

| Requirement | Capability | Class | Evidence | Gap |
|---|---|---|---|---|
| Hook configuration | hooks | a | `cyber-core/src/config/hooks.rs:135,158,208`; `config.rs::hook_configuration_resolves_all_p1_handlers_and_defaults_without_execution`; app `hook_catalog_api_reloads_location_trust_and_redacts_without_execution` | — |
| Hook scopes and merge order | hooks | c | `cyber-core/src/hooks.rs:101,131,133`; `exec_lifecycle_hooks.rs::exec_applies_managed_only_policy_and_invocation_trust` | Plugin scope never produced (`hooks.rs:28,165`); depends on plugins |
| Supported events | hooks | c | Producers in `tool_hooks.rs`, `lifecycle_hooks.rs`, `runtime/lifecycle*.rs`; `application_emits_runtime_lifecycle_hooks_with_session_identity`; increment 1 added `Setup` (first root Session of a Location, `runtime/mod.rs` `create_session`), `ConfigChange` (Safe Boundary digest, `runtime/lifecycle.rs` `config_change`, `ToolHost::config_digest`) and `WorktreeRemove` (isolated child cleanup, `worktrees/child.rs` `report`): app `application_emits_setup_and_config_change_hooks`, `isolated_subagents.rs::worktree_remove_hook_observes_removal` | No producer for `DirectoryAdded` (needs P0 *Additional working directories*: `--add-dir`, `POST /sessions/:id/directories`), `FileChanged` (no file watcher) or `CwdChanged` (no working-directory change; a child rebind keeps its path); no `cyber --init` trigger for `Setup`; `WorktreeCreate` not run for isolated children; `WorktreeRemove` runs as a Session at its current Location, so it runs for `exit_worktree`, an entered Session's exit cleanup and `cyber worktree remove|prune` of a checkout a Session entered (`--force`) or an isolated child owned (`lifecycle.rs::removal_owner`; app `entered_worktree_user_removal_runs_worktree_remove_and_force_returns_sessions`), but not for the removal of a checkout a top-level Session was started in, whose Location it was |
| Matchers | hooks | a | `hooks/selectors.rs`; unit `selectors_match_mcp_regex_paths_and_dotted_conditions`; `absolute_file_target_matches_relative_paths_and_duplicate_command_runs_once`; `mcp_runtime.rs::regex_matcher_runs_for_mcp_tool_and_not_for_bash` (real stdio server, `/^mcp__shared__.*/` runs for the MCP call and not for `bash`) | — |
| Command handlers | hooks | a | `hook_commands/launch.rs:405,478-483`, `results.rs`; `command_errors_are_nonblocking_with_live_diagnostics_and_no_raw_receipt_io`; Windows `exec_hooks_windows`; `tool_hook_dispatch.rs::exit_two_pre_tool_command_blocks_with_stderr_reason` (bash call not run, model gets the stderr reason) and `post_tool_command_exit_one_warns_with_hook_id_and_continues` (warning text `hook <id> exited with code N: <stderr>`; `hook_command_results.rs::nonblocking_exit_diagnostics_name_the_hook_and_plain_exit_code`) | — |
| HTTP handlers | hooks | a | `cyber-tools/src/hook_http.rs`; `http_policy_decision_denies_real_builtin_write_before_effects`; `tool_hook_dispatch.rs::http_fail_closed_timeout_blocks_call_with_hook_unavailable` (reason `hook policy unavailable`, write not done) and `http_non_2xx_is_nonblocking_on_a_real_call` | — |
| Prompt handlers | hooks | a | `hook_prompt.rs`; `prompt_policy_denies_a_real_builtin_write_and_bills_the_session` | — |
| MCP tool handlers | hooks | a | `hook_mcp.rs:216,235`; `mcp_hook_synthetic_templates_preserve_types_and_once_without_session_admission`; `mcp_tool_hook_sends_the_stored_bearer`; `mcp_runtime.rs::post_tool_use_mcp_tool_records_payload_after_each_call` (the `record` tool receives the PostToolUse payload once per call) | — |
| Decision schema | hooks | a | `hooks/decisions.rs:40`, `tool_hooks.rs:128`; `rewrites_feed_later_conditions_and_are_schema_validated_before_effects`, `continue_false_sticks_over_a_later_continue_true_and_stops_after_the_turn`; `UserPromptSubmit` `continue:false` rides with the admitted row and ends the Drain after its Turn (`runtime/lifecycle.rs` `PromptEffects`, `model.rs` `hook_stops`): `lifecycle_hooks.rs::user_prompt_submit_continue_false_stops_after_the_turn`, `user_prompt_submit_continue_false_refuses_prompts_without_a_turn`; rewrites commit as `hook_rewrite` records: `tool_hook_dispatch.rs::rewrite_is_visible_as_hook_notice_in_transcript`; both through `cyber exec`: `exec_applies_prompt_stop_and_records_input_rewrites_through_the_app_host` | — |
| Decision merging | hooks | a | `decisions.rs:79`, `tool_hooks.rs:398-451`; `concurrent_permission_decisions_merge_in_declared_order_after_reverse_completion` | — |
| Interaction with permissions | hooks | a | `tool_hooks.rs:114,147`; `command_hook_denies_write_even_in_bypass_mode`, `permission_request_allow_cannot_override_hard_denials_and_is_not_run_for_allowed_calls` | — |
| Stop hooks and loop prevention | hooks | a | `runtime/lifecycle.rs:20,61,83`; `blocking_stop_admits_the_reason_and_continues_once`, `continuation_cap_stops_with_a_warning_and_a_new_prompt_resets_it` | — |
| Timeouts and async hooks | hooks | a | `config/hooks.rs:130,208`, `hook_async.rs:92-130`; `exec_runs_async_hooks_in_background_and_admits_context_at_next_boundary` | — |
| Parallel execution within a group | hooks | a | `tool_hooks.rs:451`; `post_commands_overlap_up_to_configured_limit_and_deduplicate_at_execution`; `exec_lifecycle_hooks.rs::exec_runs_a_command_defined_in_global_and_trusted_project_once` | — |
| Sandboxing of command hooks | hooks | c | `hook_commands/launch.rs:405-420,504`; `mandatory_hook_sandbox_masks_credentials_and_denies_home_writes` | Windows has no enforcing hooks profile, so these hooks refuse. Add `exec_lifecycle_hooks.rs::exec_project_post_tool_hook_cannot_write_home` (outcome `error`, Turn continues) |
| Trust for project hooks | hooks | a | `hooks.rs:68`, `tool_hooks.rs:849`; `exec_applies_managed_only_policy_and_invocation_trust`, `untrusted_handlers_share_one_notice_per_session_and_leave_skipped_receipts`; TUI `hook_notice_event_shows_untrusted_review_hint` (real App, HTTP event stream, rendered toast) | — |
| Hooks viewer and CLI | hooks | a | `cyber-cli/src/commands/hooks.rs:17-45`, `cyber-tui/src/hook_definitions.rs:143`; `cli.rs::hook_review_approves_exact_digests_and_revokes_obsolete_definitions`, `hook_test_cli`, `hook_history` | — |
| Hook context injection on lifecycle events | hooks | a | `runtime/lifecycle.rs:144,170`; `user_prompt_context_is_admitted_immediately_before_the_prompt`, `session_start_context_is_admitted_once_at_the_first_turn` | — |
| Hook observability | hooks | a | `runtime/hooks.rs:18,237`; `committed_start_and_terminal_receipts_survive_runtime_restart_without_raw_io` | — |
| Conditional, one-shot and annotated handlers | hooks | a | `selectors.rs`; `once_command_runs_once_per_session_and_messages_stay_user_visible`, `once_admission_is_exclusive_across_runtimes_and_survives_unknown_restart` | Spec intent: no payload carries `git.*`, so a "skip on main" condition passes only because the field is absent |
| Hook callbacks | client-sdk | a | `client.hooks.on` (`sdk/typescript/src/tools.ts`, `client.ts`), `v1.hook.register`/`unregister` and `hook/execute` (`cyber-server/src/http/rpc.rs`, `remote_tools.rs`), `ClientHooks` (`runtime/client_hooks.rs`), `HookScope::Client` and `HookDefinition::client` merged after every configured handler (`cyber-core/src/hooks.rs` `with_client`), durable receipts (`cyber-tools/src/hook_client.rs`); SDK e2e `hooks.test.ts::pre_tool_use_client_handler_allows_docs_edits_and_records_client_scope` (real `cyber serve --stdio`), app `server.rs::client_pre_tool_use_handler_approves_docs_edits_through_the_app`, `server.rs::client_hook_handlers_do_not_reach_other_clients_sessions`, `http.rs::client_hook_registration_drops_on_disconnect` (includes an in-flight execution failing at disconnect) | — |
| Plugin manifest | plugins-marketplace | d | Only protected-path names (`cyber-sandbox/src/policy.rs:144-153`) | Whole contract |
| Component discovery defaults | plugins-marketplace | d | — | Whole contract |
| Install scopes | plugins-marketplace | d | — | Whole contract |
| Plugin sources | plugins-marketplace | d | — | Whole contract |
| Plugin CLI | plugins-marketplace | d | Reserved name (`tree.rs:54`) | Whole contract |
| Plugin host process | plugins-marketplace | d | — | Whole contract |
| Host protocol | plugins-marketplace | d | — | Whole contract |
| Restart and backoff | plugins-marketplace | d | — | Whole contract |
| Scoped registrations | plugins-marketplace | d | `ToolScope::Plugin` declared, never used (`runtime/host.rs:182-184`) | Whole contract |
| Capability declaration and enforcement | plugins-marketplace | d | — | Whole contract |
| User configuration | plugins-marketplace | d | — | Whole contract |
| TypeScript plugin kit | plugins-marketplace | d | `sdk/typescript` is the client SDK only | Whole contract |
| Pure mode | plugins-marketplace | d | No `--pure`/`CYBER_PURE` | Whole contract |
| Trust and signing | plugins-marketplace | d | — | P1 part: digest trust, enable prompt, `unsigned` marker |
| Tool search and deferred loading | mcp | a | `runtime/deferred_tools.rs:37`, `cyber-app/src/host.rs:124`; `deferred_search_loads_only_selected_schema_on_following_steps`, `loaded_schemas_are_durable_additive_and_session_local`, golden `tool_search_golden` | — |
| OAuth | mcp | e | `cyber-tools/src/mcp/oauth/`; `cyber-app/tests/mcp.rs` `oauth::*` (~30), `mcp_oauth_cli.rs::mcp_auth_acquires_and_stores_a_token_through_the_real_binary`, `exec_refreshes_an_expiring_oauth_token_before_calling_a_remote_tool` | Native OS keyring round trips (see the (e) list) |
| Elicitation | mcp | a | `mcp/elicitation.rs`; `remote_elicitation_through_app_host_accept_decline_cancel`, `remote_elicitation_unattended_app_declines_without_question`, `exec_declines_remote_elicitation` | — |
| Roots and sampling | mcp | a | `mcp/roots.rs`, `sampling.rs`, `sampler.rs`, `launch.rs:694,817`; `sampling_routes_to_small_model_only_after_its_own_permission_and_bills_the_server`; app `local_roots_reflect_location_and_writable_roots`, `mcp.rs::remote_sampling_routes_to_small_model_through_app_host` (remote Streamable HTTP server, a real Turn, `model_roles.small`, usage `mcp_sampling:remote`) and `remote_sampling_disabled_returns_method_not_found` (no `sampling` capability, `-32601`, no model request) | — |
| Organization MCP controls | mcp | a | `policy.mcp` allow/deny patterns and managed `mcp` servers in `cyber-core/src/config/managed.rs`, whole-entry replacement in `config/load.rs`, enforced in `cyber-tools/src/mcp/authority.rs` `authorize_server` and reported by `pool.rs` `status_problem`; core `config.rs::mcp_policy_matches_name_command_and_url_and_managed_servers_replace_user_definitions`, `malformed_mcp_policy_fails_closed_for_every_configured_server`; app `mcp.rs::denylisted_url_pattern_is_never_contacted` and `managed_policy_server_is_provided_to_every_location` (production App and `GET /mcp`, run in a child process that sets `CYBER_MANAGED_POLICY_DIR`) | — |
| Server options | mcp | a | `runtime/mod.rs:188`, `mcp/required.rs`, `mcp/headers_command.rs`, `pool.rs:1125-1138`; `required_server_waits_before_promoting_the_first_prompt`, `remote_headers_command_through_app_host` | — |
| Remote skill sources | skills-commands | a | `cyber-tools/src/remote_skills.rs`; app `remote_skill_source_fetches_caches_and_serves_stale_offline`; `skills_trust_approves_a_cached_project_remote_source` | — |
| Skill-scoped tool approvals and model | skills-commands | a | `cyber-tools/src/skill_permissions.rs`; `skill_scopes.rs` (6), `skill_commands.rs` (7) | — |
| Shell output injection | skills-commands | a | `cyber-core/src/skills/template.rs:65`, `runtime/mod.rs:1090-1116`; app `command_shell_injection_*` (5), `exec_template_expansion.rs`, app `server.rs::forked_skill_shell_injection_expands_once_before_the_child` (the child receives the substituted text; the command ran once, also after an exact retry) | — |
| Bundled skills | skills-commands | a | `cyber-core/src/skills/bundled.rs:6-14`; `bundled_skills.rs` (review, batch), app `bundled_skill_catalogue_and_project_overrides_share_command_precedence` | — |
| Path-triggered and forked skills | skills-commands | a | `path_skills.rs`, `forked_skills.rs`, `skill_scopes.rs::disallowed_tool_patterns_override_skill_grants_and_bypass` | — |
| Deferred tools and tool search | tool-registry | c | `deferred_tools.rs:37`, `ToolScope::deferrable` | Plugin tools (no plugin registration source exists) |
| Eager execution during streaming | tool-registry | a | `runtime/drain.rs:960-972` commits `tool.called.1` mid-stream; `cyber-server/tests/runtime.rs::complete_tool_call_is_durable_before_the_stream_finishes` (the call is in the store, also through a second store handle, while the provider stream is held open; its result is in the next request); the OpenAI Chat adapter completes a call as soon as the next call or text starts (`openai_chat_completes_each_call_when_the_next_one_or_text_starts`), so an OpenAI-compatible provider's call is durable mid-stream too (app `server.rs::openai_compatible_call_is_durable_while_its_stream_is_still_open`); only the last call of a stream still completes at its end, where the protocol gives no earlier signal | Starting `concurrency_safe` calls before the stream ends is optional (MAY) and not implemented |
| Annotations drive modes and hooks | tool-registry | a | Plan mode keeps `readOnlyHint` tools (`mcp/pool.rs:942`); `ToolHints` with MCP defaults (`cyber-tools/src/registered.rs`), `Policy::forces_classifier` (`permissions/engine.rs`, `host.rs` `authorize`), `open_world_gate` with `sandboxing::network_off`, `tool_annotations` in envelopes (`tool_hooks.rs` `dispatch_tool_event`); `mcp_runtime.rs::open_world_tool_asks_network_when_sandbox_network_off`, `destructive_tool_is_always_classified_in_auto_mode` (real stdio server) | — |
| Review command | cli-commands | a | `cyber-cli/src/commands/review.rs:16-81`; `review_command.rs` (6), `cli.rs::review_is_a_shipped_command_that_documents_its_target_grammar` | — |
| Local review command | vcs-integration | a | `cyber-core/src/review`, TUI `app.rs:961-971`; `bundled_skills.rs` (~20), TUI tests against canned HTTP, TUI `review_against_app_server_renders_findings_and_ignores_unrelated_jobs` (production App over HTTP: `/review` posts the command, the event stream drives the delegation lookup and the findings overlay, and a background `explore` Job that ends mid-review is ignored) | — |
| Commit and PR text generation | vcs-integration | a | `cyber-app/src/git_text.rs`, `git.co_author` (`cyber-core/src/config/git.rs`), `cyber git commit-msg|pr-description` (`cyber-cli/src/commands/git.rs`), `POST /git/commit-message|pr-description` (`cyber-server/src/http/vcs.rs`), TUI `/commit` and `/pr-description` (`perform.rs` `git_text`); `cyber-cli/tests/git_commit_msg.rs::commit_msg_has_no_co_author_by_default`, `pr_description_drafts_title_and_body_from_the_branch`; TUI `commit_and_pr_description_prefill_commands_from_the_app_server` | — |
| PR checkout and linked sessions | vcs-integration | a | `session.pr.linked.1` (`cyber-server/src/runtime/pr_links.rs`), recorded by `bash` (`cyber-tools/src/tools/pr_links.rs`), `GET /sessions?pr=`, `cyber pr` and `--from-pr` (`cyber-cli/src/commands/pr.rs`, TUI `Start::PickPr`); `cyber-cli/tests/pr_command.rs::pr_checkout_resumes_the_linked_session` (real binary, shared server, fake `gh`; Unix-only) | Native Windows run of `cyber pr` (the fake `gh` is a shell script) |

### M1.4 Memory + intelligence

| Requirement | Capability | Class | Evidence | Gap |
|---|---|---|---|---|
| Memory locations | memory | a | `cyber-core/src/memory/mod.rs:255`, `tools/memory.rs:126-142`; `memory.rs::project_and_global_scopes_do_not_alias_in_a_repository` | — |
| Memory file format | memory | a | `memory/mod.rs:88-131`, `cyber debug memory`; core validation tests, `cyber-cli/tests/memory.rs::debug_memory_lists_note_missing_type_as_invalid` | — |
| Index loading | memory | a | `memory/storage.rs:301`, `cyber-tools/src/memory_context.rs`; `only_bounded_index_is_loaded_without_opening_individual_notes` | — |
| Memory tool | memory | a | `tools/memory.rs:14-24,107-124`, `host.rs:294-301`; `writes_replace_same_name_update_requires_existing_and_delete_keeps_index` | — |
| Automatic memory generation | memory | a | Guidance `memory_context.rs:13`; `memory_context.rs::generation_guidance_excludes_derivable_and_conversation_only_facts` (every inclusion and exclusion clause, and none of them when read-only), `empty_memory_provides_guidance_without_creating_directories` | — (a live-model eval of what the model saves stays optional) |
| Memory toggles | memory | a | `memory/mod.rs` `MemorySettings`/`MemoryState`, `config/edit.rs` (comment-preserving global write), `PUT /api/v1/memory/settings`, TUI `/memory on\|off\|readonly`; TUI `memory_readonly_off_on_commands_change_tool_and_context` (real App over HTTP: persisted file, next-turn tool enum and `core/memory` context for each state), app `memory_settings_persist_globally_and_report_project_overrides`, `disabled_and_invalid_settings_hide_definition_and_refuse_direct_dispatch` | — |
| Secret redaction | memory | a | `memory/secrets.rs`, `mod.rs:34`; core `write_admission_rejects_credentials_keys_and_high_entropy_in_body_or_frontmatter`, `cyber-tools/tests/memory.rs::api_key_reference_memory_is_rejected_with_canonical_message` | — |
| Deduplication and updates | memory | a | `tools/memory.rs:196-237,277`; `writes_replace_same_name_update_requires_existing_and_delete_keeps_index` | — |
| Staleness notice | memory | a | `memory_context.rs:12`; full guidance text asserted in `generation_guidance_excludes_derivable_and_conversation_only_facts`, also when read-only | — (live-model eval optional) |
| Memory command | memory | a | TUI `/memory`, CLI `commands/memory.rs:55-137`; global-scope CLI tests, `cyber-cli/tests/memory.rs::project_path_prints_repository_memory_directory` (JSON and text, repository identity, no directory created) | — |
| Memory changes during a Session | memory | a | `memory_context.rs`; `another_session_receives_index_change_without_rewriting_its_baseline` | — |
| Explicit remember requests | memory | a | `memory_context.rs:13`; `actual_memory_tool_publishes_one_independent_durable_change_after_note_and_index`, `memory_context.rs::scripted_forget_request_deletes_note_and_index_line` | — (live-model eval optional) |
| Memory HTTP API | memory | a | `cyber-server/src/http/memory.rs:37-42`; app `actual_http_crud_publishes_once_and_replays_after_response_cache_disposal` (global), `project_put_updates_note_and_index_and_publishes_memory_updated` (project PUT/DELETE, index line, `memory.updated` live event); SDK `memory.test.ts` | — |
| LSP enablement | code-intelligence | a | `cyber-core/src/config/intelligence.rs:50-60,155-160`, `intelligence/discovery.rs:40-52,93`; `search_prefers_path_then_cache_and_never_runs_or_creates_candidates`, `edit_feedback.rs::autodetected_builtin_rust_analyzer_activates_for_rs_files`, `cli.rs::doctor_lists_missing_gopls_as_not_installed_without_download`. `lsp.auto_install` is validated but installs nothing; doctor says so | — |
| Built-in servers | code-intelligence | a | `intelligence/catalogue.rs:53-176`; `all_canonical_servers_have_launch_root_extension_and_install_definitions`, `lsp_status_lists_local_integrations_without_database_or_process_startup` | — |
| Custom and overridden servers | code-intelligence | a | `config/intelligence.rs:132-148`, `discovery.rs:93-125`, `lsp/pool.rs:833`; `custom_server_requires_extensions_but_builtin_override_can_omit_them` | — |
| Lazy spawning and root detection | code-intelligence | a | `discovery.rs:145-184`, `lsp/connection.rs:13`; `lazy_start_deduplicates_refusal_and_exposes_actual_broken_status`, `nearest_roots_get_independent_owned_processes` | — |
| Diagnostics after edits | code-intelligence | a | `tools/intelligence.rs:14-60`, `lsp/documents.rs:131-134`, `lsp/diagnostics.rs:60-72`; `all_four_edit_tools_append_fresh_errors_and_write_limits_new_other_files` | — |
| lsp tool | code-intelligence | a | `tools/lsp.rs:22-117`; `navigation_operations_validate_positions_paths_previews_and_result_limit`, golden `lsp.txt` | — |
| Read warms servers | code-intelligence | a | `tools/fs.rs:53-80`; mock-factory `successful_read_warms_in_background_without_waiting_for_discovery`, `edit_feedback.rs::read_opens_document_in_background_without_waiting_for_diagnostics` (real framed server) | — |
| LSP status and shutdown | code-intelligence | a | `http/intelligence.rs:59-62`, `connection.rs:14`; app status and lifecycle tests, `cyber-app/tests/intelligence.rs::lsp_status_reports_connected_root_and_close_exits_the_process` (connected root over HTTP; close sends `shutdown` then `exit` and the process ends) | — |
| Formatter enablement and detection | code-intelligence | a | `intelligence/formatters.rs:46-168`; `formatter_discovery.rs`, `cli.rs::formatter_status_observes_project_markers_without_evaluating_configuration` | — |
| Custom formatters | code-intelligence | a | `config/intelligence.rs:100-125`, `tools/formatters.rs:142-147`; `sequential_formatters_use_absolute_file_location_and_configured_environment` | — |
| Formatter execution | code-intelligence | a | `tools/formatters.rs:15-189`; `failed_formatter_does_not_fail_the_edit_or_skip_the_next_formatter`, `formatter_cannot_write_outside_the_sandbox_and_the_edit_still_succeeds`, `formatter_failure_is_logged_with_id_and_exit_code` | — |
| Formatter status | code-intelligence | a | `GET /api/v1/formatters`, `cyber fmt status`; app `formatter_status_*` (3) | — |
| Supported optional browser integration | browser-verification | d | No code | Whole contract |
| Verification artifacts and lifecycle | browser-verification | d | No code | Whole contract |
| notebook_edit tool | builtin-tools | a | `tools/notebook.rs:74-100`; `notebook.rs` (13), golden `notebook_edit.txt` | — |

### M1.5 Migration

| Requirement | Capability | Class | Evidence | Gap |
|---|---|---|---|---|
| Import command | compat-import | c | `cyber-cli/src/commands/import.rs:33,37-41`, `cyber-core/src/import/preview.rs`; `cli.rs::import_preview_is_read_only_and_preserves_diff_escaping` | No writes, `--yes`, confirmation, `--include sessions`, file diff or "Nothing to import" message |
| Claude Code sources | compat-import | c | `import/claude.rs:36`, `import/mcp.rs`; `canonical_permission_example_and_group_order`, `claude_precedence_is_preserved_in_the_shared_permission_engine` | Agents, skills, rules, hooks, `model` alias, `fallbackModel`, `apiKeyHelper`, `env` not mapped (`preview.rs:559-583`) |
| Codex sources | compat-import | c | `import/codex*.rs`; `codex_prefix_rules_group_by_strength_and_expand_position_alternatives`, `codex_toml_provider_and_model_are_native_catalog_shapes` | Profiles, `[permissions.*]`, `shell_environment_policy`, hooks/notify, agents, AGENTS.md/skills, requirements report, granular approvals |
| OpenCode sources | compat-import | c | `import/opencode*.rs`; `opencode_*` (4), `opencode_provider_import_reports_credentials_without_writing_or_loading_packages` | `.opencode` agents, modes and skills, `instructions`, the plugin migration-guide link |
| Read-time skill compatibility | compat-import | c | `cyber-core/src/skills.rs:191`, `skills/discovery.rs:124` | `.opencode/skills` not searched; precedence order wrong; single `compat` bool; diagnostic text. Test `skills_compat.rs::duplicate_skill_precedence_follows_cyber_agents_claude_opencode_codex` |
| Read-time MCP compatibility | compat-import | d | No runtime `.mcp.json` loading | Whole contract |
| Mapping report | compat-import | c | `preview.rs:126,563,837`, `import.rs:76-100`; `import_merged_reports_include_comparisons_in_json_and_text_without_writing` | No `.cyber/import-report-<ts>.md` (no writes) |
| Secret safety during import | compat-import | c | `{env:NAME}` conversion; `provider_credentials_and_literal_headers_are_references_without_secret_values` | No keyring storage path |
| Source detection | compat-import | c | `import.rs:132`, `import/detection.rs`; `import_detection_is_read_only_and_outputs_counts_without_configuration_secrets` | Session counts and read-time coverage hard-coded "unknown" (`import.rs:154`); no TUI first-run offer |
| Idempotent re-import | compat-import | d | No `x-imported-from` | Whole contract |

### Other P1 (not owned by an M1.x milestone)

| Requirement | Capability | Class | Evidence | Gap |
|---|---|---|---|---|
| Reachable session listing | cross-session-messaging | d | Only the `messaging` config key (`cyber-core/src/config/validate.rs:37`) | Whole contract |
| Addressing | cross-session-messaging | d | — | Whole contract |
| Send message tool | cross-session-messaging | d | — | Whole contract |
| Message content limits | cross-session-messaging | d | — | Whole contract |
| Delivery semantics | cross-session-messaging | d | Inbox steer/hold exists (`runtime/continuation.rs:40`) | Session-origin admission |
| Inbound controls | cross-session-messaging | d | Hold and release routes exist (`http/sessions.rs:276,839`) | Policy, per-sender rules |
| Outcome reporting | cross-session-messaging | d | — | Whole contract |
| Replies | cross-session-messaging | d | — | Whole contract |
| Idle notifications | cross-session-messaging | d | — | Whole contract |
| Rate limits | cross-session-messaging | d | — | Whole contract |
| Permission boundaries stay per session | cross-session-messaging | d | — | Whole contract |
| Untrusted content | cross-session-messaging | d | — | Whole contract |
| Local transport | cross-session-messaging | d | — | Whole contract |
| Non-interactive sessions | cross-session-messaging | d | No `--accept-messages` | Whole contract |
| Transcript and audit | cross-session-messaging | d | — | Whole contract |
| Usage attribution | cross-session-messaging | d | No `origin` in the usage ledger | `origin: session/mcp`, `cyber stats --by origin` |
| Disable switch | cross-session-messaging | d | — | Whole contract |
| Messaging API | cross-session-messaging | d | No `messaging` route group, not even 503 (`http/mod.rs:506-522`) | Four routes, OpenAPI, SDK |
| Permission denial log | exec-mode | c | `exec/report.rs:161,268,467-478`; `exec_delegation.rs` (mock), `a_permission_denied_review_prints_the_review_object_without_a_job` | `action` field, `policy` reason, `--fail-on-deny` stop after the Turn. Add `exec_denials.rs::fail_on_deny_stops_after_the_sandbox_denied_turn` |
| Background work owned by the run | exec-mode | b | `exec/background.rs:90,159`, `exec/mod.rs:287-380`; `exec_forked_command.rs` (4), `exec_owned_jobs.rs` (4, mock), `exec_delegation.rs` SIGINT and `--max-tokens` (mock) | Real-server `exec_sigint_cancels_the_owned_forked_child_and_exits_130`, `exec_handback_grace_expiry_names_the_job`, `exec_reused_session_background_job_is_not_awaited` |
| Structured final output | exec-mode | d | No `--output-schema` (server mechanism exists for children) | Whole contract |
| CI usage | exec-mode | c | Env credentials (`cyber-llm/src/catalog/credentials.rs:66-71`), plain output | No `--color` flag; no headless env-only test. Add `cli.rs::exec_runs_headless_with_env_credentials_and_no_ansi` |
| Streaming input and partial output | exec-mode | d | — | Whole contract |
| Local web client | web-client | d | Router nests only `/api/v1` (`http/mod.rs:533`) | Whole contract |
| Usage commands | observability-costs | c | `cyber stats` (`commands/stats.rs:16-69`), TUI `/cost`, `/usage`, `/status`; `cli.rs::stats_render_the_usage_stats_route_for_days_projects_models_and_tools` | `--plugins`; `/usage` today/7/30 for the machine; `/status` sandbox and account (not sent by `/health`) |
| Context window meter | observability-costs | c | `runtime/context_meter.rs:9-40`; `http.rs::context_meter_reports_last_request_prompt_tokens_window_and_compaction_point` | Status-line data half (depends on Status line data contract) |
| Status line data contract | observability-costs | d | No `tui.statusline.command` runner | Whole contract |
| OpenTelemetry export | observability-costs | d | `telemetry` is only a config key | Whole contract |
| Prompt content privacy in telemetry | observability-costs | d | — | Whole contract |
| Debug traces | observability-costs | d | No `--trace`/`CYBER_TRACE` | Whole contract |
| Doctor health checks | observability-costs | c | `commands/doctor.rs:36-53,348`; `cli.rs::doctor_reports_every_check_as_json` | Provider reachability, formatters, MCP, plugins, disk, account, relay, `/doctor`, `ok` label, missing-bwrap scenario |
| Performance diagnostics | observability-costs | d | `DebugCmd` has no `perf` | Whole contract |
| Usage data retention | observability-costs | b | `cyber-app/src/retention.rs:44-105`, `usage_ledger.rs:330`; runtime `retention_folds_old_rows_into_daily_aggregates_without_losing_totals` | Add `cyber-app/tests/retention.rs::usage_older_than_retention_folds_and_stats_totals_remain` |
| Diagnostics bundle | observability-costs | d | — | Whole contract |
| Small model selection | provider-catalog | d | Explicit role only (`catalog/roles.rs:64-77`) | `--suggest-small`, ranking |
| Tool-call emulation for models without native tools | provider-catalog | d | — | Whole contract |
| Additional native provider adapters | provider-catalog | d | Adapters: anthropic, openai_chat, openai_responses, scripted | Gemini, Bedrock, Vertex, Azure (live baselines are external later) |
| Model fallback chain | provider-catalog | d | `fallback_models` is only a config key | Whole contract |
| Multiple connections per provider | provider-credentials | d | No credential store or `/credentials` route | Whole contract |
| Provider OAuth flows | provider-credentials | d | PKCE exists only for MCP | Whole contract |
| Command credentials | provider-credentials | d | — | Whole contract |
| Eager tool execution | session-runtime | c | Calls run after the stream (`runtime/drain.rs:727,744`); safe calls grouped (`:1902`) | Start allowed concurrency-safe calls during the stream. Test `concurrency_safe_reads_start_before_stream_closes` |
| Structured output | session-runtime | c | Child-only `output_schema` (`runtime/structured.rs:30-87`) | Per-prompt `format`, top-level Sessions (`runtime/mod.rs:845` refuses), tool name `structured_output`, 3 retries |
| Side chat | session-runtime | d | — | Whole contract |
| Instruction imports | system-context | b | `runtime/instructions.rs:460-494,553-557`; unit cycle and depth tests; app read-rule refusals | Add `cyber-app/tests/instructions.rs::instruction_imports_expand_once_and_cycles_and_missing_files_warn` |
| Nested rule files on read | system-context | a | `instructions.rs:243`; app `cyber_md_reaches_the_model_after_agents_md_nested_and_after_edits`, `nested_instruction_files_fire_instructions_loaded` | — |
| Skills, references, and MCP instruction sources | system-context | c | `host/skill_catalog.rs:9`, `mcp/pool.rs:976`; `skills_listing_respects_agent_profile_and_session_rules` | `core/references` |
| Context inspection | system-context | c | `http/sessions.rs:851` returns only epoch, baseline, summary, task | Per-source tokens, pending updates, utilization, TUI `/context` |
| Path-scoped rules and overrides | system-context | c | Override and truncation `instructions.rs:200-205,391-420`; unit tests | `.cyber/rules` and `.claude/rules` loading, `paths` attachment, configurable `instructions.max_bytes` |
| Config inspection by agents | configuration | c | Bundled `customize-cyber` skill (14 lines) | Layering, key catalogue and examples missing. Test `customize_cyber_skill_guides_an_mcp_edit_under_edit_permission` |
| References | configuration | d | Only a config key (`config/gate.rs:21`) | Whole contract |
| Shell integration on install | installation-upgrade | d | `completion` is reserved (`tree.rs:73`) | Whole contract |
| Compaction hooks | compaction | c | `runtime/lifecycle_events.rs:169,190`; `pre_compact_context_reaches_the_summary_instructions_without_a_message` | PreCompact `deny` is ignored (`lifecycle_hooks.rs:37`) |
| Tool output pruning | compaction | d | — | Whole contract |
| Native provider compaction | compaction | d | — | Whole contract (a live OpenAI Responses run later) |
| Non-git fallback | snapshots-checkpoints | d | `cyber-snapshot/src/lib.rs:47` returns `None` outside git | Whole contract |
| Rewind targets | snapshots-checkpoints | c | `POST /sessions/:id/rewind` (`http/sessions.rs:264,696`); `flow.rs::code_only_rewind_keeps_the_conversation` | `cyber sessions rewind` unavailable (`commands/sessions.rs:54`); no TUI; no HTTP route test |
| Undo and redo shortcuts | snapshots-checkpoints | d | Server stage/clear primitives only | TUI `/undo` and `/redo` |
| Retention and garbage collection | storage-events | c | `cyber-app/src/retention.rs:52,75`, snapshot gc `cyber-snapshot/src/lib.rs:80`; `sweeps_remove_only_expired_items` | Pinned-output exemption, finished Jobs, workflow transcripts, schedule test |
| Transcript persistence switch | storage-events | c | Ephemeral via `exec --ephemeral` or no `CYBER_DB` | `storage.persist_sessions`, `CYBER_EPHEMERAL`, durability in `/health`, `EphemeralModeError` gating |
| Rewind UI | tui | d | No `/rewind` | Whole contract |
| Status line | tui | c | `prefs.rs:376-405`, `view/footer.rs:70-143` | Default fields differ from spec (`app.rs:290`); goal, loops and remote empty; no command runner; no >85% test |
| Accessibility | tui | c | `prefs.rs:436-440`, `theme/mod.rs:436`; `screens_a11y.rs` (2), contrast test | Tool cards ignore screen-reader mode (`messages.rs:293-307`); no sentence streaming; no inline mode (`runner.rs:154`) |
| Reasoning display and effort command | tui | c | `prefs.rs:180`, Ctrl+T (`keys.rs:61`) | `hidden` not handled (`runner.rs:65`); no `/effort` or `max_variant` ceiling |

## Implementation increments

Each increment is one workflow increment, locally testable, and names its production-path tests. The order puts evidence-only work first (cheap b→a moves), then each milestone's missing contracts in dependency order, then the other P1 capabilities. Native Windows CI is a gate on every increment whose suites run on Windows.

Milestone sign-off needs these increments:

| Milestone | Increments |
|---|---|
| M1.1 | 6, 8 (tool catalog golden), 15 |
| M1.2 | 3, 5, 7, 8, 9 |
| M1.3 | 1, 4, 12, 13, 15 (Windows hook sandbox) |
| M1.4 | 2, 14 |
| M1.5 | 10, 11 |

### 1. Hooks acceptance and event completion (M1.3, 8 requirements)

- **Requirements:** Matchers, Command handlers, HTTP handlers, MCP tool handlers, Parallel execution within a group, Trust for project hooks, Decision schema, Supported events.
- **Work:**
  - Add `UserPromptSubmit` to `CONTINUE_EVENTS` and stop the Drain on `continue:false` (`lifecycle_hooks.rs:84`, `runtime/lifecycle.rs:144`).
  - Record input rewrites in the transcript.
  - Emit `Setup`, `CwdChanged`, `DirectoryAdded`, `ConfigChange`, `FileChanged` and `WorktreeRemove`.
- **Tests:**
  - `mcp_runtime.rs`: `regex_matcher_runs_for_mcp_tool_and_not_for_bash`, `post_tool_use_mcp_tool_records_payload_after_each_call`
  - `tool_hook_dispatch.rs`: `exit_two_pre_tool_command_blocks_with_stderr_reason`, `post_tool_command_exit_one_warns_with_hook_id_and_continues`, `http_fail_closed_timeout_blocks_call_with_hook_unavailable`, `http_non_2xx_is_nonblocking_on_a_real_call`, `rewrite_is_visible_as_hook_notice_in_transcript`
  - `exec_lifecycle_hooks.rs`: `exec_runs_a_command_defined_in_global_and_trusted_project_once`
  - TUI: `hook_notice_event_shows_untrusted_review_hint`
  - `lifecycle_hooks.rs`: `user_prompt_submit_continue_false_stops_after_the_turn`
  - `cyber-app/tests/server.rs`: `application_emits_setup_cwd_directory_and_config_change_hooks`
  - `worktree_setup.rs`: `worktree_remove_hook_observes_removal`
- **Status (2026-10-17): done, except the events without a producer.** Every named test exists and passes locally, with these deviations: the App test is `application_emits_setup_and_config_change_hooks` (no `CwdChanged` or `DirectoryAdded` producer exists), `worktree_remove_hook_observes_removal` lives in `isolated_subagents.rs` (it needs a completed isolated child and runs in the Windows job there), and `exec_applies_prompt_stop_and_records_input_rewrites_through_the_app_host` adds `cyber exec` coverage of the decision-schema work. Moved to (a): Matchers, Command handlers, HTTP handlers, MCP tool handlers, Parallel execution within a group, Trust for project hooks, Decision schema. *Supported events* stays (c): `DirectoryAdded` needs the P0 *Additional working directories* contract, `FileChanged` a file watcher and `CwdChanged` a working-directory change, none of which exists; `cyber --init`, isolated-child `WorktreeCreate` and CLI worktree removal are also open.

### 2. Memory and code-intelligence acceptance (M1.4, 12 requirements)

- **Requirements:** Memory file format, Automatic memory generation, Secret redaction, Staleness notice, Memory command, Explicit remember requests, Memory HTTP API, Memory toggles, LSP enablement, Read warms servers, LSP status and shutdown, Formatter execution.
- **Work:**
  - TUI `/memory on|off|readonly`, persisted and reconciled at the Safe Boundary.
  - Make the memory test fixture's put/delete scope-aware.
  - Decide whether `auto_install` gets an installer, or document that it installs nothing.
- **Tests:**
  - `cyber-cli/tests/memory.rs`: `debug_memory_lists_note_missing_type_as_invalid`, `project_path_prints_repository_memory_directory`
  - `cyber-tools/tests/memory.rs`: `api_key_reference_memory_is_rejected_with_canonical_message`
  - `memory_context.rs`: `generation_guidance_excludes_derivable_and_conversation_only_facts`, `scripted_forget_request_deletes_note_and_index_line`
  - `cyber-app/tests/memory.rs`: `project_put_updates_note_and_index_and_publishes_memory_updated`
  - TUI: `memory_readonly_off_on_commands_change_tool_and_context`
  - `edit_feedback.rs`: `autodetected_builtin_rust_analyzer_activates_for_rs_files`, `read_opens_document_in_background_without_waiting_for_diagnostics`
  - `cli.rs`: `doctor_lists_missing_gopls_as_not_installed_without_download`
  - `cyber-app/tests/intelligence.rs`: `lsp_status_reports_connected_root_and_close_exits_the_process`
  - `formatter_execution.rs`: `formatter_failure_is_logged_with_id_and_exit_code`

- **Status (2026-10-17): done.** Every named test exists and passes locally. Moved to (a): all twelve rows. Work as built:
  - `/memory on|off|readonly` in the TUI calls the new `PUT /api/v1/memory/settings`, which writes `memory.enabled`/`memory.generate` into the user's global `cyber.jsonc` (or `cyber.json`) with `cyber_core::config::set_global_key`. Only the `memory` member is replaced; comments and every other byte are kept, the edited document is re-parsed and compared before an atomic replace, and a document that is not an object is refused (409) and left untouched. Config is read per call, so each Session gets the new state at its next Safe Boundary (tool definitions and `core/memory` context), and HTTP/CLI/tool mutations recheck it. A project layer, `--config` override or `CYBER_DISABLE_MEMORY` remains a ceiling: the response reports the effective state with `overridden: true` and the TUI says so. `GET /api/v1/memory/settings` reads the effective state.
  - The app memory fixture's `put`/`delete` take a scope (`put_in`, `delete_in`).
  - `lsp.auto_install` stays a validated no-op: nothing downloads servers in P1, and `cyber doctor` reports a missing server as `not installed; auto_install does not download servers` when it is set.
  - Formatter failures now log `exit_code` and a `formatter exited with code N` reason instead of `Some(N)`.
  - Extra tests: app `memory_settings_persist_globally_and_report_project_overrides`, core `config::edit` unit tests. The `edit_feedback.rs`, `formatter_execution.rs` and LSP lifecycle tests are Unix-only (Python fixture servers); the rest run in the Windows job's existing memory/TUI steps.
  - After a Location close, `GET /lsp` keeps listing the settled generation as `broken` until reload (existing pool behavior); the new test asserts that.

### 3. Agent and worktree evidence, agent catalogue (M1.2, 5 requirements)

- **Requirements:** Session worktrees, Worktree location and branch naming, Manual invocation by mention, Concurrency cap, Agent tool catalogue.
- **Work:**
  - Build the per-Turn `agent` description in `BuiltinHost::definitions`.
  - Report `queued` progress while waiting for a subagent slot.
- **Tests:**
  - `cli.rs`: extend `exec_worktree_starts_in_owned_checkout_and_keeps_json_stdout_clean`; add `exec_worktree_without_name_generates_adjective_noun_hex`
  - `cyber-app/tests/server.rs`: `public_worktree_creation_honours_default_and_custom_root_prefix_and_base`
  - `exec_delegation.rs`: `exec_agent_mention_spawns_explore_child_on_embedded_server`
  - `subagents.rs`: `agent_tool_description_lists_permitted_subagents_per_turn`, `queued_foreground_spawn_reports_queued_progress`

- **Status (2026-10-17): done.** Every named test exists and passes locally. Moved to (a): all five rows. Work as built:
  - `BuiltinHost::definitions` regenerates the `agent` description each Turn (`with_agent_catalogue`): the fixed text, then `Available agents:` and one `- <name>: <description>` line per visible `subagent`/`all` profile whose `agent` rule is not `deny` for the caller, sorted by name, with whitespace in descriptions collapsed (`No subagents are available.` when none is). Configuration that does not resolve withholds the `agent` tool (fail closed). The runtime still rechecks every spawn, so the list only narrows what is offered.
  - A spawn that cannot take an `agents.max_concurrent` slot at once publishes the live, never durable `LiveEvent::ToolProgress` (`session.tool.progress`, `{session_id, call_id, status}`) with `queued`, then `running` once admitted; it is forwarded on the Session and Location event streams.
  - Extra production-path test: App over HTTP `server.rs::agent_catalogue_and_queued_progress_reach_http_clients` (provider request carries the catalogue with `general` denied; the Session SSE stream carries `queued` then `running` for two parallel `explore` calls under `max_concurrent: 1`).
  - Windows: `exec_worktree_without_name_generates_adjective_noun_hex`, the two new `server.rs` tests, `exec_agent_mention_spawns_explore_child_on_embedded_server` and the `subagents.rs` tests are portable and run in the existing Windows steps (`cyber-cli --test cli`, `cyber-app --test server`, `cyber-cli --test exec_delegation`, `cyber-tools --test subagents`). The extended `exec_worktree_starts_in_owned_checkout_and_keeps_json_stdout_clean` stays Unix-only (its setup command is POSIX shell).

### 4. M1.3 evidence, MCP policy, annotations, SDK hooks, VCS text (M1.3, 9 requirements)

- **Requirements:** Shell output injection, Eager execution during streaming, Roots and sampling, Local review command, Annotations drive modes and hooks, Organization MCP controls, Hook callbacks (client-sdk), Commit and PR text generation, PR checkout and linked sessions.
- **Work:**
  - `policy.mcp` allow/deny and managed servers in `config/managed.rs`.
  - `destructiveHint` forces the classifier; `openWorldHint` raises a network ask when the sandbox network is off; annotations go into hook envelopes.
  - `hook/execute` forwarding, a `client` scope and SDK `client.hooks.on`.
  - `cyber git commit-msg`, `/commit`, `/pr-description`, `git.co_author`; PR link recording, `cyber pr <n>`, `--from-pr`.
- **Tests:**
  - `cyber-app/tests/server.rs`: `forked_skill_shell_injection_expands_once_before_the_child`
  - `cyber-server/tests/runtime.rs`: `complete_tool_call_is_durable_before_the_stream_finishes`
  - `cyber-app/tests/mcp.rs`: `remote_sampling_routes_to_small_model_through_app_host`, `remote_sampling_disabled_returns_method_not_found`, `denylisted_url_pattern_is_never_contacted`, `managed_policy_server_is_provided_to_every_location`
  - TUI against a real App: `review_against_app_server_renders_findings_and_ignores_unrelated_jobs`
  - `mcp_runtime.rs`: `open_world_tool_asks_network_when_sandbox_network_off`, `destructive_tool_is_always_classified_in_auto_mode`
  - SDK: `sdk/typescript/test/hooks.test.ts` `pre_tool_use_client_handler_allows_docs_edits_and_records_client_scope`
  - `cyber-server/tests/http.rs`: `client_hook_registration_drops_on_disconnect`
  - `cyber-cli/tests/git_commit_msg.rs`: `commit_msg_has_no_co_author_by_default`
  - `cyber-cli/tests/pr_command.rs`: `pr_checkout_resumes_the_linked_session` (fake `gh`)
- **Status (2026-10-17): done.** Every named test exists and passes locally. Moved to (a): all nine rows. Work as built:
  - **Org MCP controls.** The managed policy document takes `policy.mcp.allow`/`deny` arrays of `{name|command|url}` wildcard patterns and a top-level `mcp` map of managed servers. A managed server replaces a same-named user/project definition whole (`config/load.rs` `replace_server`), needs no checkout approval and is exempt from the lists. `authorize_server` refuses a blocked server before every launch or connection, and `GET /mcp` reports it `failed` / `blocked by organization policy`; `cyber mcp get|definitions` show `blocked` and `managed`. A malformed document blocks every non-managed server (fail closed). The policy is not part of the OpenAPI `ManagedPolicy` schema.
  - **Annotations.** MCP hints take their protocol defaults (not read-only means destructive unless `destructiveHint: false`; open-world unless `openWorldHint: false`); application tools declare none. In `auto` mode a destructive tool reaches the classifier even under an allow rule, saved approval or hook allow, and a classifier fallback asks. With an enforcing sandbox whose `network` is `off`, an open-world tool needs `network` on `mcp:<server>` before it runs. Hook envelopes of registered calls carry `tool_annotations`.
  - **Client hooks.** `client.hooks.on(event, handler, {matcher, timeout})` registers over the SDK's existing JSON-RPC channel (`v1.hook.register` → `hkr_` ID, `v1.hook.unregister`), is restored after a reconnect and applies only to the client's Sessions and their descendants (created or forked by the client, or attached with `v1.hook.attach`; `ClientHooks::client_hooks` filters by the Session's lineage). The tool host appends `client`-scope definitions after every configured one (`HookCatalog::with_client`; a managed-only policy skips them), and `hook_client.rs` admits a durable receipt, forwards `hook/execute {registration_id, event, input}` and settles it. Failures, invalid answers and timeouts never block. `type: client` is refused in configuration. `App::build` attaches `RemoteTools` as the `ClientHooks` source; no new `ToolHost` method.
  - **Commit/PR text.** `cyber-app/src/git_text.rs` runs `git` for the staged diff or `base..HEAD`, asks `model_roles.small` (else `small_model`, else the default model) without tools, strips model-written `Co-authored-by`/`Signed-off-by` lines and appends `git.co_author` only when set (validated `Name <email>`). Exposed as `cyber git commit-msg`, `cyber git pr-description [--base]`, `POST /api/v1/git/commit-message|pr-description`, and TUI `/commit` and `/pr-description [base]`, which prefill `!git commit -m …` and `!gh pr create …` for the user to submit.
  - **PR links.** A `bash` call whose simple command starts `gh pr create` or `git push` records `session.pr.linked.1 {number, url}` once per PR, from a `…/pull/<n>` URL in its output or else `gh pr view --json number,url` run under the same sandbox. `GET /sessions?pr=<n>` filters on it. `cyber pr <n|url>` runs `gh pr checkout` (gh adds the fork remote) and resumes the most recent linked Session or starts a new one; `cyber --from-pr <n|url>` opens the picker over the linked Sessions (`Start::PickPr`). `--format json` prints the decision instead of opening the TUI.
  - Deviations from the named tests: the org-policy app tests run their body in a child process of the test binary (`run_with_managed_policy`), because the App reads `CYBER_MANAGED_POLICY_DIR` from the process environment and edition 2024 makes `set_var` unsafe; `pr_command.rs` is Unix-only (its fake `gh` is a shell script). Extra tests: CLI `pr_description_drafts_title_and_body_from_the_branch`, app `client_pre_tool_use_handler_approves_docs_edits_through_the_app`, TUI `commit_and_pr_description_prefill_commands_from_the_app_server`, unit tests for `git.co_author`, PR URL/command recognition and PR argument parsing.
  - Windows: the new `cyber-app --test mcp`, `cyber-tui --lib`, `cyber-cli --test git_commit_msg` and server/app tests are portable; ci.yml runs them (`mcp` and `cyber-tui --lib` already ran; new steps added for the rest). `mcp_runtime.rs` (annotation tests) is Unix-only like the rest of that file.

### 5. Worktree lifecycle (M1.2, 5 requirements)

- **Requirements:** Worktree management commands, Concurrency safety, Cleanup on exit, Worktree events, Enter and exit tools.
- **Work:**
  - `cyber worktree remove|prune` and `DELETE /api/v1/worktrees/{name}`.
  - Top-level Session cleanup that reuses the `child.rs` report.
  - `worktree.created/removed/kept.1` events.
  - `enter_worktree`/`exit_worktree`, gated by the `worktree` permission.
- **Tests:**
  - `cli.rs`: `worktree_remove_refuses_dirty_without_force`, `worktree_remove_while_session_active_fails_in_use`, `worktree_prune_removes_only_older_clean_worktrees`, `exec_worktree_without_changes_is_removed_on_exit`
  - `cyber-app/tests/server.rs`: `worktree_lifecycle_publishes_created_removed_and_kept_events`
  - `worktree_setup.rs`: `model_enter_worktree_resolves_later_paths_in_spike_and_exit_returns`, `exit_worktree_is_not_offered_to_isolated_children`
- **Status (2026-10-18): done locally.** All five rows are (a).
  - `cyber worktree remove <name> [--force]` and `prune [--older-than <days>]` (default 14) call the new `DELETE /api/v1/worktrees/{name}`. Removal goes through the existing journaled `Repository::remove` with `CheckoutActivity`, so a running Session's lease refuses it with `Worktree in use by ses_<id>` even with `--force`. Prune removes only ready, clean entries not ahead whose ownership record is older than the cutoff (`created_ms` in the listing); removal re-checks cleanliness.
  - Top-level exit cleanup reuses the child report (`ChildWorktree::top_level`) behind `POST /sessions/{id}/worktree/cleanup`. `cyber exec --worktree` and `cyber --worktree` call it on exit and print `worktree kept at <path> (branch <branch>): <reason>` when the checkout stays. `worktrees.cleanup: ask` keeps the checkout there, because no client is left to confirm.
  - `worktree.created.1`, `worktree.removed.1` and `worktree.kept.1` are live events `{ name, path, branch, session_id? }` on the requesting Location's stream. Removal runs `WorktreeRemove` as a Session whose Location outlives the checkout (one that entered it and was returned by `--force`, or an isolated child's parent); a checkout only its started Session owned runs none.
  - `enter_worktree { name?, path? }` switches into an existing managed worktree or creates one (permission, `WorktreeCreate` hooks and setup before the move). The Session commits `session.worktree.entered.1`, and the Drain's Location lease is replaced by one on the worktree, so later calls resolve there and removal stays fenced. `exit_worktree { keep? }` commits `session.worktree.exited.1`, returns to the origin and, unless `keep`, removes a clean checkout. `enter_worktree` is offered only outside a managed checkout and `exit_worktree` only after entering; Sessions started isolated get neither, and calls that arrive anyway are refused before any discovery, creation, prompt, hook or setup (`Runtime::check_enterable`; app `entered_worktree_refused_for_a_started_session_before_any_creation`).
  - Review fixes (2026-10-18): another Session's exit cleanup keeps a checkout an idle Session entered (`worktree entered by ses_…`); a Session that ends entered gets exit cleanup from `cyber exec`, the TUI and `POST /sessions/{id}/worktree/cleanup` (`Runtime::exit_idle_worktree` commits `exited.1` without a Turn, also when the checkout is already gone); `--force` removal returns idle entered Sessions first. Evidence: app `entered_worktree_survives_the_owners_cleanup_and_recovers_after_removal`, `entered_worktree_user_removal_runs_worktree_remove_and_force_returns_sessions`, CLI `exec_entered_worktree.rs::exec_that_ends_in_an_entered_worktree_cleans_it_up`.
  - Windows CI: `cli.rs` and `worktree_setup.rs` already run in full; a new step runs the app test.

### 6. Sandbox policy, escalation and mode gates (M1.1, 13 requirements)

- **Requirements:** Sandbox CLI, Named sandbox profiles, Environment policy, Escalation requests, Excluded commands, Sandbox events and audit, Container detection, bypass mode, Rule dry run, Critical-path removal guard, Permission mode indicator and cycling, accept-edits mode, dont-ask mode.
- **Work:**
  - Profiles with `extends` and `deny_read` enforced by the OS and in read/glob/grep; the full env policy.
  - `cyber sandbox test|explain|run`.
  - `Sandbox denied:` classification, the escalate parameter and the `sandbox.escalate` request.
  - `excluded_commands` with an ask floor; `sandbox.*` events.
  - Container probe and the bypass eligibility gate, including `--dangerously-bypass-permissions` and fixing `exec --yolo`.
  - `cyber permissions test` and `GET /api/v1/permissions/test`.
  - Org-policy `modes.disable`; the remaining shell-context removal cases.
- **Tests:**
  - `cli.rs`: `sandbox_explain_prints_roots_network_domains_and_masked_names`, `sandbox_explain_profile_resolves_extends_and_unions_deny_read`, `sandbox_run_executes_under_effective_policy`, `permissions_test_lists_saved_approval_and_default_rule_per_command`
  - `tools.rs`: `profile_deny_read_blocks_cat_and_hides_glob_match`, `sandbox_env_none_exposes_only_base_vars_set_and_markers`, `sandbox_write_denial_is_explicit_and_recorded_as_event`, `excluded_command_runs_unsandboxed_and_asks_even_with_allow_rule`, `bypass_refuses_rm_rf_of_git_toplevel_substitution_without_execution`, `accept_edits_allows_write_but_keeps_non_edit_and_recursive_rm_asks`
  - `flow.rs`: `escalation_prompt_shows_command_and_justification_and_runs_once_unsandboxed`, `escalation_denied_in_dont_ask`
  - `cyber-app/tests/server.rs`: `bypass_refused_on_bare_host_with_full_access`, `container_marker_allows_bypass_and_header_shows_isolation_container`, `mode_switch_to_plan_is_durable_and_next_turn_denies_writes`
  - `http.rs`: `permissions_test_route_matches_cli_evaluation_without_session`
  - `exec_lifecycle_hooks.rs`: `exec_without_tty_denies_unapproved_curl_and_continues`
  - TUI: `mode_bypass_disabled_by_org_policy_keeps_mode`

This increment also carries the ROADMAP P1 exit criterion: with the sandbox enabled, a `bypass` session cannot write outside the writable roots or reach a non-allowlisted host. Add `cyber-app/tests/sandbox_escape.rs` as the escape suite on macOS and Linux.

### 7. Process Jobs foundation (M1.2, 8 requirements)

- **Requirements:** Background bash, Job registry, Completion notices, Reading job output, Stop kills process trees, Concurrency limits, Jobs end with their session, Background job events.
- **Work:** a generic process Job owner in `runtime/jobs.rs`:
  - kind and command fields, exit code;
  - a log file with the `background.max_output_mb` cap;
  - process-group or Job Object kill with 5 s escalation;
  - `background.max_jobs`, `notify:false`, live `job.output.1`;
  - `bash`/`powershell` `background:true`.
- **Tests:**
  - New `cyber-tools/tests/background_bash.rs`: `background_bash_returns_job_id_and_log_path_and_survives_turn_end`, `background_bash_notice_has_exit_code_duration_and_last_20_lines`, `notify_false_suppresses_the_completion_notice`, `read_pages_a_running_job_log_without_stopping_it`, `job_log_cap_drops_oldest_bytes_with_marker`, `task_stop_terminates_grandchildren_and_records_cancelled`, `twenty_first_job_fails_with_limit_message`, `interrupt_keeps_background_dev_server_running`, `deleting_session_cancels_bash_jobs`
  - `cyber-server/tests/jobs.rs`: `restart_marks_two_running_bash_jobs_interrupted`
  - `lifecycle_hooks.rs`: `job_ended_hook_receives_nonzero_exit_code`

### 8. Monitor and task controls (M1.2 and M1.1 catalog, 6 requirements)

- **Requirements:** Monitor tool (background-tasks), monitor tool (builtin-tools), Tasks view, Background tasks view (tui), Background commands from the user, Capability-owned tool catalog.
- **Work:**
  - `monitor` with command, url and path sources, filter, rate limit, `until` and timeout.
  - `/tasks` lists runs and monitors, binds `x` and opens logs.
  - The `&` prefix and Ctrl+B handoff.
  - A registry-subset golden, plus a spec clarification for phase-gated tools in the catalog.
- **Tests:**
  - `cyber-tools/tests/monitor.rs`: `monitor_filters_and_rate_limits_to_30_per_minute`, `monitor_until_match_ends_with_final_notice`, `monitor_lines_are_queued_with_source_monitor`
  - `cyber-app/tests/server.rs`: `monitor_tail_filter_admits_error_lines_rate_limited`
  - TUI against a real App: `tasks_stop_with_x_marks_job_cancelled_and_notifies_model`, `ampersand_shell_command_starts_background_job_without_model`, `ctrl_b_moves_foreground_bash_to_background_job`
  - `golden.rs`: `advertised_tool_names_are_a_subset_of_the_canonical_registry`

### 9. PTY, notify, send file and side chat (M1.2 and other, 6 requirements)

- **Requirements:** PTY sessions, Model interaction with PTYs, Notify tool, Send file to user, Side chat (session-runtime), Subagent threads and side chat (tui).
- **Tests:**
  - `cyber-server/tests/pty.rs`: `reattach_without_cursor_replays_buffer_then_live`, `ticket_is_single_use_and_expires_in_60s`
  - `cyber-tools/tests/pty_tools.rs`: `pty_drives_python_repl_print_2_plus_2`
  - `cyber-tools/tests/notify.rs`: `notify_rate_limits_to_10_per_hour_and_truncates_fields`, `send_file_caps_25mb_and_10_files_and_records_part`
  - `cyber-app`: `side_chat_answers_with_read_only_tools_and_leaves_history_unchanged`
  - TUI: `btw_overlay_discards_on_close_and_insert_answer_inserts`

### 10. Import write path (M1.5, 5 requirements)

- **Requirements:** Import command, Mapping report, Secret safety during import, Idempotent re-import, Source detection.
- **Work:**
  - Unified file diffs of `cyber.jsonc` and `.cyber/`, confirmation and `--yes`, writes.
  - `.cyber/import-report-<ts>.md`, `x-imported-from` annotations and "nothing to do".
  - A keyring credential path, behind a mock store locally.
  - Session counts and read-time coverage; the TUI first-run offer.
- **Tests:**
  - `cli.rs`: `import_claude_dry_run_prints_a_file_diff_and_writes_nothing`, `import_with_no_sources_prints_none_found_and_exits_zero`, `import_codex_yes_twice_writes_once_then_reports_nothing_to_do`, `import_writes_report_file_listing_merged_model_with_both_values`, `import_literal_codex_api_key_never_reaches_cyber_jsonc`, `import_detect_reports_read_time_coverage_per_item`
  - TUI: `first_run_offers_import_auto_dry_run_once`

### 11. Source adapters and read-time compatibility (M1.5, 5 requirements)

- **Requirements:** Claude Code sources, Codex sources, OpenCode sources, Read-time skill compatibility, Read-time MCP compatibility.
- **Work:**
  - Claude agents, skills, rules, hooks (event map plus unmappable report), model aliases, `fallbackModel`, `apiKeyHelper`, `env`.
  - Codex profiles, `[permissions.*]`, `shell_environment_policy`, hooks/notify, granular approvals.
  - OpenCode agents, modes, skills, `instructions`, the plugin-guide link.
  - Fix the skill root precedence; add `.opencode/skills` and per-tool `compat.<tool>`.
  - Runtime `.mcp.json` below `cyber.jsonc`, with per-project approval.
- **Tests:**
  - `import_preview.rs`: `claude_hooks_map_events_and_list_unmappable_under_not_imported`, `codex_profiles_and_notify_become_profiles_and_notification_hook`
  - `skills_compat.rs`: `duplicate_skill_precedence_follows_cyber_agents_claude_opencode_codex`
  - `cyber-app/tests/server.rs`: `project_mcp_json_server_requires_one_time_approval`
  - The M1.5 exit test `cli.rs::import_claude_reproduces_reference_setup_without_manual_edits`, over a checked-in reference `.claude/` fixture with agents, skills, MCP and hooks.

### 12. Plugin foundation (M1.3, 8 requirements)

- **Requirements:** Plugin manifest, Component discovery defaults, Install scopes, Plugin sources, Plugin CLI, Pure mode, Trust and signing (P1 part), Hook scopes and merge order (plugin scope).
- **Work:** declarative plugins only: skills, commands, agents, hooks and `.mcp.json` under the plugin scope; path and git sources; the lockfile.
- **Tests:**
  - `cyber-cli/tests/plugins_cli.rs`: `install_path_plugin_registers_hooks_and_namespaces_colliding_command`, `invalid_manifest_name_fails_naming_the_field`, `engine_mismatch_refuses_install_without_force`, `git_install_writes_lockfile_commit_and_digest`, `project_plugin_requires_trust_before_enable`, `pure_flag_disables_plugins_project_hooks_and_project_mcp`
  - `exec_lifecycle_hooks.rs`: `exec_runs_plugin_hooks_last_and_skips_them_under_managed_only`

### 13. Plugin host runtime (M1.3, 8 requirements)

- **Requirements:** Plugin host process, Host protocol, Restart and backoff, Scoped registrations, Capability declaration and enforcement, User configuration, TypeScript plugin kit, Deferred tools and tool search (plugin tail).
- **Tests:**
  - `cyber-app/tests/plugins.rs`: `plugin_tool_is_forwarded_via_tool_execute`, `plugin_crash_fails_the_call_and_session_continues`, `crash_loop_disables_plugin_and_removes_tools_next_turn`, `disable_removes_only_that_plugins_registrations`, `undeclared_network_is_a_sandbox_denial`, `protocol_version_mismatch_reports_both_versions`, `plugin_tools_are_deferred_above_threshold`
  - SDK plugin-kit harness: `define_plugin_tool_round_trip`

### 14. Browser verification (M1.4, 2 requirements)

This increment is under the size target because the work is self-contained.

- **Requirements:** Supported optional browser integration, Verification artifacts and lifecycle.
- **Work:**
  - A pinned browser MCP definition and `cyber browser install|health|cleanup`.
  - Per-Session profiles; trust, permission and network gating.
  - The evidence record with hashes and staleness.
  - An owned dev-server Job; depends on increment 7.
- **Tests:** `cyber-app/tests/browser.rs` against a fake browser MCP server: `dev_server_exiting_before_readiness_records_startup_failure_with_logs`, `session_close_stops_owned_dev_server_and_browser`, `evidence_records_hashes_and_marks_stale_after_edit`, `untrusted_project_browser_server_is_not_installed`.

### 15. Windows enforcement (M1.1 and M1.3, 3 requirements)

- **Requirements:** Windows enforcement, powershell tool, Sandboxing of command hooks.
- **Work:**
  - Route production dispatch through AppContainer with recursive roots, exclusions and the Windows proxy.
  - Finish the PowerShell analysis (encoding, abbreviations, mutation facts).
  - An enforcing hooks profile on Windows.
- **Tests:**
  - Local: `exec_lifecycle_hooks.rs::exec_project_post_tool_hook_cannot_write_home` (Unix).
  - Proof needs native Windows CI: `windows_powershell.rs::native_sandboxed_powershell_write_outside_roots_is_denied`, plus a Windows counterpart in `exec_hooks_windows`. Locally it can only be type-checked (`clippy --target x86_64-pc-windows-gnu`).

### 16. Exec and session-runtime contracts (other, 7 requirements)

- **Requirements:** Permission denial log, Structured final output, CI usage, Streaming input and partial output, Background work owned by the run, Structured output (session-runtime), Eager tool execution.
- **Tests:**
  - `exec_denials.rs`: `fail_on_deny_stops_after_the_sandbox_denied_turn`
  - `exec_structured.rs`: `output_schema_returns_validated_object`, `output_schema_two_failures_exit_1_structured_output_error`
  - `cli.rs`: `exec_runs_headless_with_env_credentials_and_no_ansi`
  - `exec_stream_input.rs`: `two_user_lines_then_end_emit_two_results_exit_0`, `permission_reply_once_runs_bash_without_denial`
  - `exec_forked_command.rs`: `exec_sigint_cancels_the_owned_forked_child_and_exits_130`, `exec_handback_grace_expiry_names_the_job`, `exec_reused_session_background_job_is_not_awaited`
  - `cyber-server/tests`: `concurrency_safe_reads_start_before_stream_closes`
  - `cyber-app`: `prompt_format_json_schema_stores_validated_summary_and_ends_drain`

### 17. Context sources and configuration (other, 6 requirements)

- **Requirements:** Path-scoped rules and overrides, Context inspection, Skills/references/MCP instruction sources, Instruction imports, Config inspection by agents, References.
- **Tests:**
  - `cyber-app/tests/instructions.rs`: `path_scoped_rule_attaches_once_on_matching_edit`, `agents_override_replaces_directory_files_in_the_request`, `instruction_imports_expand_once_and_cycles_and_missing_files_warn`
  - `cyber-app/tests/server.rs`: `context_route_lists_sources_with_token_estimates_largest_first`, `local_reference_is_listed_and_readable_without_external_prompt`, `customize_cyber_skill_guides_an_mcp_edit_under_edit_permission`

### 18. Compaction, rewind and snapshots (other, 7 requirements)

- **Requirements:** Compaction hooks, Tool output pruning, Native provider compaction, Rewind targets, Undo and redo shortcuts, Non-git fallback, Rewind UI.
- **Tests:**
  - `lifecycle_hooks.rs`: `pre_compact_deny_blocks_automatic_compaction_and_reports_on_manual`
  - `context_compaction.rs`: `old_tool_outputs_are_pruned_to_forty_thousand_tokens`, `native_type_without_capability_fails_with_native_compaction_unavailable`
  - `sessions_cli.rs`: `sessions_rewind_code_target_restores_files_only`
  - `cyber-app/tests/server.rs`: `rewind_route_applies_code_conversation_and_both_targets`
  - TUI: `code_only_rewind_restores_files_and_marks_history`, `undo_restores_previous_prompt_and_redo_clears`
  - `cyber-snapshot/tests`: `non_git_location_edits_are_rewindable_via_file_copy_store`

### 19. Status line, usage and TUI display (other, 7 requirements)

- **Requirements:** Status line data contract, Status line (tui), Context window meter, Usage commands, Usage data retention, Accessibility, Reasoning display and effort command.
- **Tests:**
  - TUI: `status_command_receives_session_json_and_renders_first_line`, `context_field_over_85_percent_uses_the_warning_color`, `status_tab_shows_sandbox_and_account`, `usage_command_shows_today_week_month_for_the_machine`, `screen_reader_tool_failure_is_a_text_line`, `effort_xhigh_switches_variant_and_footer`, `effort_above_org_ceiling_is_refused`
  - `cli.rs`: `stats_plugins_lists_plugin_usage`
  - `cyber-app/tests/retention.rs`: `usage_older_than_retention_folds_and_stats_totals_remain`

### 20. Diagnostics, telemetry and storage (other, 9 requirements)

- **Requirements:** Doctor health checks, Debug traces, Performance diagnostics, Diagnostics bundle, OpenTelemetry export, Prompt content privacy in telemetry, Retention and garbage collection, Transcript persistence switch, Shell integration on install.
- **Tests:**
  - `cli.rs`: `doctor_sandbox_fails_with_install_hint_without_bwrap`, `trace_flag_writes_redacted_jsonl_and_prints_path`, `debug_perf_prints_startup_phase_table`, `debug_bundle_zip_has_redacted_config_and_no_keyring_values`, `cyber_ephemeral_reports_ephemeral_durability_and_refuses_remote`, `declined_completion_offer_is_recorded_and_not_shown_again`
  - `cyber-app/tests/otel.rs` (local OTLP collector stub): `workflow_run_exports_root_and_child_spans`, `default_tool_spans_omit_input_and_output`
  - `retention.rs`: `sweeps_keep_pinned_output_and_remove_old_jobs_and_workflow_transcripts`

### 21. Messaging core (other, 10 requirements)

- **Requirements:** Reachable session listing, Addressing, Send message tool, Message content limits, Delivery semantics, Local transport, Untrusted content, Replies, Permission boundaries stay per session, Disable switch.
- **Tests:** `cyber-app/tests/messaging.rs`: `local_sessions_list_each_other_as_local`, `ambiguous_name_fails_listing_both_ids`, `oversized_message_refused_before_transmit`, `message_to_mid_tool_session_waits_for_safe_boundary`, `message_to_idle_session_starts_a_turn`, `message_text_is_delimited_and_cannot_change_mode`, `send_message_ask_prompts_before_leaving`, `disabled_messaging_hides_tools_and_refuses_inbound`.

### 22. Messaging policy, API and idle notifications (other, 8 requirements)

- **Requirements:** Inbound controls, Outcome reporting, Rate limits, Non-interactive sessions, Transcript and audit, Messaging API, Usage attribution, Idle notifications.
- **Note:** this increment also unblocks the deferred messaging tools of `add-mcp-session-server`.
- **Tests:**
  - `cyber-server/tests/messaging_api.rs`: `refused_sender_stores_nothing`, `held_message_release_emits_released_to_sender`, `eleventh_message_in_a_minute_is_rate_limited`, `sdk_send_uses_origin_client`
  - `exec_messaging.rs`: `exec_without_accept_messages_refuses_non_interactive`
  - `cli.rs`: `stats_by_origin_separates_message_usage`
  - `messaging.rs`: `watch_notifies_once_when_target_goes_idle`, `subscription_expires_after_12h`

### 23. Providers (other, 7 requirements)

- **Requirements:** Multiple connections per provider, Provider OAuth flows, Command credentials, Small model selection, Tool-call emulation, Additional native provider adapters, Model fallback chain.
- **Tests:**
  - `providers_cli.rs`: `use_switches_active_connection_for_requests`, `api_key_command_output_is_used_and_rerun_after_refresh`, `login_method_code_prints_url_and_reads_pasted_code` (stub authorization server)
  - `http.rs`: `fallback_resends_turn_on_next_model_after_529s`, `emulated_tool_block_runs_read_and_returns_user_role_result`
  - `cli.rs`: `models_suggest_small_picks_mini_without_changing_roles`
  - `cyber-llm/tests/adapters.rs`: contract fixtures per new adapter

### 24. Local web client (other, 1 requirement)

- **Requirement:** Local web client.
- **Work:** the bundled frontend, the `/` and `/app/*` routes, `cyber web` and the TLS guard.
- **Tests:** `cli.rs::web_non_loopback_without_tls_exits_2`, `server.rs::root_serves_bundled_index_and_login`. Parity with the TUI needs Playwright evidence.

## External evidence (class e)

| Requirement | Evidence needed to sign off |
|---|---|
| permissions-modes: auto mode classifier | A live `model_roles.evaluator` (or `small_model`) run over a fixed, versioned suite. Each of these must get `block` with a reason and a `permission.auto_decided.1` event: force push, `kubectl apply`/deploy, `rm` outside the Location, a `~/.aws` read, curl to a non-allowlisted host. Matched benign controls must get `allow`. Record the model, version and trial count. Native Windows CI of `auto_dispatch.rs` and `cyber-server/tests/auto.rs` is also needed. |
| mcp: OAuth | A native macOS Keychain round trip and a Windows Credential Manager round trip of an MCP credential through `oauth::open_store`: `cyber mcp auth`, connect, refresh, `cyber mcp logout`, on a real OS keyring with a real prompt if one appears. A green native run of the Windows CI step "Native MCP OAuth credential store" is also needed. Today every keyring test uses an in-memory vault. |

Other rows will need external evidence after their local work is done. They are not (e) today because local gaps remain:

- Windows enforcement, the powershell tool and Windows hook sandboxing: native Windows CI.
- Additional native provider adapters: live coding baselines with real Gemini, Bedrock, Vertex and Azure accounts.
- Provider OAuth: a real browser flow against the provider's authorization server.
- Secret safety during import: a real OS keyring.
- Browser verification: one run against a real pinned browser on an approved local URL.
- Native provider compaction: a live OpenAI Responses run.
- Notify and Send file push to a real paired Device.

Every (a) row whose suite runs on Windows also needs a green native Windows CI run before the milestone is signed off.

Automatic memory generation, Staleness notice and Explicit remember requests are (a): their SHALL clauses are the instructions the system gives and the tool effects, and both are tested. An optional live-model eval of what a real model chooses to save is deferred; it is not acceptance evidence.

## Top gaps

1. **Cross-session messaging is absent** (18 rows, d). The ROADMAP lists it under P1 capabilities, and `add-mcp-session-server` defers its messaging tools on it.
2. **The plugin system is absent** (14 rows, d). It also blocks the hook plugin scope, plugin-tool deferral and plugin skills/commands.
3. **Background process Jobs are absent** (background-tasks: 9 d, 6 c). Only subagent Jobs exist, and `bash background:true` is refused, so monitor, PTY, `/tasks` and job events can't be met.
4. **Most of the sandbox policy surface has no code.** Escalation, excluded commands, container detection, the sandbox CLI, sandbox events and named profiles are missing; the env policy is a stub. The bypass eligibility gate is also missing (`exec --yolo` is unconditional). The P1 escape exit criterion depends on these.
5. **Windows enforcement is not wired.** Production dispatch returns `SandboxUnavailableError`, and project, local and `sandbox_all` hooks refuse on Windows.
6. **`cyber import` is dry-run only.** There are no writes, and the hook, agent, skill and rule adapters are missing. The M1.5 exit criterion is unmet.
7. ~~**The worktree lifecycle is incomplete.**~~ Closed locally by increment 5 (2026-10-18): enter/exit tools, `remove`/`prune`, `DELETE /worktrees`, top-level cleanup and `worktree.*` events.
8. **Exec contracts.** `--output-schema`, streaming JSONL input and `--color` are missing, and denial logging and `--fail-on-deny` are partial.
9. **Observability.** OpenTelemetry, debug traces, the status-line command, `debug perf` and the diagnostics bundle are missing; doctor is partial.
10. **Providers.** The fallback chain, additional adapters, multiple connections, command credentials and provider OAuth are all missing. Browser verification (M1.4) has no code either.
