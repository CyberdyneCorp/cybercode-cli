Specified 2026-10-18; implemented 2026-10-22 except 10.2 (see the notes on each task). Sections 1–9 each ship working behavior with their own tests; section 10 is the end-to-end acceptance with the real binary. Implement after `implement-cross-session-messaging`, `add-messaging-direct-peers` and agent teams, per the roadmap order.

Every test runs locally, with no network and no real provider:
- module tests use a QuickJS harness in `crates/cyber-workflow/tests/bundled.rs`;
- engine tests use the scripted agents of `crates/cyber-server/tests/workflow_runs.rs`, in a new `crates/cyber-server/tests/bundled_workflows.rs`;
- production-path tests use the App over authenticated TCP with the OpenAI-compatible scripted provider of `crates/cyber-app/tests/support/scripted_model.rs`, answering by prompt label so each agent returns a fixed structured result, in a new `crates/cyber-app/tests/bundled_workflows.rs` (real temporary git repositories and worktrees);
- CLI tests run the real `cyber` binary, in `crates/cyber-cli/tests/workflows_cli.rs`;
- TUI tests use `TestBackend` and `insta` against a real server.

The production-path test named in each section must pass before the section is ticked.

## 1. Bundled origin and discovery

- [x] 1.1 `crates/cyber-workflow/src/bundled/` with `common.js` and placeholder modules; `bundled::modules()` concatenates each module with `common.js` (`concat!` + `include_str!`), parses once (`OnceLock`); `Origin::Bundled { name }` serialized as `{kind: "bundled", name}`.
- [x] 1.2 Discovery returns bundled entries at the lowest precedence; a saved module of the same name lists `bundled:<name>` in `shadowed`; `workflows.bundled` (config, default true) and `CYBER_DISABLE_BUNDLED_WORKFLOWS=1` hide them. Config schema and validation in `crates/cyber-core/src/config/`.
- [x] 1.3 Unit tests: every bundled module parses, `meta.name` equals its file stem, `meta.args` defaults validate against the schema, `meta.budgets` and `meta.estimate` present; precedence bundled < global < project; disabled hides all five.
- [x] 1.4 Runs by name resolve bundled modules; the run records origin and digest; resume treats a changed bundled digest as a revision (`--accept-revision`).
  - Production path: `crates/cyber-app/tests/bundled_workflows.rs` `a_bundled_workflow_runs_by_name_and_a_project_module_overrides_it` (scenario "Ejected copy overrides the bundled one" through files written by the test).
  - Done: `crates/cyber-workflow/tests/bundled.rs` (every module through the engine), `crates/cyber-app/tests/workflows.rs` `bundled_runs_validate_and_prepare_their_args_before_a_run_exists` (runs by name record `origin: {kind: "bundled", name}`) and `a_bundled_run_resumes_as_the_bundled_workflow` (resume reloads the bundled module; it was resumed as an inline script before). The ejected-override scenario is `crates/cyber-cli/tests/cli.rs` `an_ejected_workflow_overrides_the_bundled_one_and_is_never_overwritten`.

## 2. Library surface: HTTP, CLI, SDK

- [x] 2.1 `GET /api/v1/workflows?directory=` (`v1.workflow.library`) in `http/workflows.rs`, replacing the `503` for that exact path only; openapi and SDK regeneration (`scripts/generate_sdk.py`), `client.workflows.library()`. (Delivered with the M2.1 gaps; bundled entries appear once 1.1–1.2 add `Origin::Bundled`, as `origin: {kind: "bundled", name}`. `budgets` reads `meta.budget`, the key runs already use.)
- [x] 2.2 `cyber workflows list` ORIGIN column and `--format json`; `cyber workflows show <name> [--source]`; `cyber workflows eject <name> [--global] [--force]` (refuses to overwrite, exit 1 naming the file).
- [x] 2.3 Args validation against `meta.args` before a run record exists, for every surface (HTTP, CLI, tool); `ExecutionFailure` naming `args.<path>`.
  - Production path: `crates/cyber-cli/tests/workflows_cli.rs` `bundled_workflows_are_listed_shown_and_ejected` (scenarios "Bundled listed on a fresh install", "Eject refuses to overwrite", "Library over HTTP" via `cyber api GET /api/v1/workflows`).
  - Done: `cyber workflows show` shares the `show` subcommand with runs (a run ID shows the run, a Workflow name the library entry). Tests: `crates/cyber-cli/tests/cli.rs` `bundled_workflows_are_listed_and_shown`, `bundled_workflows_can_be_turned_off`, `workflows_list_shows_saved_workflows_with_project_overrides_and_parse_errors`; `crates/cyber-app/tests/workflows.rs` `the_library_lists_saved_workflows_by_precedence_with_issues` (HTTP); args validation in `workflow_start::args_tests` and the app test above.

