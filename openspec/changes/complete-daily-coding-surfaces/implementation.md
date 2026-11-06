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


## TUI reviewed rewind implementation (2026-11-04)

Task 2.3 is implemented: `/rewind`, idle unmodified Esc-Esc, `/undo` and `/redo`
use the reviewed server flow. Busy interruption is explicit and waits for an idle
acknowledgement. Full-screen review includes actual patches, current exclusions,
retained tool outcomes and external-effect disclosure. The existing permission,
question and native-stop priorities remain. Draft replacement requires the exact
Session/Location/panel generation and an unchanged composer; late acknowledgements
and new attachment chips cannot replace a newer draft. Original text is retained
in a bounded in-memory map; restart does not reconstruct unsent drafts or media.

Server preview/confirm now supports explicit stage/clear actions. Reviewed file
stages persist the first baseline, raw selected snapshot and union of actually
changed paths in additive replayable events. Clear intersects that path set with
the three-way plan. Recording the whole post-restore tree was rejected in review:
it would adopt unrelated owner files into later redo authority. Legacy restage,
clear and branch checkout cannot drop the new manifest. Custom snapshot hosts
must implement reviewed staging; conversation-only remains available without it.

Local macOS validation: 642 TUI tests passed, three ignored; four server rewind
ownership tests and 12 Git snapshot integration tests passed. SDK typecheck and
128 tests passed, ten configured external cases skipped. TUI/server/snapshot
all-target Clippy passed. The real CLI binary built, with an observed nonfatal
linker unwind warning. Real terminal menu/review captures are in
`docs/screenshots/rewind-{menu,review}.png`; the sample edit is supplied by the
capture fixture during a held scripted-provider Turn, using actual snapshots and
HTTP preview without confirmation. Wide/narrow/light/NO_COLOR/screen-reader
render snapshots are included. Regression coverage exercises multiple stages,
original-baseline preservation, durable event replay, owner files, conversation
without snapshots, inactive branches, explicit busy interruption, changed drafts,
and replies delivered after a scope switch.

This completes implementation task 2.3, not acceptance task 2.4 or issue #93.
Native process-kill/restart and restore-journal acceptance, complete historical
exclusion reporting, private non-Git snapshots, path rules/context, root structured
output and JSONL remain open. No external model provider was used; Linux/Windows
execution remains for CI and subsequent native acceptance.

Final correction checks: all eight focused rewind tests passed after adding
same-Session generation/ambiguous-response checks and refusing typed undo/redo
before attachment consumption. Workspace all-target Clippy passed with warnings
denied; current OpenAPI regeneration/check, SDK generation consistency, typecheck
and 128 SDK tests (ten external skips), formatting and diff checks passed. Strict
OpenSpec again passed 114 items; spec lint reported zero errors and 25 warnings.
The code-review pass found no remaining blocking issue in this increment. Runtime
ownership stays behind snapshot ports; TUI HTTP orchestration is separate from
rendering. Native crash/restart and broader platform acceptance remain explicit
coverage gaps under task 2.4 rather than completed checks.

### PR #108 command-catalog CI correction

Linux and macOS CI each failed only
`services::tests::every_tui_slash_command_is_in_the_builtin_catalog` because the
new TUI undo/redo handlers were absent from the shared core catalog. Reproduced
that exact failure locally, then registered `undo` and `redo` beside `rewind` so
slash completion and the public command catalog expose them. The existing
regression remains unchanged. All 41 `cyber-app` library tests then passed;
core/app library Clippy with warnings denied, formatting and diff checks passed.
The full platform suites must rerun on the corrected PR head.

## Post-merge rewind recovery acceptance increment

Starting from merged #108 (`cf392c83`), the unchanged-main baseline passed four
real CLI/server branch scenarios and four server rewind ownership tests. This
increment adds recovery coverage without changing production behavior:

| Boundary | Evidence |
| --- | --- |
| Acknowledged code/both stage, then service kill | Real `cyber` CLI, private registered service killed with SIGKILL/TerminateProcess, fresh service reload, same baseline/path manifest, restage, second kill and reviewed clear; unrelated owner edits remain and stale clear reviews refuse |
| Conversation stage outside Git | Kill/reload, next input commits the staged boundary, inactive continuation remains exportable |
| Explicitly disabled snapshots | Code preview refuses; conversation stage survives kill/reload and a reviewed clear remains available |
| Busy Session and explicit interruption | Old confirmation refuses while busy and after acknowledged idle; only a fresh review applies, with inactive messages retained |
| Native acknowledgement loss | Instrumented snapshot port, then a fresh Runtime over the same durable Store; subtree stop reports unknown evidence and closes admission without replaying restore |
| Reopened terminal | Fresh production TUI App loads a server stage over HTTP; reviewed clear preserves the new terminal's draft without reconstructing the old unsent input |

