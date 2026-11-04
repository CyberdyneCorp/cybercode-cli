# Daily coding implementation evidence

## Baseline and priority, 2026-11-03

The user prioritized #93 after authorizing #105's merge, superseding the original
Plugins/hooks scheduling dependency. #92 remains incomplete, including its native
admission and pending policy gates. The manual closure of #94 does not accept its
remaining retention/checkpoint/rewind and UX/performance scope.

The implementation branch starts from unchanged main `a6165c18`, the merge of
#105. Its tree `05da00a0bdb37288ecf2f55b902550c4a0fdb1bf` is identical to tested
head `6cfb7f2b`; all 19 configured checks passed, including Linux/macOS Rust,
Windows build and native binary proofs. Before edits, the focused server
`branch_checkout` regression passed locally (one test). This establishes the
existing branch race baseline, not completion of daily-coding acceptance.

| Clause | Baseline | First increment / remaining scope |
| --- | --- | --- |
| Rewind | Server stage/clear/commit and branch preservation exist; CLI command reserved | Add typed reviewed CLI and owner preview/confirmation; TUI, composer undo/redo and broader races remain |
| Non-Git snapshots | `NoSnapshots` outside available Git shadow workspaces | Private BLAKE3 backend, cap/retention and recoverable restore journal remain |
| Rules/context | Existing Epoch assembly and basic context endpoint | Path-scoped rule ownership and effective-source inspection remain |
| Root structured output | Existing child result validation; root schema ownership absent | Root per-input validation/finalization/retry implementation remains |
| JSONL | Exec reads a single prompt | Version-2 persistent driver, replies, correlation and draining remain |

## Reviewed CLI/server rewind

The new preview does not restore files, stage a revert, change the active branch
or start inference. It returns Session/directory/sequence/boundary/active-tip and
snapshot identities, actual proposed three-way file patches, current exclusions,
retained tool statuses and external/unknown-effect disclosure. The response is
limited to 4 MiB; that is not a bound on existing Git planning memory.

Planning constructs the proposed merged result in a separate shadow Git index.
Unrelated owner files are preserved and are not falsely shown as deletions. The
source files and live shadow index are unchanged. Conflicts are reported before
confirmation. The fingerprint binds the complete server-derived preview and is
not an authentication credential.

Confirmation recomputes the preview under idle Location ownership, revalidates
Session state/admission and holds the Session projection through checked restore
and the stage/commit event batch. Git rechecks the current tree after acquiring
its workspace mutation lock and refreshes admission before file writes. Custom
snapshot hosts must implement reviewed planning/restoration or code preview is
refused. Native failures or post-effect event-commit failures retain the native
activity fence and existing backup evidence; a successful transcript projection
is not fabricated.

The CLI defaults to preview and requires explicit `--confirm <fingerprint>` for
all targets. Conversation-only works without Git snapshots. Existing one-step
owner `/rewind` and explicit revert stage/clear/commit routes retain their prior
compatibility behavior and do not consume the fingerprint. The new flow does not
claim a new restore journal, complete historical exclusion reporting, root
structured-output implementation or arbitrary external-writer atomicity.

The canonical and overlapping active revert/search specs now preserve inactive
branches and independent recovery evidence. Root retry wording is reconciled to
one initial submission plus two corrections, separately from child results.
Archived historical artifacts are preserved.

## Local validation and review

On macOS, the final rewind increment passed four real CLI/server/scripted-provider
branch and rewind scenarios, four instrumented server ownership/recovery tests,
12 real Git snapshot tests, 79 CLI unit tests including generated-help consistency,
SDK typecheck and 128 SDK tests (10 configured external tests skipped), the current
OpenAPI artifact check, CLI/server/snapshot all-target Clippy, formatting and diff
checks. Strict OpenSpec validation passed 114 items; spec lint reported 0 errors
and 25 warnings. No external model provider was used. Linux and Windows execution
of this increment remains for CI; the main baseline results do not replace it.

The code-review pass identified and resolved: using effectful stage as a preview;
a target/current comparison falsely describing unrelated owner files as deletions;
the workspace-lock gap between checking the reviewed tree and restoring; false
settlement after a possible native effect; and silent finalization of an existing
legacy stage. Regressions cover each corrected behavior. Planning uses the shared
three-way planner with an isolated proposed index; confirmation uses the snapshot
port rather than duplicating filesystem behavior in the CLI/HTTP adapters.

This completes the implementation portion of task 2.2. Task 2.4 remains open for
broader native crash/restart/interrupt/recovery acceptance; TUI task 2.3 and all
later workstreams remain unfinished. The original one-step owner API remains a
compatibility path, not a newly reviewed surface. The new flow refuses an existing
stage until the owner explicitly commits or clears it. The retained native fence
is evidence of an unknown effect, not an implemented recovery-journal UI.