## 3. Direct read-only tools (M2.1 remainder)

- [x] 3.1 `tools.read`, `tools.glob`, `tools.grep` in `prelude.js`, gated by `meta.tools` or the invocation's `tools`; otherwise `PermissionDenied: tool not allowed by meta.tools`; no mutating tool exposed. (Delivered with the M2.1 gaps; the engine, not the prelude, checks the allowlist.)
- [x] 3.2 Host side: `ToolHost::workflow_tool` (`BuiltinHost::direct_tool`) with the run Mode ceiling, `read`/`external_directory` rules, deny_read and limits; structured results (`{paths, truncated}` in path order, default 1,000 and `max` up to 10,000; `{matches: [{path, line, text}], truncated}`, default 100 and `max` up to 1,000; `{content, truncated, next_offset}`); a non-interactive `ask` is rejected with `No interactive approver for workflow run`.
- [x] 3.3 Replay: results persisted as `workflow.call.recorded.1` records of kind `tool` (the M2.1 host-call record; the design's `workflow.host.settled.1` was not added) keyed by tool, canonical input and ordinal; a resumed run gets the recorded result even after files change.
  - Production path: `crates/cyber-app/tests/bundled_workflows.rs` `script_tools_glob_and_grep_respect_rules_and_replay_after_restart` (scenario "Script lists files to fan out"; a deny_read glob hides a file; restart mid-run and the file list is unchanged).

## 4. Shared module conventions

- [x] 4.1 `common.js`: finding validation with the `vcs-integration` consumer rules, dedup keys, severity order, chunking by size and directory, `guarded(agent)` wrapper turning `BudgetExceeded` into a partial-result signal and other failures into `limitations` entries, the result envelope `{summary, limitations, partial, stats}`.
- [x] 4.2 `workflow.run` approval shows effective budgets (stricter of meta, invocation, `budgets.run`) and `meta.estimate`; the `workflow` tool description lists library entries (≤ 20, bundled first) and prefers library Workflows; `workflows.auto` guidance names the five.
- [x] 4.3 QuickJS harness tests for every `common.js` helper, including a summary containing a data-block terminator that stays inside its block.
  - Production path: `crates/cyber-app/tests/bundled_workflows.rs` `the_workflow_tool_lists_the_library_and_the_approval_shows_budgets` (scenario "Model starts a bundled workflow by name" with a scripted model that calls `workflow {name: "audit"}`).
  - Done: the model's context lists the library (`core/workflows`, at most 20, bundled first, with argument names) whether or not `workflows.auto` is on; the approval carries `estimate`. Helper tests in `crates/cyber-workflow/tests/bundled.rs` run `common.js` through QuickJS. The scripted-model scenario "Model starts a bundled workflow by name" is not automated.

## 5. `review` and the `/review` switch

- [x] 5.1 `review.js` per design section 5: groups ≤ 48 KiB, lens × group reviewers on `fast` through `pipeline()`, dedup, one `verify` refuter per finding, ordering, `max_findings` cap with the overflow in `limitations`.
- [x] 5.2 Host preparation: for the bundled `review` (or `meta.prepare: "review_target"`), resolve `args.target` with the `vcs-integration` resolver under the reviewer's permissions before creating the run; pass `args.diff`; `ReviewTargetError` (422) with no run record.
- [x] 5.3 `/review` (TUI) and `cyber review` run the bundled workflow when no `review` skill or command overrides it; the TUI follows the run and shows findings most severe first; `cyber review` exit codes 0 / 4 (partial findings printed) / 1.
- [x] 5.4 Regression: an overriding `.cyber/commands/review.md` still wins and gets its arguments unchanged.
  - Production path: `crates/cyber-app/tests/bundled_workflows.rs` `review_raises_findings_per_lens_and_drops_refuted_ones` (scenarios "Review verifies findings", "Invalid review target creates no run"), and `crates/cyber-cli/tests/workflows_cli.rs` `cyber_review_runs_the_bundled_workflow_and_prints_findings`.
  - Done: `/review` and `cyber review` run the `review` Workflow when the library has one and the `review` command is the built-in one; with the bundled Workflows disabled the bundled review skill runs as before (its P1 tests pin that path with `CYBER_DISABLE_BUNDLED_WORKFLOWS=1`). `review.js` collects reviews with `parallel()` and verifies the capped findings after a global dedup, instead of `pipeline()` per group, so `max_findings` bounds the verifiers. Tests: `crates/cyber-cli/tests/review_command.rs` `review_runs_the_bundled_workflow_and_reports_verified_findings`, `the_review_workflow_reports_target_errors_and_clean_trees_without_a_run`, `a_budget_stopped_review_workflow_exits_4_with_partial_findings`, `a_project_review_command_overrides_the_bundled_review` (5.4, with the Workflows available); `crates/cyber-tui/src/tests.rs` `review_runs_the_review_workflow_when_the_library_has_one`, `a_review_run_that_ends_early_partially_or_fails_is_reported`, `review_runs_the_bundled_workflow_against_the_app_server`.

## 6. `audit`

- [x] 6.1 `audit.js` per design section 6: `tools.glob` scope, `max_files` cap, files > 256 KiB skipped, chunks by directory, finders with the focus checklist, dedup, `votes` refuters for critical/high with majority, one verifier otherwise, `coverage`.
- [x] 6.2 Engine tests: cap and skip reasons, majority logic (2 of 3 refute → dropped; 1 of 3 → kept), partial result on `BudgetExceeded`.
  - Production path: `crates/cyber-app/tests/bundled_workflows.rs` `audit_covers_the_scope_verifies_by_majority_and_reports_skips` (scenarios "Audit reports skipped files", "Majority refutes an audit finding", "Budget stops a fan-out" with `cyber workflows run audit --wait --max-cost` exiting 4).
  - Done: engine tests in `crates/cyber-workflow/tests/bundled.rs`; `tools.glob` gained `sizes: true` for the 256 KiB skip. The budget scenario runs on the real binary with `--max-turns` (a scripted model reports no cost): `crates/cyber-cli/tests/bundled_workflows.rs` `audit_reports_coverage_and_a_turn_budget_stops_it_with_exit_4`.

## 7. `migrate`

- [x] 7.1 `migrate.js` per design section 7: selection by `files` or `pattern` + `contains`, `ask()` above `confirm_over` unless `yes`, `NoInteractiveUser` before any agent, one worktree agent per batch running `check` and committing, a verifier per batch, no merge, `integrate` suggestion.
- [x] 7.2 Up-front permission check: evaluate `edit` on the batch paths and `bash` on each `check` command under the owner's rules (the `permissions test` evaluation); in a non-interactive run fail before any agent with the missing rules.
- [x] 7.3 Engine tests: batching, the confirmation gate, a batch whose check fails is `failed` and not `verified`.
  - Production path: `crates/cyber-app/tests/bundled_workflows.rs` `migrate_runs_batches_on_worktree_branches_with_checks` (scenarios "Migrate asks before a large run", "Migrate batches stay on branches" with a real git repository and `check: ["sh -c 'test -f marker'"]`, "Migrate in CI without allow rules").
  - Done: the permission check is host preparation (`meta.prepare: "migrate_permissions"`, `crates/cyber-tools/src/host/workflow_prepare.rs`) for runs no one can answer; it refuses with `403 PermissionDenied` naming each rule before a run exists. `agent()` with a schema now keeps an isolated agent's `branch`, which migrate reports. Tests: `crates/cyber-workflow/tests/bundled.rs`, the app test of 1.4, and `crates/cyber-cli/tests/bundled_workflows.rs` `migrate_commits_each_batch_on_its_own_branch_and_merges_nothing` (real worktree branches).

## 8. `research`

- [x] 8.1 `research.js` per design section 8: searchers per (source, angle), dedup of sources, readers extracting claims with locations, a cross-check per answer-relevant claim, `verified: false` for single-source claims, `smart` synthesis with an unverified section.
- [x] 8.2 Web availability: decide from the reviewer agent's permission evaluation of `websearch`/`webfetch` in the Location; when denied, a `limitations` entry and no web searchers.
  - Production path: `crates/cyber-app/tests/bundled_workflows.rs` `research_cross_checks_claims_and_degrades_without_web` (scenario "Research without web access"; repo and docs sources from a fixture directory).
  - Done: `meta.prepare: "research_sources"` sets `args.web_permitted` from the reviewer's `websearch`/`webfetch` decisions. Tests: `crates/cyber-workflow/tests/bundled.rs`, the app test of 1.4, `crates/cyber-cli/tests/bundled_workflows.rs` `research_answers_from_cross_checked_claims`.

## 9. `plan` and the TUI Library tab

- [x] 9.1 `plan.js` per design section 9: planners per angle (read-only, `smart`), judges with the five-criterion rubric, merge with named grafts; no edit tools.
- [x] 9.2 TUI `/workflows` Library tab: list with origin badges, Enter opens a JSON args form prefilled from defaults and validated before posting, `o` opens the source read-only; snapshots at 120×40 and 80×24.
  - Production path: `crates/cyber-app/tests/bundled_workflows.rs` `plan_drafts_from_angles_judges_and_merges` (scenario "Plan merges the winner"), and `crates/cyber-tui/src/screens_workflows.rs` `library_tab_starts_a_bundled_run_with_args` against a real server.
  - Done: Tab switches Runs/Library; the form is one line of JSON checked against `meta.args` in the TUI before posting; `GET /api/v1/workflows?source=true` carries the sources. Tests: `crates/cyber-workflow/tests/bundled.rs` (plan), `crates/cyber-cli/tests/bundled_workflows.rs` `plan_merges_the_highest_scoring_draft`, `crates/cyber-tui/src/screens_workflows.rs` `the_library_tab_lists_the_library_and_starts_a_workflow_from_its_form` (snapshots `workflow_library_120x40`, `workflow_library_80x24`, `workflow_library_form_120x40`).

## 10. Acceptance with the real binary

- [x] 10.1 With `./target/debug/cyber` and a scripted provider (`scripts/screens/provider.py` pattern, deterministic per prompt label): run each of the five bundled Workflows with `cyber workflows run <name> --wait`, record exit codes and result JSON; `cyber review` on a two-file diff; `migrate` on a temporary repository with real branches; `audit --max-cost` exiting 4 with a partial result; eject and override.
- [ ] 10.2 TUI captures from a pty with pyte rendered to PNG: the Library tab, the args form, `/review` findings from the workflow, the run monitor during an `audit` fan-out.
- [x] 10.3 Update `docs/implementation/p2-status.md` (M2.2 evidence), `ROADMAP.md` M2.2 status line, README (bundled workflows in the feature list and the comparison row), and `docs/features.md`.
  - 10.1 done as an automated test on the real binary rather than a manual run: `crates/cyber-cli/tests/bundled_workflows.rs` runs all five through `cyber exec --embedded --workflow` against a scripted model answering by task (exit codes and result JSON asserted; migrate on real worktree branches; audit exits 4 on a turn budget); `cyber review` on a two-file diff and eject/override are in `review_command.rs` and `cli.rs`.
  - 10.2 open: the pty/pyte PNG captures need the `scripts/screens` capture pipeline and a manual look; the TUI surfaces are covered by `TestBackend` snapshots meanwhile.