The four new real-CLI recovery cases and server ownership suite are explicitly
selected by the native Windows nextest profile. Linux/macOS already run these
binaries in full. SIGKILL/TerminateProcess targets only a PID read from the
fixture's private `CYBER_HOME` registration; no production fault switch or
external process selection was introduced. Fixtures use scripted local providers,
real Git snapshots and authenticated owner routes.

Task 2.4 remains open for native partial-replacement crash cuts and complete
recovery acceptance. The unknown-acknowledgement test is an instrumented host
contract test, not proof of a killed native file replacement. The recoverable
restore journal, historical exclusion reporting and non-Git backend remain
unfinished; this test increment does not claim those capabilities.

Local macOS results: eight CLI branch/recovery scenarios passed; four server
ownership tests passed; nine focused TUI rewind tests passed. CLI/server/TUI
all-target Clippy with warnings denied, formatting, diff checks and generated SDK
consistency passed. Strict OpenSpec validation passed 114 items; lint reported
zero errors and 25 warnings. The code-review pass found no remaining blocker in
this increment; two long test scenarios were split into named recovery phases
rather than exempting them from Clippy's complexity limit. Linux/Windows execution
remains pending CI. Existing native C deprecation and linker unwind warnings were
observed during local builds; successful checks do not substitute for platform CI.


## Private non-Git snapshots and retained restore intents

This increment starts from merged #109 (`3dac1557`). The snapshot port selects
private file-copy storage for admitted Linux/macOS Locations outside Git, retaining
the existing Git backend. Task 3.1 is implemented. Tasks 3.2–3.4 and acceptance 2.4
remain open: managed keep/rollback resolution, native receipt acknowledgement,
journal reclamation and broader native crash/platform acceptance are unfinished.
The goal and issue #93 are not complete; #92 and owner-closed #94 keep their scope.

Objects are BLAKE3-addressed; manifests bind the canonical Location plus native
device/inode/creation identity. Every capture hashes bytes, including same-size/
same-mtime changes. Descriptor-relative no-follow traversal applies ignores,
private roots, per-file limits and unsupported metadata exclusions. An omitted
historical/current path is not a deletion. Review includes retained file-copy
exclusions and ordinary mode changes. Existing ownership is preserved; recreating
a deleted file prepares recorded uid/gid/mode in private storage or refuses before
publication. Missing parents are private. ACL/xattr/platform-flag/read-only files
are excluded until their metadata can be retained and recreated safely.

Restore checks the reviewed tree and admission under owned mutation/storage locks,
publishes a durable intent, prepares stages and uses native exchange/no-replace
operations. Actual displaced inodes are retained, with independently flushed
progress and final filesystem completion. Locks remain owned by the blocking
worker after async request cancellation. An incomplete/malformed journal refuses
further file-copy capture/restore; no automatic retry or rollback is inferred.
The read-only authenticated owner recovery route remains usable while the native
writer is paused, does not create absent namespaces, and never acknowledges a
Session/native receipt. `filesystem_complete` is deliberately limited to worker
filesystem completion; an absent `applied` entry does not establish no effect.

The default per-Location 200 MiB logical cap includes retained stages/preimages.
Optional snapshot warnings do not fail a successful CLI conversation or stop the
TUI Session. A complete application durable-reference view protects event-held
roots, including stages and inactive branches. Failed/missing reference collection
refuses pruning; every namespace containing a journal remains retained. Aged
unpublished temporary objects/manifests can be reclaimed when no journal exists.

The substantive code-review pass resolved: treating omitted files as deletions;
configured ignores overridden by project whitelists; blocking live inspection on
a writer-held lock; missing displaced-preimage quota reservation; privacy gaps
from inherited macOS ACL grants; and recreating files with weaker metadata.
Regressions exercise these corrections. Snapshot I/O remains behind the existing
server port; the application supplies private scopes and durable references, while
CLI/TUI only classify optional diagnostics and render existing reviewed responses.
No external reviewer or external model provider was used.

Local verification and CI results are recorded in the PR. Platform constraints are
explicit: stable creation identity, admitted metadata/native operations and
same-filesystem private restore storage are required; Windows file-copy writes
remain unavailable with an actionable reason and conversation-only fallback.
Git restore has not gained the new journal. The native CI matrix runs snapshot
proofs on Linux x64/arm64 and macOS Intel/ARM; Windows CI selects the explicit
unsupported-file-copy test. Broader cut points, metadata/platform combinations and
managed resolution remain coverage gaps rather than completed acceptance.


Local macOS validation for this increment: 17 file-copy tests and 12 existing Git
snapshot tests passed, including whole-plan conflict refusal and preimage-space
reservation before mutation. All 11 real CLI branch/recovery scenarios passed with
serialized fixtures; the initial parallel run stalled during first exec startup
before prompt admission and was terminated, not counted as a pass. Ten focused TUI
rewind tests and four server ownership/recovery tests passed. The current OpenAPI
artifact and generated SDK consistency checks passed; SDK typecheck and 128 tests
passed with ten configured external skips. Real Git-backed terminal menu/review
screenshots were recaptured and visually checked. Strict OpenSpec 1.13.2 passed all
114 items; spec lint reported zero errors and 25 warnings. Formatting and diff
checks passed. Native C deprecation and linker unwind warnings were observed;
Linux/Windows and additional macOS architectures require the PR's platform CI.

Workspace all-target Clippy with warnings denied also passed on the final code.


### PR #110 CI correction and merged-history integration

The initial PR head `6a8a33b8` failed its Linux x64/ARM snapshot proofs and the
Linux Rust suite because directory sync called fsync on cap-std O_PATH handles.
Sync now reopens `.` relative to the pinned descriptor with read/directory/no-follow
flags, retaining descriptor authority while obtaining an fsync-capable handle.
The new Linux regression explicitly constructs an O_PATH directory and syncs it.
The macOS ARM proof also reported a storage-lock acquisition failure; acquisition
now retries only bounded short contention/interruption before any restore starts,
keeps a live owner exclusive and reports other lock errors distinctly. Regression
coverage checks both lock handoff and refusal while the owner remains live.
No source mutation is retried by this acquisition change.

Merged current main `913fe1b0`, including #111's Ctrl+R history search, into the
existing PR branch without conflicts. The roadmap's 1026-requirement inventory and
history-search feature/docs/specs are preserved. A combined regression verifies
that a snapshot warning preserves the search and draft, cancelling search does
not arm rewind, and subsequent Esc-Esc opens rewind without replacing the draft.
Managed recovery resolution and journal reclamation remain outside this correction.


After integration, local macOS checks passed: two storage regressions, 17 file-copy
cases, 12 Git snapshot cases, 11 rewind-filtered TUI cases, nine history-search-
filtered TUI cases (one overlap), and three real non-Git CLI cases. Ten additional
parallel file-copy suite repetitions passed. Workspace all-target Clippy with
warnings denied, formatting/diff checks, generated SDK consistency and requirement
inventory checks passed. Strict OpenSpec passed 115 items after #111; lint reported
zero errors and 25 warnings. Linux O_PATH execution cannot run on this macOS host;
the corrected head requires fresh Linux x64/ARM and macOS platform proof CI. No
old-head green checks are used to accept this integrated revision.


## Historical Git exclusions and managed-recovery review

The 2026-11-05 implementation request includes managed recovery, native
acknowledgement, safe cleanup and all remaining daily-coding surfaces. The
[concrete recovery protocol](managed-recovery.md) is proposed for the required
security/data-model review before changing recovery admission; it is not
implemented by the historical-exclusion increment. Rules/context, root structured
output and JSONL retain their existing approved scope and remain unfinished.

The unchanged `b41f11f1` baseline passed two storage tests, 17 file-copy tests and
12 Git snapshot tests locally on macOS. Git snapshots now return immutable annotated
tree records containing exact Location and bounded excluded paths. Git itself peels
them in existing tree operations. Bare legacy tree identities remain readable with
explicit unknown exclusion history; history is never reconstructed from today's
configuration. Dedicated shadow refs retain tree and metadata together, including
across ordinary Git GC. Reference/lease-aware reclamation remains required before
these refs may be pruned; this increment does not claim that cleanup policy.

Capture enumerates ignored/size-excluded paths, compacts ignored directories and
removes newly ignored cached entries only from the shadow index. An unreadable or
failed capture refuses rather than fabricating a complete snapshot. Paths excluded
by either historical capture or the current capture are not deletion/restore
permission. New regressions cover identical Git trees with different exclusions,
restart/config changes, current-owner exclusions, legacy trees, foreign Locations
sharing an object store and a build directory with 4100 ignored artifacts. The
Windows native nextest profile now explicitly selects the Git snapshot integration
binary, alongside its existing unsupported file-copy case.

GitHub's issue timeline identifies PR #110 as the automatic closer of #93 at merge.
The PR description contained a negated closing-keyword reference despite unfinished
work. That text is corrected and #93 is reopened for the current authorized scope.
The owner's closed binary issue #94 is unchanged.


Validation on macOS: final snapshot tests passed (two storage, 17 file-copy and
17 Git cases). All 11 real CLI branch/recovery cases passed with serialized
fixtures; 11 focused TUI rewind cases passed. Workspace all-target Clippy with
warnings denied passed, followed by final snapshot tests and package all-target
Clippy after tightening validation before Git commands. Formatting/diff checks,
generated SDK consistency and requirement inventory passed. Strict OpenSpec 1.13.2
passed 117 items; lint reported zero errors and 25 warnings. No external provider
was used. Windows/Linux execution and the expanded Windows Git selection require
PR CI; no macOS result substitutes for their native proof.

The substantive review corrected directory inventory growth (4100 ignored build
artifacts compact to one prefix), newly ignored shadow-index entries, late object
identity checks and implicit omission authority. New Git refs intentionally retain
objects until a safe reclamation protocol is implemented. The managed recovery
design is awaiting owner review and the remaining rules/context, structured output
and JSONL are not claimed complete by this increment.

## Managed recovery continuation baseline, 2026-11-06

Fast-forwarded the clean primary checkout from `b41f11f1` to current main
`db9e171e`. No open repository PRs were returned by the live PR inventory.
Other worktrees were inventoried and left untouched. The recovery execution
breakdown M1–M8 in tasks.md maps the existing protocol to actual worker, snapshot
and Store interfaces without implementing or approving it. At that point task 3.0
remained open and the protocol was submitted for user review; the subsequent
decision and implementation are recorded below.

On unchanged Rust sources at `db9e171e`, `cargo test --locked -p cyber-snapshot`
passed two storage unit tests, 17 file-copy cases and 17 Git snapshot cases on
macOS. The platform-specific integration binary and doc tests each selected zero
tests; they are not native Linux/Windows evidence. No external provider was used.
Strict OpenSpec 1.13.2 validation passed all 117 items, spec lint reported zero
errors and 25 warnings, and inventory reported 53 capabilities/1030 requirements
with no ROADMAP diff. Diff whitespace checks passed. The global `openspec`
command was unavailable; validation used `npx --yes @fission-ai/openspec@1.13.2`.
These initial baseline checks excluded workspace-wide Clippy, the SDK suite,
real-client acceptance and new platform CI. Managed recovery and the remaining
daily-coding/plugin scope are still incomplete.

## Approved recovery protocol and atomic Store prerequisite

The user approved the concrete protocol on 2026-11-06: "Approve this protocol
and implement." Tasks 3.0/M1 and managed-recovery.md now record that decision.
The independent Linux plugin support, Setup and DirectoryAdded gates remain
unchanged. The existing recovery protocol is the implementation scope.

The first implementation prerequisite adds `Store::append_pair_checked` for the
native activity and resolution receipt owners. It requires two distinct bounded
IDs and exact sequences; decisions, events, sequence updates and projectors share
one writer transaction. A rejected second owner or COMMIT rolls back all changes.
Each changed owner is notified only after acknowledged commit. An actual WAL-hook
error after COMMIT proves both owners remain durable while notifications stay
silent and the writer is fenced; reopening permits inspecting the original records
and refuses the stale repeated append. This is Store evidence, not native worker
acknowledgement or a complete recovery retry implementation.

The unchanged-main Store baseline passed 49 tests. The updated Store suite passed
57 tests (12 unit, one process-kill, three MCP, 15 reservation and 26 Store cases),
including eight new paired-transaction cases. Coverage includes late sequence,
validation and projector failures, rolled-back decision SQL, deferred COMMIT
rejection, concurrency, restart, invalid owners, empty retries and cumulative
encoded-byte/event limits. Package all-target Clippy with warnings denied passed.
Workspace all-target Clippy with warnings denied also passed. Strict OpenSpec
validation passed all 117 items after the storage-events delta; spec lint retained
zero errors/25 warnings, inventory left ROADMAP unchanged, and formatting/diff
checks passed. No SDK/client generation is needed for this internal Store API.
All results here are local macOS results; Linux/Windows CI is not yet run. The
Windows selection explicitly includes the new Store lost-COMMIT unit regression;
its Store integration binary was already selected.

The code-review skill pass identified that size validation should not allocate an
unbounded serialized event copy; it now counts exact JSON encoding into a limited
sink. The pass also added cumulative cross-owner and empty-retry coverage and
explicit Windows selection. Clippy initially rejected one test's complexity
(16/15); extracting the shared scenario assertion resolved it without weakening
its assertions. The cognitive-complexity skill does not support Rust; the
repository's Clippy check supplies the applicable Rust gate.

No HTTP/SDK/CLI contract or database schema is changed yet. Exact snapshot-worker
origin, worker lock retention, new journals, owner recovery operations and safe
reclamation remain required before M2–M8 or the parent recovery tasks can close.
No recovery success or native acknowledgement is inferred from this prerequisite.

## Owned non-Git restore worker checkpoint

Reviewed non-Git code rewind, stage and clear now use runtime-issued synchronous
snapshot workers. The actual blocking callback retains its lifecycle, checkout,
workspace and private-store owners after requester disposal. A separate admission
owner permits inspection and Stop between source effects; each mutation takes the
Session projection lock and rechecks captured authority and revision. New prompt
admission refuses active or durable unresolved snapshot work before hooks and at
consuming admission; exact input retries remain readable. Reload restores the
typed worker fence. Generic and binary scopes cannot adopt snapshot origins.

Version-2 file-copy intents bind Session creation, activity, checkout, canonical
Location, native root and journal namespace/directory identities. Successful
restoration uses the paired writer to commit Session events and only its own
native acknowledgement, then acknowledges the journal under retained native locks.
Unacknowledged managed completion blocks reuse. Version-1 journals retain their
previous inspection semantics and never gain native settlement authority. The
semantic compatibility migration `20261106000000_snapshot_worker_ownership` adds
no tables, retains existing evidence and makes older migration catalogs refuse
mutation instead of ignoring typed fences.

The substantive code-review pass corrected holding the Session mutex across the
whole callback, early admission/hook bypass, treating an unobserved origin append
as effect authority, root rebinding, non-UTF-8 origin serialization, downgrade
compatibility and insufficient quota padding. Restore now budgets the full escaped
intent, all progress paths and acknowledgement metadata plus an atomic publication
copy and stage/preimage payloads before creating an intent. A fixture that fits
the former changes-only estimate is refused, while its larger-cap control passes.
Root replacement and symlink-to-displaced-root controls leave both roots unchanged.

Final macOS snapshot tests passed 39 cases (four unit, 18 file-copy and 17 Git).
The unsupported-platform integration binary selected zero on macOS. Store passed
58 cases including the new migration downgrade/reopen test. Affected server,
snapshot and Store all-target Clippy with warnings denied passed after the final
quota and migration changes. Earlier in this checkpoint, workspace all-target
Clippy passed; 12 admission-authority, three callback-authority, 13 Location
activity, four rewind-preview and six binary-document tests passed. The native
root identity test passed. OpenAPI consistency and SDK generation checks passed;
there is no network DTO change. Strict OpenSpec 1.13.2 passed 117 items; lint
reported zero errors/25 warnings and inventory remained 53 capabilities/1030
requirements with no ROADMAP change. Formatting and diff checks passed.
All seven runtime snapshot-worker tests passed on the final quota/migration code:
normal exact settlement, requester disposal/Stop/shutdown/reload, pre-effect
refusal, changed revision, rejected consuming commit, ignored failed origin append
and a non-UTF-8 metadata binding. The last is a serialization fixture, not positive
native support for non-UTF-8 filesystem paths.
All 12 real CLI branch/recovery cases passed serialized on that final code,
including owned non-Git stage/clear receipts and preservation of later owner edits.
The CLI build emitted native macOS sandbox deprecation warnings and a Darwin
linker unwind-size warning; these were observed, not classified against main.

This is partial M2/M3 infrastructure. It does not implement per-change native
source/stage/parent ownership evidence, managed keep/rollback review/confirmation,
request lookup/reconciliation or reclamation. Worker binding and matching bytes
are insufficient rollback proof. Git and legacy restore entrypoints are not yet
migrated to owned workers. Instrumented callback disposal/revision tests are not
real process-kill proof for the new typed restore path; existing killed legacy
file-copy tests do not substitute for that acceptance. Linux/Windows execution,
expanded native crash/race coverage and the SDK TypeScript suite were not run in
this checkpoint. No external model provider was used; CLI fixtures use a scripted
HTTP model. M2–M8 and parent recovery tasks remain open; neither issue is complete.
Crash reconciliation must include reservation-before-origin and
origin-before-intent cuts; this checkpoint conservatively retains their fences
and does not provide their owner recovery operation yet.
