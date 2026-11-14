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

## Prepared file-copy ownership and native cut evidence

Refreshed external state: origin/main remains `db9e171e`, issues #93/#92 are open,
and the repository has no open PRs. This continuation adds the per-file evidence
needed by M3/M4 without claiming the managed resolution operations exist.

New non-Git file-copy restores write version-3 intents, including the legacy direct
adapter path (which still has no runtime origin). Before a source namespace change,
the intent records native device/inode/birth identities for original and staged
files and every parent route. Missing parents are staged privately; their actual
native identities and ordinary metadata are durable before no-replace publication.
Original inodes, stages and staged parents are flushed before ownership publication.
The full journal quota includes this evidence and a maximum of 4096 distinct
parents. No missing progress entry becomes evidence that a rename did not happen.

Internal read-only inspection compares both live and private slots against native
identities and expected content/ordinary metadata. It does not populate content
objects, sync owner files, create parents, acknowledge native activity or change
the transcript. Same-byte foreign source/preimage/stage files, aliases, replaced
parents, populated/foreign parent stages and changed created-parent metadata refuse.
Existing owner-directory mode changes are preserved. Source effects recheck current
authority and parent routing before each parent publication and file operation.
Whole-plan recovery review, fingerprints and resolution intents remain to implement.

Version-1/2 journals stay readable without gaining historical per-file evidence;
unprepared version-3 intents retain their fence without claiming file proof.
Validation rejects malformed/duplicate native inventories and contradictory
progress/completion. No cleanup or legacy native-fence clearing was added.

The code-review skill pass found that macOS native metadata copying changed the
stage birth timestamp after its identity was recorded. Snapshot files support only
plain ownership/mode, so all supported stage metadata is now prepared before
recording identity and is not recopied during installation. Shared binary file
metadata checks retain their original behavior; a common fingerprint helper also
supports ordinary snapshot-created directories. The pass added original-inode
durability, per-parent revocation checks, same-filesystem preflight, metadata
conflicts and read-only object-inventory controls. Test helpers now pause based on
observed file effects rather than authorization callback counts. Clippy's test
complexity and nested-if findings were corrected without allowances.

Final local macOS verification: snapshot tests passed 54 cases (19 unit, 18
file-copy and 17 Git). One unit drives five real child-process kill cuts after
preparation, parent publication and each replace/delete/create, without writing
progress. Parent assertions observe the actual namespace before killing the child;
fresh Store owners then prove the retained native slots without replay. The child
helper's ordinary test invocation performs no cut; its five explicit child
invocations supply that evidence. The unsupported-platform binary selected zero.
Five Core native tests passed, including file/metadata exchange and ACL preservation.
All 12 real CLI branch/recovery tests passed on the final source, preceded by three
focused non-Git cases. Core/snapshot/CLI all-target Clippy with warnings denied,
formatting and diff checks passed. Strict OpenSpec passed 117 items, lint reported
zero errors/25 warnings, and inventory remained 53 capabilities/1030 requirements
without a ROADMAP diff. Native macOS sandbox deprecation and Darwin linker warnings
were observed; their origin was not assigned against main. No external provider
was used; CLI inference fixtures use scripted HTTP responses.

These process kills prove storage ownership, not typed runtime activity settlement
or keep/rollback acceptance. Git-native equivalence, managed runtime crash cuts,
reservation/origin/intent gaps, resolution/retry/acknowledgement reconciliation,
safe reclamation and owner API/SDK/CLI/TUI recovery remain open. Linux/Windows,
cross-filesystem native runs, the full workspace tests/Clippy, the runtime binary
document suite and SDK TypeScript checks were not rerun in this checkpoint. No
network DTO changed or SDK generation was required. M2–M8 and parent tasks remain
open; neither issue is complete.

## Recovery review and independent native resolution backend

The previous goal turn made verified progress in `52147d8d`. This continuation
adds owner review at `POST /sessions/{id}/rewind/recovery/review` and generated SDK
`session.snapshotRecoveryReview`. Selection binds the journal/action, selected
Session creation, current canonical Location/checkout and exact native activity.
Live, missing, foreign, generic or already acknowledged activity cannot acquire a
confirmation from that review. It starts no inference and changes no transcript,
source file or native acknowledgement. Review includes current, recorded
before/after, retained private and proposed image metadata; recorded created
parents; retained exclusions or explicit unavailable history; and conflicts. Its
response is bounded at 16 MiB. The simpler catalog retains its 4 MiB limit.

New intents retain the original snapshot selectors and bounded exclusion history.
The native backend fingerprints exact file identities/generations, parent
observations, immutable intent and action. Keep preserves reviewed ordinary owner
replacements without assigning them to the restore. Rollback requires strict
native ownership and stages from actual retained preimages, independently of
capture objects. Both publish a separate version-3 resolution intent and retain
actual displaced live inodes. A prior request ID is lookup-only, including an
incomplete outcome; different ownership/content refuses. A generation change after
preparation is not adopted as new source ownership. Whole-plan validation precedes
source effects and each file operation rechecks its reviewed source. Created
source directories remain protected for acknowledged safe cleanup.

The new snapshot port returns an explicit filesystem recovery receipt. Ordinary
rewind consuming commit refuses that receipt rather than acknowledging only the
new worker and pretending the original activity was resolved. Dedicated recovery
admission, global durable request ownership, exact original/resolution writer
commit, acknowledgement reconciliation and operator apply routes/CLI/TUI are not
implemented by this backend. Neither native fence is cleared by file completion.

Substantive code/security review found a replaceable-lock proof gap. New origins
bind the exact private lock inode and birth identity; new lock creation is flushed
before origin publication. Recovery opens an existing namespace and its existing
exclusive lock without creating missing evidence. The regression actually replaces
a live owner's lock and acquires the different inode, then proves native recovery
refuses that false exit proof. Origins without the binding remain inspectable and
fenced. Inspection ignores capture disable/lowered capture size while retaining a
64 MiB native file bound; resolution writes retain the configured total cap.
Retained file-copy journals are selected by provenance rather than current capture
enablement or later Git discovery. Git-native journals remain unimplemented.

Review also corrected capture-object dependence, a private-preparation adoption
window, missing visible retained images, and stale native generations. Tests cover
whole-plan conflict refusal, keep after owner edits, partial resolution evidence,
duplicate request refusal without even calling authorization, quota failure and
retry after capacity is raised. Runtime metadata tests distinguish live/unknown/
settled origins and refuse generic or mismatched activities. They are instrumented
attestation tests, not native consuming-transaction proof.

Final local macOS snapshot tests passed 67 cases (32 unit, 18 file-copy, 17 Git);
the unsupported-platform binary selected zero. All 11 focused runtime snapshot
tests passed. All 13 real CLI branch/recovery cases passed; the final review case
was rerun after the last canonical-Location validation and passed. It verifies
Session ownership, acknowledgement limits, retained exclusion history, unchanged
files/messages and no additional scripted-provider inference. Affected server,
snapshot and CLI all-target Clippy with warnings denied passed. OpenAPI consistency
and SDK generation checks passed. SDK typecheck passed; SDK tests passed 129 cases
with ten configured process end-to-end cases skipped because `CYBER_BIN` was unset.
The new SDK review transport contract ran. Strict OpenSpec passed 117 items; lint
reported zero errors/25 warnings, inventory remained 53 capabilities/1030
requirements, and formatting/diff checks passed. Native sandbox deprecation and
Darwin linker warnings were observed without attributing their origin to main.

No external model provider was used. Linux/Windows, full workspace tests/Clippy,
TUI/App recovery acceptance, real SDK process acceptance, native resolution
process-kill cuts and paired runtime lost-reply/reconciliation were not run in this
checkpoint. The earlier storage kill tests still run but do not substitute for
these missing resolution/runtime cuts. Late-scope disposal and uncertainty-reason
requirements are recorded in tasks.md before enabling acknowledgement. The remote
refresh still reports main `db9e171e` and no open PRs. M2–M8 remain open; the full
#93/#92 goal remains active and neither issue is complete.

### Late native-scope disposal safeguard

Scope disposal now reads the latest activity in the same Store writer transaction
as its conditional append. It checks Session, worker kind, document/origin and
expected reservation/launch sequence. An already-settled exact owner is a no-op:
its complete receipt, event sequence and notification stream are preserved. A
changed owner or sequence refuses the append and retains conservative uncertainty.
Rejected origin publication also no longer leaks the attempted in-memory origin
into the reservation's disposal event.

Two new instrumented runtime regressions cover late disposal after an independently
committed acknowledgement (with and without an unrelated uncertainty flag), and
changed journal ownership. They check unchanged durable payload/sequence and no
notification from either the no-op or rejected transaction. The existing rejected
origin regression now verifies that no origin was manufactured by disposal. These
use the real Store and worker lifetime but inject the acknowledgement event; they
do not prove the still-unimplemented consuming recovery commit or a native crash.

Final local macOS server unit tests passed 396 cases, with one ignored subprocess
fixture (`database_lock_probe`, invoked explicitly by its parent regressions).
Server all-target Clippy with warnings denied passed. Strict OpenSpec validation
passed all 117 items. Formatting and diff checks passed.

The shared idle-cancellation and Location integration binaries also passed (four
and thirteen tests respectively), exercising acknowledged versus unknown disposal,
shutdown, cancellation timeouts, worker panic and retained resource ownership.

The code-review skill was applied to the changed disposal path and tests, including
writer serialization, exact ownership, corrupt/missing evidence refusal, no-op
notifications and shared binary/generic-worker behavior. Review caught and fixed
the rejected-origin serialization issue. No new public operation or acknowledgement
authority is added. Opaque Location uncertainty is never cleared here. The wider M5
work must still distinguish snapshot uncertainty from other reasons and coordinate
its in-memory state with recovery acknowledgement, including acknowledgement that
follows a successful unknown-disposal append. Git, runtime request ownership,
reconciliation, cleanup and native/client acceptance remain open. No Linux/Windows,
full-workspace or real recovery operator acceptance was run for this safeguard.

### Snapshot uncertainty provenance and committed journal-ack failure

The Handle now retains exact snapshot activity IDs separately from opaque native
process/checkout uncertainty. All existing Location uncertainty checks combine
both reasons. Reload reconstructs both from durable latest activity records,
bounded to 4096 unresolved owners; it no longer folds snapshots into an opaque
boolean or drops generic native fences. Document-specific workers keep their
separate document recovery contract, and ambiguous mixed worker kinds are opaque.
No legacy record gains snapshot authority from reload.

Snapshot disposal publishes its exact in-memory reason before enqueueing its
writer decision. If that writer observes the already-settled exact owner, it
removes only that reason. There is no caller-side post-commit snapshot flag write
that can re-fence an acknowledgement after the writer has consumed it. The opaque
flag and every other snapshot ID survive. Combined reads hold the snapshot mutex
while reading the opaque flag so removing a snapshot reason cannot hide an opaque
reason published before that removal. The eventual consuming recovery transaction
still needs its exact native proof and post-commit reconciliation.

Owned rewind now delegates uncertainty to the actual worker instead of marking an
opaque outcome for every commit error. If the Session/native writer pair commits
and the later journal acknowledgement fails, the error explicitly reports the
committed runtime acknowledgement and need for journal reconciliation. The native
activity stays settled, the committed branch change remains visible, and repeating
the stale rewind confirmation cannot replay the restore. This does not implement
the missing journal reconciliation operation or authorize cleanup.

The restart regression combines an interrupted snapshot and generic process, then
simulates an independent exact snapshot acknowledgement and verifies that reload
still refuses new snapshot work because of the generic fence. The late-disposal
regression covers no unrelated reason, an opaque reason and another snapshot ID
in independent fixtures. A new scripted-provider server integration exercises the
owned rewind confirm path with an injected journal-ack failure, checks the committed
branch/native state, stale-confirm refusal, held input admission before/after
reload, and exactly one restore invocation. Filesystem identities and the failed
journal flush are instrumented; this is not native recovery/reconciliation proof.

Local macOS checks passed: 397 server unit tests (one ignored subprocess fixture,
invoked by its parent Location identity tests), four idle-cancellation, thirteen
Location activity and five rewind-preview integration cases. Strict OpenSpec
passed all 117 items; server all-target Clippy with warnings denied and
formatting/diff checks passed. No external model provider,
new public schema or SDK contract is involved. Linux/Windows, full-workspace,
real CLI/TUI recovery, native resolution crash cuts and lost-writer-reply journal
reconciliation remain unperformed for this increment. M2–M8 and both issues remain
open.

Substantive code/security review covered all combined reads, opaque writers,
reload classification, exact-ID removal at the writer, failure/lifetime ordering,
the owned versus legacy commit paths and shared binary worker behavior. It
corrected the potential post-commit flag race and combined-read ordering. The
broader recovery authority, native consuming transaction and reconciliation gates
remain required before operator apply or cleanup is admitted.

### Non-Git recovery admission and consuming owner pair

The file-copy recovery path now connects review to a durable request reservation,
the actual owned filesystem worker and the exact consuming native transaction.
`POST /sessions/{sessionID}/rewind/recovery/confirm` accepts the journal, action,
fingerprint and stable request ID. `GET .../recovery/requests/{requestID}` reads only
the durable request (404 when absent). OpenAPI and typed SDK methods are generated.
Existing request IDs return lookup before native access, including unknown outcomes
and later owner edits; different content cannot reuse the request. Lookup reports
journal acknowledgement as unobserved rather than inferring it from runtime
settlement. A request resolved by another request has a distinct result state.

The reservation aggregate derives from Session creation and a length-framed request
ID hash, before checkout/native preparation. Snapshot-only recovery authority
captures ancestry, local/subtree revisions, active closed scopes and solve fence
revisions. It preserves those boundaries and cannot lend ordinary launch authority.
The Store projector validates the exact reservation and immutable launch metadata;
it does not grant generic hook/process admission through a snapshot fence. Ordinary
native admission and new ordinary snapshot admission are mutually fenced at the
writer. Prompt, child and tool admission targets also check the durable snapshot
fence there, so a stale caller precheck cannot race a newly committed reservation.
Regressions verify rejected native reservation and input append leave no published
ownership or inbox row. Recovery refuses live local native controls/background work. The
actual callback retains checkout/lifecycle/workspace/private-store ownership.

After native guard revalidation, the writer pair checks both original/resolution
origins, the captured cancellation boundary, current Session revision and exact
request/receipt. Only an observed durable pair settles the original native owner
and records the resolution receipt, removing only the original snapshot reason.
It emits no old rewind or transcript events and preserves generic unknown outcomes.
Normal rewind commit cannot consume a recovery result or lend its authority.

Native journal acknowledgement follows that observed writer commit. It publishes
the resolution acknowledgement first, then a validated settlement link in the
original journal. The original may remain `completed: false`; its history is never
rewritten as a completed restore. Missing/mismatched acknowledgement references
retain the fence. Metadata budgeting includes atomic replacement of the parent
settlement record, and no source directory or retained payload is reclaimed.

Substantive code/security review corrected ordinary hook admission through the
snapshot fence and private intent changes during the consuming handoff. The native
worker now pins the immutable intent digest and exact filesystem receipt. The
resolution retains the reviewed completion/acknowledgement state, allowing its
fingerprint to be verified independently of later mutable acknowledgement flags.
Tests reject a changed frozen review and changed intent context even when live
files and the returned receipt are unchanged. Old resolutions without this frozen
review state remain readable but cannot acquire that missing consuming proof.
Origins without the exact native lock proof remain inspectable and fenced.

Real CLI/API acceptance creates 1280 non-Git files, records two snapshots, observes
an actual source exchange during restore and kills the real service while the
namespace is partial. A fresh service observes both applied and unapplied source
files. Separate keep and rollback scenarios then confirm recovery, inspect the
durable settlement link, preserve unrelated owner data and the unchanged
conversation, and verify no additional scripted-provider inference. Keep also
preserves a later ordinary owner image. Retries after another owner edit and after
another service restart are lookup-only; changed request content is refused. This
uses actual file-copy operations, server, CLI and process kills, without a restore
fault hook. It covers one observed partial-exchange cut per action, not every cut.

Runtime regressions preserve a captured closed boundary and unrelated generic
unknown evidence, reject a new stop boundary before effects, and reject a changed
Session revision before acknowledgement. The WAL-hook commit-reply-loss regression
uses a real file-backed Store: both owner settlements persist, no commit
notification or file acknowledgement is emitted, and lookup finds the same receipt
after the Store is closed and reopened. Its filesystem callback is instrumented;
it is not a service kill at the writer boundary or journal reconciliation proof.

Final local macOS evidence: 404 server unit tests passed (one ignored subprocess
fixture invoked by its parent Location identity tests), four idle-cancellation,
thirteen Location and five rewind-preview integration cases passed. Snapshot
evidence includes 35 final unit cases, eighteen file-copy and seventeen Git cases
(70 unique tests). All fourteen CLI branch/recovery cases passed; the new real-kill
case was rerun after the helper refactor and added request-content refusal and
passed for both actions. SDK typecheck and generation checks passed; SDK tests
passed 130 cases with ten configured process end-to-end cases skipped because
`CYBER_BIN` was unset. OpenAPI consistency passed. Server/snapshot/CLI all-target
Clippy with warnings denied passed. The complexity skill has no Rust analyzer;
Clippy found the test helper at 32/15, and extracting its kill, settlement-link and
retry phases passed the repository threshold without allowances. Formatting and
diff checks passed. macOS sandbox deprecation and Darwin linker warnings were
observed without assigning their origin to main.

The complete feature is not merge-ready. At this checkpoint explicit journal
acknowledgement reconciliation after lost replies or failed flushes was absent;
the following continuation supplies it for committed recovery requests. Reservations
that never publish a native origin remain unknown and lookup-only; missing records
do not prove no effects or authorize clearing checkout/process uncertainty. Those
resource/intent cuts require their own exact proof and operator resolution. Git
journaling, interrupted-resolution/pre-origin acceptance, reference/reader/lease
protection and two-phase reclamation, typed CLI/TUI/App recovery, full native cuts,
cross-platform results and complete delivery review remain open. Linux/Windows,
full-workspace checks and real SDK process acceptance were not run here. M2–M8 and
both issues remain open. A remote refresh still reports main `db9e171e` and no open
PRs; this increment has not been pushed or merged.


### Committed recovery-request acknowledgement reconciliation

The explicit POST `/sessions/{id}/rewind/recovery/requests/{request_id}/acknowledge`
operation observes both durable native owners in one database read snapshot and
requires their exact committed receipt. It never admits an unknown or merely
filesystem-complete request. A separate runtime-issued metadata worker retains
actual lifecycle, Location and synchronous native store/workspace owners through
its last call. Its live control remains available to Stop; dropping the requester
cancels work but does not release those resources before the callback finishes.
The control is volatile because no new source effect or intent is admitted; the
already committed owner pair remains the durable recovery identity after a crash.

New receipts persist an immutable resolution digest. Native reconciliation pins
that digest, the original intent and both private namespaces, then writes child
acknowledgement before the parent's settlement link, checking current runtime
authority before each write. A cut between writes retains the fence and supports
idempotent retry. No source-file scan, replacement, inference or transcript event
is involved. Older receipts without the digest can only observe an already fully
acknowledged settlement; they cannot grant new metadata writes. Unrelated opaque
uncertainty remains unchanged.

Regression evidence includes a real file-backed Store committed-but-lost reply
followed by reconciliation despite its fenced writer, rejection of an uncommitted
filesystem receipt, and requester disposal while a synchronous callback retains a
real semaphore permit through shutdown. Native tests reject modified immutable
intent and missing legacy digest, cut between the two journal writes, then retry
while preserving later owner edits. The real CLI/API keep and rollback scenarios
also reset only mutable acknowledgement fields after a genuine prior partial-
restore kill, then acknowledge with a source replaced by an external-target symlink.
Both preserve that symlink, its target, transcript and inference count. This fixture
models the acknowledgement cut; it is not an actual process kill at that boundary.

At that checkpoint, the recovery-request path did not reconcile an ordinary successful rewind's
Session/native receipt, reservations lacking native origin, Git journals, or
reclamation. Those remain required alongside complete clients and platform cuts.
M2–M8 and both issues remain open.


Verification for this continuation on local macOS: 406 server unit tests passed,
with the one configured subprocess fixture ignored; all 22 affected integration
cases passed (4 idle-cancellation, 13 Location, 5 rewind-preview). Snapshot tests
passed 72 cases (37 unit, 18 file-copy, 17 Git). The focused real CLI/API case
passed both keep and rollback scenarios. SDK typecheck and generation checks
passed; 131 SDK tests passed, with 10 configured process tests skipped because
`CYBER_BIN` was unset. OpenAPI consistency and final server/snapshot/CLI all-target
Clippy with warnings denied passed. Formatting, diff checks and strict OpenSpec
passed (117 items); spec lint reported 0 errors and 25 warnings, and inventory
remains 53 capabilities/1030 requirements. The initial integration invocation used
nonexistent shorthand target names; the corrected three actual targets above all
ran successfully. No Linux/Windows, full workspace, TUI/App recovery or real SDK
process acceptance was performed for this continuation.

The code-review skill was applied to every changed runtime/native/HTTP/test and
generated SDK file. Review checked exact paired ownership and read-snapshot
consistency, digest compatibility, private namespace/lock binding, child-before-
parent durable ordering, cancellation/resource lifetime, preservation of generic
uncertainty, route ownership and replay separation. The review added the explicit
read transaction to the acknowledgement factory so both owners are observed from
one snapshot. No additional critical/major findings remain for this increment;
the missing broader recovery capabilities above remain acceptance gaps for the
complete feature. Architecture follows the repository's runtime port and native
adapter split. The increment remains local and does not make M2–M8 complete.


### Ordinary file-copy restore acknowledgement reconciliation

New ordinary restore settlements retain `SnapshotRestoreReceipt` on their exact
native activity. The native guard supplies its captured immutable intent digest;
the existing paired append binds it to the same Session event batch by sequence
range and SHA-256 digest of serialized event kind/data/causation IDs. The batch is
bounded to 4096 events. No new Session event or transcript movement is introduced.
Older adapters omitting immutable proof retain their existing settlement behavior,
but cannot authorize later acknowledgement writes through this API.

GET `/sessions/{id}/rewind/restores/{activity_id}` observes the original native
settlement and exact Session batch in a single read transaction; absent, foreign
or historical receipts without this proof return 404. POST to its `/acknowledge`
child obtains the same metadata-only worker resources used for recovery requests.
The worker verifies current Session revision/authority before the native adapter
checks original lock, private namespace and journal identity, completed ordinary
intent and immutable digest. It writes only the acknowledgement flag. It neither
rescans source files nor invokes restore. Later Session batches do not invalidate
the original receipt, but changes during the acquired worker remain fenced.

The real CLI/API regression resets only mutable journal acknowledgement after a
successful ordinary non-Git rewind, creates a symlink at the restored path to an
external file, then performs acknowledgement twice. Both responses retain exactly
the lookup receipt, and the source symlink, target bytes, transcript and inference
count remain unchanged. This is a simulated acknowledgement-flush cut, not a real
process kill at that boundary. Additional runtime coverage uses an actual file-
backed Store WAL hook to lose the paired commit reply, retains lookup evidence,
refuses acknowledgement against stale in-memory Session state, reopens the Store,
adds a later Session revision, and reconciles without calling the source worker.
The native adapter fixture refuses incomplete/digest-mismatched intents and a
revocation before its final metadata write. These fixtures do not supply production
runtime authority by themselves.

Pre-origin ownership proof, Git journal equivalence, safe reference-aware cleanup,
complete CLI/TUI/App recovery, remaining crash cuts and platform acceptance remain
required. M2–M8 and both issues stay open; the feature is not merge-ready.


Final local macOS arm64 verification: 407 server unit tests passed with one ignored
subprocess fixture; all 22 affected integration cases passed (4 idle-cancellation,
13 Location, 5 rewind-preview). All 73 snapshot cases passed (38 unit, 18 file-copy,
17 Git), including the native cancellation/digest/incomplete-intent regression.
All 14 real CLI branch/recovery cases passed, including the two actual partial-
restore kill scenarios and the new ordinary acknowledgement fixture. SDK typecheck
and generation consistency passed; 132 SDK tests passed and 10 configured process
tests were skipped because `CYBER_BIN` was unset. OpenAPI consistency, formatting,
diff checks and server/snapshot/CLI all-target Clippy with warnings denied passed.
Strict pinned OpenSpec passed all 117 items; lint reported 0 errors/25 warnings,
and inventory remains 53 capabilities/1030 requirements. Native sandbox deprecation
and Darwin linker warnings were observed without assigning their origin to main.
Linux/Windows, full-workspace and TUI/App recovery acceptance were not run here.

During verification the new Store fixture initially used the wrong options type,
then installed its lost-reply hook on a reader rather than the actual writer. The
corrected writer hook proves an actual durable paired commit with a lost reply and
no notification. The new native fixture initially omitted required progress from
its completed journal; it now performs all native installs and records valid
completion before testing metadata-only acknowledgement. The SDK regression also
caught the optional response wrapper contract; missing receipt lookup now returns
404 consistently with recovery-request lookup. Final tests above supersede those
failed fixture/contract runs.

The code-review skill was applied across the changed runtime/native/HTTP/test/spec
and generated SDK files. The review checks Session/activity key equality, native
origin/type separation, read-transaction consistency, sequence bounds and batch
identity, current authority/cancellation, exact native locks and namespace, and the
absence of source reads/replay during acknowledgement. Lookup checks both requested
owner keys against the receipt, and even an empty batch requires its original
Session sequence anchor. The repository runtime port/native adapter separation is
preserved. No critical/major findings remain for this increment; broader feature
acceptance gaps remain recorded above. A remote refresh reports main `db9e171e`,
issues #92/#93 open and no open PRs. This increment is local, not pushed or merged.


### Exact lifetime evidence before snapshot origin

The pre-origin audit found that the current Session-owned checkout lease cannot
prove exit of one snapshot worker. New file-copy workers now obtain a separate
private lifetime file lock before the runtime writes reservation sequence zero and
before it acquires checkout resources. The immutable binding includes exact native
Location/namespace/worker-directory/file identities and activity/Session-creation/
checkout ownership. The native activity retains it unchanged through origin,
receipt settlement and late disposal; native attestation rejects a changed binding.
`SnapshotWork::authorize` revalidates the native route before effects. The guard is
the last worker field released, after its resource owners. Hosts lacking this port,
Git paths and historical reservations retain no lifetime proof.

The record is flushed privately, then published under its stable activity name by
native no-replace rename. Interrupted private stages remain retained, without
poisoning a stable request owner name or deleting evidence to retry. Reopening an
existing worker requires the exact immutable payload and native identities; missing,
replaced, unsafe or live locks refuse. Lifetime metadata is included in the same
per-Location quota as journal/object/manifest data. GC conservatively stops pruning
while lifetime records or stages exist. This retains evidence until M6 supplies
complete reference accounting and durable two-phase reclamation; it does not claim
that reclamation or full retention/cap acceptance is implemented.

Runtime regressions retain a real semaphore-backed lifetime through requester
abort and shutdown before an origin exists, and reject a foreign lifetime before
publishing any reservation. Native tests check private permissions, quota refusal
before owner-file creation, replacement/missing-proof refusal and conservative GC.
A native child actually holds the lifetime lock after dropping its Store: a fresh
Store refuses the live lock, then reacquires the exact identity after killing and
waiting for that process. A second process cut pauses after the staged record's
flush but before no-replace publication; its death leaves that stage intact while
a fresh worker publishes a distinct complete lock. These are storage-level cuts,
not real service/operator acceptance for the complete early-recovery protocol.
The real CLI assertion now checks that a successful ordinary rewind's runtime
record contains the same immutable lifetime payload as its private file.

No pre-origin acknowledgement path is enabled by this increment. A missing origin
or journal remains insufficient to acknowledge checkout/process uncertainty. The
operator consuming proof, explicit source-admission phases, managed-checkout
ownership equivalence, Git integration and remaining native cuts remain required.
M2–M8 and both issues remain open.


Final local macOS arm64 evidence: 409 server unit tests passed (one ignored
subprocess fixture invoked by its existing parent tests), all 22 affected
integrations passed (4 idle-cancellation, 13 Location, 5 rewind-preview), and all
77 snapshot cases passed (42 unit, 18 file-copy, 17 Git). Snapshot tests list one
ignored lifetime subprocess fixture, which both new process-kill parents invoke.
All 14 real CLI branch/recovery tests passed, including ordinary rewind's persisted
lifetime assertion and actual partial-restore keep/rollback kills. SDK typecheck,
generation consistency and 132 SDK tests passed; 10 configured process tests were
skipped because `CYBER_BIN` was unset. Server/snapshot/CLI all-target Clippy with
warnings denied passed. The expanded CLI receipt assertion initially exceeded
18/15 complexity; extracting its lifetime assertion passed without allowances.
Formatting, diff checks and strict OpenSpec passed (117 items); lint reports
0 errors/25 warnings and inventory stays 53 capabilities/1030 requirements. Native
sandbox deprecation and Darwin linker warnings were observed without assigning
their origin to main. Linux/Windows, full workspace, real SDK process and TUI/App
recovery acceptance were not run.

The code-review skill was applied to every changed runtime/native/test/spec file.
Review checks lifetime-before-reservation ordering, unchanged exact binding through
native attestation and disposal, guard teardown order, immutable payload and native
route/descriptor consistency, private modes/alias rejection, quota accounting and
reference protection. Review changed construction to staged no-replace publication
so a crash cannot leave a partial stable owner record, and reads payload and native
identity from the same descriptor during verification. The runtime/native adapter
port separation is retained. No critical/major findings remain for this prerequisite;
its explicitly missing consuming recovery protocol and broader acceptance gates
are not accepted by these tests. This is local progress, not a merged delivery or
completion of either issue.


### Consuming an exited reservation before origin publication

The owner can now list `/sessions/{id}/rewind/preparations`, POST the selected
`/{activity_id}/review` and `/confirm` operations, and GET its
`/requests/{request_id}` receipt. The matching typed SDK operations are generated.
Review/confirmation require a positive sequence-zero filesystem-only snapshot
reservation, an unchanged exact lifetime binding, no published origin, and at most
its unlaunched unknown-disposal record. Acquiring the original native lifetime lock
proves exit. Missing journal records are never used as evidence. Legacy, generic,
binary, live, foreign, changed or already-launched owners refuse.

Confirmation retains current recovery authority and Session revision without
reopening closed boundaries. It atomically pairs settlement of that exact native
activity with a separate Session-creation/request-derived cancellation receipt.
The real proof, lifecycle and active owner stay retained through the writer's
COMMIT and its observed reply. Cancellation changes no Session events or source
files, clears only its exact cached snapshot reason, and preserves opaque and other
snapshot uncertainty. Read-only lookup validates both durable owners before clearing
that same cached reason after a lost reply. Changed request content or cross-activity
reuse refuses before native access; repeat confirmation is lookup-only. Receipt
validation bounds native sequences and fingerprints before interpreting persisted
data, and ordinary refusal paths use conflict errors rather than generic failures.

An unstarted resolution's own cancellation receipt does not acknowledge its failed
parent restore. Recovery-request lookup reports `cancelled_before_origin`, no
resolution origin/receipt and unobserved journal acknowledgement. The parent still
requires its original keep/rollback recovery. Managed checkout/process uncertainty
is neither inferred settled nor cleared by this operation.

Runtime tests model an abrupt pre-origin cut by rejecting only the scope's disposal
append with a writer-local SQLite trigger. They verify live refusal, exact paired
settlement, independent uncertainty, closed native-port retry, content/cross-owner
refusal and unchanged Session revision. A second trigger rejects insertion of the
request receipt after the original native append, verifying rollback of both owners
and absence of notifications. An actual file-backed Store WAL-hook case commits but
loses the pair reply, reopens that Store under `NoSnapshots`, and finds the original
receipt without native replay. The unstarted resolution test proves the parent
restore stays unknown. These are runtime/Store fixtures, not process-kill proof.

The real CLI/API test prepares 1280 binary files with shared 64 KiB images, starts a
reviewed rewind, observes its durable reservation at sequence zero, kills the actual
service and verifies no origin was published. After restart, it creates a later
owner symlink, reviews/confirms cancellation, kills/restarts again, and checks the
same receipt by retry and lookup. The symlink/target bytes, another source image,
transcript and inference count are preserved; an explicit subsequent prompt is
admitted. The cut uses actual service death without a restore fault hook. The first
fixture used large text diffs and correctly hit the 4 MiB preview bound; binary
images keep that reviewed preview bounded while retaining the native scan window.

Origin-before-intent still needs explicit positive source-admission phases and its
own consuming proof. Independent managed-checkout recovery, Git journaling,
reclamation, typed CLI/TUI/App recovery and full platform/cut acceptance remain open.
M2–M8 and both issues are incomplete; this does not make the whole feature merge-ready.


Final local macOS arm64 verification: 413 server unit tests passed with one ignored
existing subprocess fixture; all 22 affected integrations passed (4 idle-
cancellation, 13 Location, 5 rewind-preview). All 77 snapshot cases passed (42 unit,
18 file-copy, 17 Git), with its lifetime child fixture listed ignored and invoked
by its real process-kill parents. All 15 real CLI branch/recovery cases passed,
including the new pre-origin service kill and existing partial-exchange cuts.
OpenAPI consistency, SDK generation/typecheck and 133 SDK tests passed; 10 configured
SDK process tests were skipped because `CYBER_BIN` was unset. All-target
server/snapshot/CLI Clippy with warnings denied, formatting and diff checks passed.
Strict pinned OpenSpec passed 117 items; lint reports 0 errors/25 warnings and
inventory stays 53 capabilities/1030 requirements. Sandbox deprecation and Darwin
linker warnings were observed without assigning their origin to main. Linux/Windows,
full-workspace and TUI/App recovery acceptance were not run here.

The code-review skill was applied across all changed runtime/native/HTTP/test/docs
and generated SDK files. It checks positive unstarted history, immutable native
ownership and current authority, exact paired request identity and lookup, descriptor
retention through COMMIT/reply, cancellation/Stop controls, sequence bounds, error
contracts and separation from restore/binary/generic acknowledgements. Review adds
strict receipt validation and validates the latest native event rather than
filtering past a later foreign event. Native proof verification is explicitly
restricted to synchronous metadata and cannot reenter the runtime/Store from the
writer. The repository runtime-port/native-adapter split is preserved. No critical
or major findings remain for this increment; complete early-resource recovery and
other parent acceptance gaps remain open.

The initial runtime fixture attempted to use `set_append_fault`, whose documented
scope is ordinary `append`; it does not intercept checked/paired appends. Writer-local
SQLite triggers now model the abrupt disposal and second-owner insertion cuts,
and the final suites above pass. No production Store fault semantics were changed.
This increment remains local and does not complete M2–M8, issue #93 or issue #92.


### Published-origin cancellation before source admission (2026-11-06, local)

This continuation extends the exited-preparation path to a published origin with
positive unadmitted evidence. Before its first source callback a new file-copy worker
commits native activity sequence 2 with `snapshot_source_admitted: true`, publishes
the exact origin and admitted phase in private native metadata, then rechecks
current authority. The local scope advances only after observing the Store reply;
a lost reply cannot run the callback or overwrite durable admission with disposal.
All native resources remain inside the actual worker through final filesystem work.

Review found database-only false evidence insufficient after restoration of an older
database backup. Version-2 lifetime bindings now name a native phase file, flushed
before stable lifetime publication. It can only advance to admitted, never reset.
Cancellation requires an exited exact lifetime lock, positive native false evidence,
and, for an origin, exact database false reservation/launch history. Missing,
legacy or admitted native phase refuses. Historical cancellation receipts remain
readable without acquiring new authority. The compatibility marker prevents a
previous worker-catalog binary from mutating this Store while retaining event bytes.

The paired Store cancellation retains the original activity, request, creation,
Session revision, origin and lifetime proof. Only after its observed commit does
native metadata receive `preparation.json`; the original intent remains unchanged.
The new preparation request `/acknowledge` route reconciles metadata without source
mutation, inference or transcript movement. Lookup and duplicate confirmation only
read the original durable pair. A cancelled resolution reports
`cancelled_before_effects` when it had an origin; its failed parent remains fenced.
Quota includes native phases, orphan construction files and cancellation receipts;
existing conservative GC protection remains, with reclamation still incomplete.

Regression coverage includes rejected admission and a real SQLite committed-but-lost
reply before callback dispatch, native database-backup disagreement/missing phase,
legacy lifetime refusal, cap/retry, missing/corrupt intent and later symlink owner
bytes. A real service kill observes the exact origin/false admission boundary over
1280 binary source files; cancellation/restart/retry preserves later owner edits
and transcript, then permits an ordinary prompt in the unmanaged non-Git fixture.
This real-process observation does not prove the finer before-intent cut; the native
missing-intent case is separate. Independent checkout-resource proof, Git-native
recovery, reclamation, complete clients and full M8 platform/cut coverage remain
open. Neither issue nor the whole recovery delivery is complete or merge-ready.


Local verification for this continuation: 416 server unit tests passed (one ignored
existing subprocess fixture), plus all 22 affected integrations (4 idle cancellation,
13 Location activity, 5 rewind preview). The complete snapshot suite passed 46 unit,
18 file-copy and 17 Git cases; the added legacy-lifetime regression was then included
in a passing 47-case native unit run, with its subprocess child fixture listed
ignored and exercised by its parent tests. All 16 real CLI branch/recovery tests
passed; the two early-kill cases were rerun successfully after adding explicit
acknowledgement calls before/after restart and original-intent/tombstone assertions.
Store tests passed all 59 cases, including the previous-catalog compatibility gate.
OpenAPI consistency and generated SDK checks passed; SDK typechecking and 134 tests
passed, with 10 configured process tests skipped because `CYBER_BIN` was unset.
Strict pinned OpenSpec passed 117 items; lint reports 0 errors/25 warnings and the
inventory remains 53 capabilities/1030 requirements. Formatting and diff checks
passed. Server/snapshot/CLI all-target Clippy passed with warnings denied, followed
by another passing server/snapshot check after the final port review. Native
sandbox deprecation and Darwin linker warnings were observed without
assigning an origin to main. Linux/Windows, full-workspace and TUI/App recovery
acceptance were not run here.

The code-review skill was applied to every changed runtime, port, native adapter,
Store migration, HTTP, test, documentation and generated SDK file. The runtime/native
port separation fits the repository. Review checked exact lifetime and journal
routes, private file identities/modes/aliases, positive source-admission history,
writer resource retention and lost replies, request identity/lookup, current owner
authority, encoded/quota bounds, legacy compatibility and no source/transcript replay.
It corrected database-only proof by adding native phases and made the default
adapter preparation verification refuse without an explicit implementation. New
public acknowledgement coverage reaches the actual route through the real CLI;
metadata cap/retry and missing-intent behavior also have native regression tests.
No critical or major findings remain for this continuation; complete recovery still
has the parent acceptance gaps described above.

Remote refresh confirmed main remains `db9e171e`, issues #92/#93 are open and no
PRs are open. This continuation remains local; no merge or accepted delivery is
claimed. The next ownership gap is the managed checkout acquisition/settlement cut:
its Session-owned claim must become independent exact worker evidence before an
exited preparation can safely settle that resource. No existing unknown lease may
be cleared merely because the snapshot worker exited.


### Independent checkout acquisition for snapshot workers (2026-11-06, local)

The prior code retained a checkout lease but acquired it under the Session ID.
Snapshot restoration and keep/rollback workers now use a dedicated
`claim_snapshot_location` port with their already-reserved native activity ID.
The core repository issues a distinct `op_` checkout claim; the production tools
adapter applies it to every enclosing managed checkout, and the App adapter
forwards this port. The existing retained settlement holds each native lock and
Location activity until the actual worker releases its resources. Generic Session,
binary, MCP and LSP claim selection is unchanged. A managed host without this
explicit port refuses; unmanaged defaults retain existing Location behavior.

Core regressions run real Git checkout creation/removal: Session/snapshot claims
coexist, duplicate snapshot ownership refuses, generic/binary/foreign IDs cannot
substitute, retained settlement excludes removal, interrupted acquisition stays
unknown, and snapshot settlement cannot acknowledge an unknown Session. A production
host regression acquires a snapshot alongside a live Session, retains its exact
activity-named lock, then permits removal only after proof disposal. Runtime tests
verify dispatch of the actual reserved activity and the default managed-host refusal.

This does not create acknowledgement authority from an activity name. Exact
persisted checkout file/directory identity, creation/lifetime binding, multi-checkout
acquisition proof and interrupted-claim reconciliation remain required. No unknown
Session or operation record is force-cleared. The complete managed recovery delivery
and both issues remain incomplete; no parent task is marked complete here.


Local macOS arm64 verification for this continuation: all 68 core worktree repository
cases passed, with its existing subprocess fixture listed ignored; all 418 server
unit cases passed with one ignored fixture, plus 22 affected integrations (4 idle
cancellation, 13 Location activity, 5 rewind preview). All 38 production worktree
host cases and all 16 real CLI branch/recovery cases passed, including retained
native ownership, service kills, early preparations and partial restore keep/rollback.
All-target core/server/tools/App/CLI Clippy passed with warnings denied. Formatting
and diff checks passed. Strict pinned OpenSpec passed all 117 items; lint reports
0 errors/25 warnings. No HTTP payload or generated SDK contract changed. Full-workspace,
Linux/Windows native execution and live TUI/App recovery acceptance were not run.
Observed native sandbox deprecation and Darwin linker warnings are recorded without
assigning their origin to main.

The code-review skill was applied across all changed core/runtime/host/App/test and
documentation files. Review confirms the port/adapters fit the existing architecture,
typed worker claims cannot reuse Session/binary namespaces, source/worktree binding
checks remain in the runtime, ordinary cancellation acknowledgement behavior is
shared unchanged, and the actual retained Location resource continues to outlive
filesystem effects and consuming commits. Tests cover the new public core and host
ports and refusal paths. No critical/major findings remain for independent acquisition;
immutable interrupted-claim proof and full managed recovery acceptance remain open.
This continuation remains local and does not authorize unknown claim settlement,
complete M2–M8 or make the full recovery increment merge-ready.


### Exact proof for acquired native checkout claims (2026-11-06, local)

New snapshot checkout leases retain native common/activity/creation directory pins,
the creation descriptor and the locked claim. A bounded versioned native binding
records exact identities, owner/worktree, creation digest and Unix owner/mode (or
Windows full file/volume/creation identity and attributes). Verification opens fresh
no-follow child routes, refuses aliases/replacements, compares creation bytes and
reads the exact claim using positional reads on its original locked descriptor.
Unknown record fields refuse verification. Snapshot activity claims use exclusive
creation and are never reset, including after settlement; generic Session behavior
is unchanged. Legacy/unbound claims do not acquire new proof.

The host retains a dedicated `SnapshotLocationGuard`; managed guards must implement
it explicitly. Normal workers verify it before authorization, after native completion
and at consuming commit. The same proof moves into the Store writer, is verified
there, and is returned only after the observed paired commit. It therefore remains
held through actual writer completion/reply. A failed native verification rolls back
both aggregates and prevents journal acknowledgement. The callback performs bounded
native metadata work and cannot call the primary runtime/Store or launch processes.
Unmanaged owners retain their existing behavior without inventing managed proof.

Native regressions exercise claim mode/unknown-field changes, hard links, identical
file replacement, altered/replaced creation records and replaced activity namespaces.
Runtime regression counters verify that changed proof prevents the source callback
and that writer-local rejection rolls back both owners, alongside a successful
consuming commit. Production host coverage uses the exact native guard and refuses
an introduced hard link without releasing retained ownership.

An initial new integration assertion expected legacy Git rewind to issue an owned
snapshot claim. It found no such claim: Git still uses the existing restore path.
The corrected test verifies that legacy Git restoration preserves an explicitly
retained independent native checkout proof, rather than claiming owned Git recovery.
Git journaling/owned restoration remains required and is not accepted by that test.

This continuation verifies acquired resources; it does not yet publish the complete
checkout binding through the immutable snapshot lifetime before stable acquisition,
reacquire interrupted unknown claims, or consume native checkout acknowledgements
from recovery receipts. The incomplete binding-publication cut retains evidence and
cannot be force-cleared. Full M2–M8, native platform/cut coverage, reclamation and
client acceptance remain open. No parent task or issue is complete.


Local verification: 420 server unit tests passed with one ignored subprocess fixture,
plus 22 affected integrations (4 idle cancellation, 13 Location activity and 5 rewind
preview). The reviewed core worktree repository suite passed all 70 cases with its
subprocess child listed ignored; all five selected activity unit regressions passed.
All 39 production worktree host cases passed after correcting the Git compatibility
assertion described above. All 16 real CLI branch/recovery cases passed, including
real early/partial kills and request reconciliation. These broad checks were followed
by focused core/host verification and all-target Clippy after the final core review
changes. Store tests passed all 60 cases after adding the checkout-proof compatibility
marker; the previous catalog refuses mutation and native bytes remain unchanged.
All-target core/server/tools/App/CLI Clippy passed with warnings denied. Formatting,
diff, generated SDK consistency and strict pinned OpenSpec (117 items) passed; spec
lint reports 0 errors/25 warnings. No HTTP payload changed, and SDK execution tests
were not rerun for these Rust/native-only changes. Full-workspace tests, native
Linux/Windows execution and live TUI/App recovery acceptance were not run. Native
sandbox deprecation/Darwin linker warnings were observed without attributing an
origin to main. Remote main remains `db9e171e`.

The code-review skill was applied to all changed core/native, runtime/writer, host,
Store migration, test and documentation files. The metadata port keeps the existing
runtime/native-adapter direction. Review checks descriptor and namespace retention,
creation/record bounds, exact IDs and metadata, no-follow/alias handling, positional
reads under Windows locks, one-shot exclusive publication, writer-local proof
failure rollback, returned proof lifetime, and old-writer compatibility. Verification
and proof disposal may occur in the writer and must not reenter the primary Store;
the provided native adapters only query/release native resources there. Review added
exclusive claim creation, strict unknown-field comparison and the compatibility gate.
No critical/major findings remain for acquired-claim verification. Unknown acquisition
recovery, complete immutable lifetime binding and owned Git journaling remain explicit
acceptance gaps; this local increment does not complete either issue or the full
recovery delivery.


Final review also adds a writer-local Stop check after native checkout verification;
a cancelled ordinary restore cannot consume its acknowledgement merely because its
caller-side check ran before queueing. The targeted regression injects Stop during
the final native check and verifies unchanged Session state, unknown native origin
and no journal acknowledgement. Checked owner refusals return `Conflict` while
schema, corruption and storage failures retain their distinct error paths. The
final snapshot-worker suite (34 tests) and server Clippy passed after this change.
The checkout-proof migration has a separate passing Store all-target Clippy check.


### Initialized lifetime-bound checkout publication (2026-11-06, local)

Stable checkout activity names no longer expose an initial generic record followed
by a later binding rewrite. Construction stages and flushes the complete native
record while retaining its actual file lock, directory pins and creation descriptor,
then publishes without replacement and synchronizes the directory. Linux/macOS use
native no-replace rename. A pre-publication cut leaves a private complete stage;
that activity cannot be reused while its construction evidence remains. Prefixes
include activity length to prevent overlapping IDs from colliding. Admission reserves
room for the new record within a 4096-entry inventory; legacy/unbound records remain
bounded to 4 KiB, and lifetime-bound records to 16 KiB. No construction cleanup is
enabled here.

Version-2 native claims include the exact activity, Session creation, canonical
source directory and versioned digest of the complete immutable lifetime binding.
The v1 digest framing has a fixed known-answer regression. Native constructors
validate Location/context before publication. Runtime inspects the entire returned
resource set before native settlement, rejecting foreign activity/creation/digest,
missing context, unknown versions, duplicate or incomplete resources. It checks the
same binding set after retaining ownership. This prevents a foreign resource from
being acknowledged merely to inspect it. Production CLI and App adapters implement
the bound port; unsupported managed hosts refuse rather than dropping the context.
The new compatibility marker rejects earlier writer catalogs without changing any
retained native/event bytes.

Native regressions include actual process death while a complete stage is locked,
no stable publication at that cut, no replay/reuse of construction evidence, a
foreign stable-name collision, and namespace replacement immediately before
publication. The last retains the original stage and refuses installation. Review
keeps the original directory/creation pins through construction instead of reopening
them after creating a stage, and rechecks routing after the preparation boundary.
Production-host tests exercise actual managed creation, context roundtrip and refusal
of a foreign request before namespace creation. Scope tests use an instrumented port
to prove all context failures precede native settlement. These are acquired-resource
and publication checks, not completed unknown-claim acknowledgement.

Windows retains initialized unbound legacy publication through no-replace link/unlink;
an interrupted extra link refuses verification. Lifetime-bound directory durability
is explicitly unavailable there and refuses before claim creation. This does not
claim Windows native acceptance or weaken an existing native fence. Native Windows
execution, Linux execution, complete publication/cut coverage, exact unknown-claim
reacquisition/acknowledgement, Git journals, reference-aware reclamation and full
client acceptance remain required. M2–M8 and parent tasks remain open.


Verification on local macOS arm64: all 410 core unit cases passed with two ignored
subprocess fixtures, and all 71 core worktree repository cases passed with its child
fixture listed ignored. After final pin/route review, all eight activity unit cases
and six selected native snapshot integration cases passed; the new process-kill child
is invoked by its parent test. All 422 server unit cases passed with one ignored
fixture, plus 22 affected integrations (4 idle cancellation, 13 Location activity,
5 rewind preview). The subsequent 40-case snapshot-focused run passed, including the
known-answer digest and pre-settlement context refusal cases. All 40 production
worktree host cases, all 61 Store cases and all 16 real CLI branch/recovery cases
passed. All-target core/server/Store/tools/App/CLI Clippy passed with warnings denied
after review fixes. Formatting, diff and generated SDK consistency checks passed;
strict pinned OpenSpec passed 117 items and lint reports 0 errors/25 warnings.
SDK execution tests, full-workspace tests, native Linux/Windows execution and live
TUI/App recovery acceptance were not run for this increment. Native sandbox
API deprecation and Darwin linker warnings were observed without attributing their
origin to main.

The code-review skill was applied to every changed native/core, runtime/port,
Store compatibility, CLI/App adapter, test and documentation file. Review checks
complete pre-publication records, original pin retention and no-follow routing,
no-replace/collision behavior, locked-file durability, bounded inventory and encoding,
context identity before acknowledgement, stable digest framing, prior-catalog refusal,
and absence of source replay or legacy authority promotion. It adds revalidation at
the publication boundary and a namespace-replacement regression. Initial checks found
a borrow error in the new subprocess fixture and Clippy test placement/borrow style
issues; those are corrected and the final checks above passed. No critical/major
findings remain for initialized lifetime-bound publication. Full unknown-resource
reconciliation, complete ownership/reference publication across all cuts, Git recovery,
reclamation and platform/client acceptance remain open. This local continuation does
not make the complete recovery increment merge-ready or complete either issue.


## Exact exited-claim reacquisition and durable acquired resources (2026-11-07)

Core exposes a separate read-only `SnapshotClaim`: callers supply the exact
version-2 binding, immutable lifetime context and managed creation. Reacquisition
opens existing evidence, takes the native exclusive lock and verifies retained
common/activity/creation pins, creation bytes, file identity and complete record.
It does not create or rewrite a claim and exposes no settlement API. Real managed
checkout coverage checks live-worker refusal, unchanged unknown bytes, retained
removal exclusion, foreign creation/lifetime and legacy refusal, and a byte-identical
replacement refusal while retaining both original and replacement evidence.

Ordinary and recovery runtime factories publish their verified complete acquired
binding set through the lifetime port before returning a worker. The private source
phase retains `Some([])` for a complete unmanaged acquisition and `None` for absent
constructor evidence. Publication holds the native global snapshot lock for quota
accounting, checks bounded ownership/context and duplicates, refuses changed sets
or publication after source admission, and retains the original references when
admitting source work. Managed source admission refuses absent resource evidence.
The compatibility catalog marker prevents previous Store writers from erasing the
new phase fields; its migration regression preserves retained reference bytes.

Native tests cover missing/empty, foreign/duplicate, quota failure, identical retry,
immutable replacement and post-admission reference retention. The resource DTOs in
that state-machine fixture are instrumented; actual file/lock ownership is covered
by the core integration and existing production-host tests. Runtime failure injection
checks that rejected publication returns no worker, retains an unknown reservation
with no origin/source admission, releases admission/lifetime permits and performs
no acknowledgement. Real CLI successful restore assertions inspect the actual
private source phase and require its original lifetime/origin and explicit empty
resource set in unmanaged fixtures.

The code-review skill was applied across every changed core/runtime/native/Store,
test and spec file. Review checks no-follow existing-only reacquisition, exact
creation/record comparison, retained locks on all exits, no generic settlement on
read-only proof, private bounded atomic publication under native quota ownership,
resource immutability and source ordering, and backward writer refusal. The added
runtime failure and managed native missing-set regressions close the observed test
gaps. This step enables no unknown-claim acknowledgement and does not complete
partial-constructor reconciliation, Git recovery, reference-aware reclamation,
M2–M8 or either issue. The final runtime regression found that pre-origin disposal could settle a lifetime-bound
reservation after failed resource publication. Disposal now preserves that exact
snapshot uncertainty until explicit reconciliation; generic and binary scopes keep
their existing disposal rules.


Verification for this continuation on local macOS arm64: the broad runs passed
84 snapshot cases (one ignored child fixture), 423 server unit cases (one ignored
fixture) plus 22 affected server integrations, 62 Store cases and 72 core worktree
integration cases (one ignored child fixture). After final creation validation,
the exact native core reacquisition regression passed again. All 16 real CLI
branch/recovery cases passed with the new durable resource assertions. All-target
Clippy for core/server/Store/snapshot/tools/App/CLI passed with warnings denied.
Strict pinned OpenSpec passed 117 items, spec lint reports 0 errors/25 warnings,
and generated SDK consistency, formatting and diff checks passed. Native sandbox
API deprecation warnings were observed; no unchanged-main comparison or origin
attribution is made here. Full-workspace tests, SDK execution tests, native Linux/
Windows runs and live TUI/App recovery acceptance were not run for this increment.


The final native missing-managed-resource admission regression passed. The initial
focused runtime run passed 40 cases and failed the new publication-failure case,
exposing the pre-origin disposal gap above. After the fix all 424 server unit
cases passed with one ignored fixture, and server all-target Clippy passed again.
The first test-selection command matched zero tests and was replaced by the actual
snapshot module selection; no coverage is attributed to that empty run.

The final focused disposal regression passed with explicit exact snapshot
uncertainty and no opaque Location flag. All 16 real CLI cases passed again after
the disposal fix, including both pre-origin/pre-source service kills and partial
keep/rollback restart cases. Final formatting/diff checks passed. Review found no
remaining critical/major issue within this increment; the broader recovery and
platform/client acceptance gaps above remain required before delivery.


## Consuming original acquired checkout evidence (2026-11-07)

Preparation workers now load the complete native source-phase resource set under
the original exited worker lifetime lock. The explicit production host port
reacquires the exact existing claims as read-only `SnapshotClaim` values, matching
current managed creation/context and the complete ordered persisted resource set.
It performs no Git/process launch, claim creation, generic settlement or source
mutation. The returned native proof retains original lock descriptors and metadata
pins. CLI and App implement the same host port; a host without managed support
refuses rather than replacing original evidence with a fresh claim.

A versioned SHA256 digest binds the immutable lifetime and complete acquired set.
Validation checks version, owner/context, nearest checkout presence, empty unmanaged
sets, inventory bound and duplicates. The native publication validator shares this
encoding/validation. Fixed known-answer coverage pins both lifetime and resource
encodings, including the empty complete set; changed lifetime identities change the
digest and legacy lifetimes cannot authorize it. The review fingerprint includes
the resource digest, and the paired original/request cancellation receipt stores
it. Runtime rechecks the published set and actual retained proof inside the Store
writer, returning the worker through COMMIT and observed reply. Original native
locks remain held until that worker unwinds.

Explicit journal metadata acknowledgement verifies the committed resource digest
against native publication. Native tombstone creation and loading both validate
that binding, so internal Store entry points cannot bypass it. Historical receipts
without a resource digest retain their existing journal-only authority and gain no
checkout acknowledgement capability. Lost-reply/request lookup remains database-only
and never reacquires native resources or replays source work. A compatibility catalog
marker prevents older writers from discarding consumed-resource evidence; OpenAPI
and generated TypeScript types include the optional review/receipt digest.

Regression coverage uses the actual managed production host to retain and reacquire
an unknown original claim, refuse active/foreign/incomplete evidence and preserve
claim bytes. Runtime instrumentation changes native publication after review,
verifies failed confirmation and absent receipt, restores the exact set for positive
confirmation, then changes evidence again while database lookup still succeeds and
explicit metadata reconciliation refuses. Separate changed-proof and Stop arrivals at the consuming
writer's retained-checkout verification roll back both owners; the final Stop check
follows all native proof verification. The same unchanged review succeeds after
valid proof returns under a fresh preparation owner. Native tests refuse a mismatched digest
before publishing journal metadata and preserve old journal reconciliation without
promoting its receipt. Real CLI kill/restart tests compare review/receipt resource
binding and require the exact empty-set digest after published origins.

The code-review skill is applied to all changed runtime/native/Store/adapter,
API/SDK, test and specification files. Review checks native-only writer callbacks,
original evidence rather than fresh claims, lifetime/resource ownership, bounded
encoding, immutable digest framing, held descriptors through commit/reply, both
native tombstone entry points, legacy journal compatibility and absence of native
claim acknowledgement or cleanup authority. Native unknown-claim settlement,
partial-construction resource completion, Git recovery, reclamation and complete
platform/client acceptance remain open with M2–M8 and parent tasks 2.4/3.2–3.4.


Verification on local macOS arm64: final server unit tests passed 426 cases with
one ignored fixture, including changed original proof and Stop during consuming
verification. The 43-case focused snapshot run also passed. All 85 snapshot tests
passed with one ignored subprocess fixture; after adding the native reader
corruption assertion, its focused digest/tombstone regression passed again. All
63 Store tests passed, including prior-catalog refusal and exact retained event
bytes. The selected production-host managed lifetime/reacquisition integration
passed (39 unrelated cases filtered). OpenAPI regeneration/current-document check
passed; SDK typechecking and 134 SDK tests passed, with 10 real-process SDK tests
skipped because CYBER_BIN was not set. Generated SDK consistency passed. Strict
pinned OpenSpec passed all 117 items and spec lint reports 0 errors/25 warnings.
Final all-target Clippy for server/Store/snapshot/tools/App/CLI passed with warnings
denied. An initial Clippy run found 18/15 complexity in the expanded CLI fixture;
extracting its receipt assertions into a helper resolved it without an allowance.
The initial compile's missing review field was fixed before successful validation.
Formatting and diff checks passed. Native sandbox deprecation and Darwin linker
warnings were observed without assigning their origin to main. Full-workspace
tests, native Linux/Windows execution, and live TUI/App recovery acceptance were
not run for this increment. The code/security review found no remaining critical
or major issue within this consuming-proof scope; native checkout acknowledgement
and the broader recovery acceptance obligations remain incomplete.

Final real CLI verification passed all 16 branch/recovery cases after the consuming
Stop fix and assertion-helper extraction, including pre-origin and published-origin
service kills, restart/lookup, and partial keep/rollback. Their successful receipt
assertions require review/commit resource agreement and the exact complete empty-set
digest after published origins. No additional parent acceptance is claimed.


## Native acknowledgement of exact pre-effect checkout claims (2026-11-07)

The production retained checkout proof can now publish a separate native
acknowledgement after observed durable preparation cancellation. The marker binds
the exact version-2 claim, the complete acquired-resource digest and a versioned
SHA256 digest of the entire committed receipt. The fixed known-answer encoding
refuses digest-less or non-positive pre-effect receipts. No original claim bytes
are reset, rewritten or deleted. `SnapshotClaim` still exposes read-only proof;
the separate `CheckoutAcknowledgement` adapter authority performs this specific
metadata publication and cannot operate on a different or legacy claim.

Publication retains the original claim lock and pins. A complete bounded record
with its own native file identity and construction name is staged, flushed,
published without replacement and directory-synchronized. Existing matching markers
are revalidated, retained and synchronized on retry. Replaced/aliased markers,
foreign contexts and different receipts refuse. Marker encoding is bounded to
32 KiB and shares the 4096-entry activity inventory. Native bound publication is
admitted only on Linux/macOS; unsupported durability refuses before creating
metadata. All construction evidence remains retained for the pending cleanup path.

Before paired commit the actual worker checks existing native journal cancellation
metadata and retains any existing checkout marker against the proposed receipt.
This prevents a database backup from producing a conflicting receipt even if a
prior acknowledgement stopped between journal and checkout publication. The writer
then rechecks native resources, current authority and Stop. Only the observed commit
reply permits journal/checkout publication. A native publication error keeps the
committed pair and reports reconciliation. Confirm/lookup retries remain database-
only; explicit acknowledgement reacquires and attests the same proof before retrying
metadata. Source and transcript are never replayed by those operations.

Complete resource publication now precedes checkout admission settlement. Publication
failure does not settle any acquired checkout. A cut after complete publication and
before settlement retains the positive resource set needed for exact pre-effect
claim reconciliation. Partial acquisition, incomplete publication and corrupt claim
records still refuse; this step does not invent missing provenance or complete all
constructor cuts. Normal admitted-source/Git recovery and their consuming native
checkout integration remain separate required work.

Removal checks every independent activity record under its original lock. An exact
validated native acknowledgement can discharge only its snapshot claim; legacy or
independent unknown Session/process records remain fenced even with --force. Only
acknowledgement-construction names tied to that exact acknowledged owner are pinned
as retained metadata; they cannot authorize anything alone and are not deleted.
Creation records referenced by native acknowledgement are archived by a no-replace
rename in their original creation directory during an authorized worktree removal.
Their native file identity and bytes survive removal retry/recreation. Removal-only
verification may use that exact archived record when the live route is absent;
ordinary snapshot worker reacquisition still requires the live creation route. A
new incarnation's live creation never falls back to old archived proof.

Actual core regressions exercise marker idempotence, original/independent unknown
byte preservation, native replacement and hard-link refusal, foreign/unacknowledged
stages and a changed Managed context. A real process is killed after durable stage
preparation and before publication; retry completes without rewriting the claim or
deleting the retained stage, which remains locked during later removal admission.
Namespace replacement immediately before publication retains the original stage and
installs no marker in the replacement. Real Git tests prove removal after exact
acknowledgement, independent unknown refusal, durable creation archive, repeated
removal, clean recreation and refusal to affect the new incarnation. Production-host
coverage exercises actual native claims with an explicitly instrumented lifetime/
receipt: it is adapter evidence, not a full owned-Git runtime acceptance claim.

Runtime instrumentation refuses an existing foreign native receipt before commit,
then fails publication after the commit. Lookup and confirm retries return the
committed pair without another attempt; explicit acknowledgement alone retries.
It also proves publication failure precedes any checkout admission settlement.
Native backend coverage refuses a different journal receipt before consuming commit
and preserves identical retry, resource-digest and legacy journal-only behavior.

The code-review skill is applied across every changed core/native, runtime/port,
Store compatibility, production adapter, test and documentation file. Review checks
no source/process work in metadata callbacks, no primary Store reentry, candidate
receipt checks before commit, lifetime/claim/marker lock retention, exact native
route/content checks, bounded no-replace publication, retry durability, orphan/alias
refusal, independent unknown preservation and removal/recreation compatibility.
Review identified the creation-record lifetime and publication-before-settlement
ordering gaps, both addressed with regression coverage. Partial/corrupt constructor
cuts, post-effect/owned Git integration, reference/reader/lease-aware reclamation,
complete clients and full native/platform acceptance remain required with M2–M8 and
2.4/3.2–3.4. No parent task is accepted and the full recovery increment is not yet
ready for delivery.


Verification on local macOS arm64: all 415 core unit tests passed with three ignored
tests, including the new acknowledgement subprocess fixture invoked by its parent
kill test. All 73 core worktree integration cases passed with one ignored fixture;
the native acknowledgement case includes real removal/retry/recreation. All 428
server unit tests passed with one ignored fixture after the publication-ordering
fix; the earlier focused snapshot run passed 44 cases. All 85 snapshot cases passed
with one ignored fixture, all 64 Store tests passed, and all 40 production-host
worktree cases passed. All 16 real CLI branch/recovery cases passed again after the
final runtime ordering change. All-target Clippy for core/server/tools/Store/
snapshot/App/CLI passed with warnings denied. Formatting, diff and generated SDK
consistency passed. Strict pinned OpenSpec passed all 117 items and spec lint reports
0 errors/25 warnings. SDK execution tests, full-workspace tests, native Linux/Windows
execution and live TUI/App recovery acceptance were not run for this increment.
Native sandbox deprecation and Darwin linker warnings were observed without an
unchanged-main comparison or origin attribution. An initial integration compile
used the async listing API incorrectly; it was corrected before the passing full
worktree run. No critical/major review issue remains in this pre-effect native
acknowledgement scope. The broader unfinished obligations above remain required.


## Exact Git image reads before owned-journal integration (2026-11-07)

Git restore planning previously accepted the first loosely decoded tree-listing
record without checking the returned path or blob bytes against its object ID.
The shared `Shadow::blob` boundary now asks for a literal path and decodes exactly
one NUL-terminated record, preserving tabs/newlines in names. It validates safe
relative path components, supported modes and full object IDs. A valid directory
entry remains absence of a file image, preserving the existing planner distinction;
unsupported entry kinds and malformed/ambiguous records refuse. File, executable
and symbolic-link blob images are checked against Git's type/length-framed SHA-1
or SHA-256 identity before being returned. Historical exclusion validation remains
first, and the user's index/refs/config are not changed. SHA-2 reuses the already
locked workspace 0.10.9 dependency; no dependency versions were upgraded.

Unit coverage pins independent empty-blob identities for both formats and rejects
corrupt bytes, unsafe paths, mismatched names, multiple or unterminated records,
unsupported entries and invalid object IDs. Real Git restores cover literal glob/
magic-looking names and tabs/newlines while preserving the user index. Another
real-repository test replaces a loose target blob with a different valid compressed
Git object; restore refuses and leaves both current source and user index unchanged.
The code-review skill was applied to all code, dependency/lock, test and spec changes,
checking exact byte parsing, no option/pathspec substitution, supported mode handling,
framed hashes and refusal before effects. This does not enable owned Git restore,
complete Git graph/native journal provenance or satisfy parent recovery acceptance.

Verification: the full snapshot run passed 88 cases with one ignored fixture before
the additional real corrupt-object regression. The final 19-case Git integration
run passed with that regression included. Snapshot all-target Clippy passed with
warnings denied; formatting and diff checks passed. The initial locked command
required adding the existing SHA-2 dependency to the snapshot lock entry, which was
updated without changing versions before the passing locked run. Final strict
OpenSpec validation is recorded below. Full-workspace tests, real CLI/TUI/App
acceptance and native Linux/Windows execution were not run for this image-read
increment; owned Git recovery and its complete crash/platform acceptance remain open.

Strict pinned OpenSpec passed all 117 items; spec lint reports 0 errors/25 warnings.
Generated SDK consistency passed with no API shape changes.


## Verified scoped Git catalogs (2026-11-07)

The next Git integration prerequisite replaces changed-path derivation from Git
diff output with an immutable literal-path catalog. Exclusion-tag payloads and
every eligible scoped directory tree are checked against type/length-framed Git
object identities before paths, modes and leaf IDs are used. Raw binary tree
records preserve unusual names and reject malformed modes, unsafe paths, duplicate
names and truncated IDs. Leaf bytes remain verified when materialized. This does
not assert that all unchanged blobs were read or enable the native Git journal.

Each tree read is bounded to 8 MiB; traversed graph, catalog contents and pending
path/object queue each have a 64 MiB byte bound. Traversal limits depth to 64 and
directories/catalog entries to 65,536. At most four catalogs are cached per Shadow
instance, keyed by shadow repository, Location and snapshot identity. Excess
subprocess output is rejected after killing and awaiting the child, with bounded
diagnostics. These are metadata bounds, not source effect or reclamation authority.
CLI path selectors are literal and inherited pathspec flags are removed. Location
components retain literal Unix backslashes rather than changing them to separators.

Regression coverage uses real Git repositories with different valid compressed
objects substituted for exclusion tags, root trees and nested trees. Each restore
refuses while preserving source and the user index. Literal glob-shaped scopes and
backslash names restore only their exact Location and preserve sibling owner edits.
A separate subprocess repeats this under inherited GIT_GLOB_PATHSPECS. Unit tests
cover binary SHA-1/SHA-256 tree IDs, duplicate/truncated records and metadata
overflow, including an actual Git child followed by a successful read.

The code-review skill was applied to all changed source, tests and specs. Review
checked typed object hashes, scoped traversal, byte/entry bounds, exact paths,
subprocess failure cleanup and absence of new native acknowledgement authority.
No critical or major finding remains in this catalog increment. The new catalog
module remains inside the existing Git adapter; runtime ports and schemas are
unchanged. Owned Git source/link handling and durable original Git context in native
journals remain the next integration work; all parent recovery tasks stay open.

Verification on macOS arm64: locked snapshot tests passed 94 cases with two ignored
fixtures (the pathspec subprocess fixture is invoked by its parent). All-target
snapshot Clippy passed with warnings denied. An initial collapsible-if Clippy
finding was resolved before the passing final run. Full-workspace tests, SDK
execution tests, real CLI/TUI/App recovery and Linux/Windows execution were not run
for this increment; no broader acceptance is inferred.

Final review found and removed a second tag resolution between verification and
root traversal. Root identity and exclusions now come from the same verified tag
bytes; legacy bare trees retain their exact supplied identity. This closes the
substitution window rather than trusting a subsequent rev-parse result. Final
formatting, generated SDK consistency and diff checks passed. Strict pinned
OpenSpec passed 117 items; spec lint reports 0 errors/25 warnings.


## Git image import into the native journal (2026-11-07)

`git/prepared.rs` now constructs a bounded image packet from verified private Git
objects before the synchronous source callback. It retains original target, recorded
and expected IDs, leaf IDs/modes/bytes, exact worktree/Location prefix, allowed paths
and the union of historical exclusions. Paths become exact Location-relative names
for the native engine. Per-file output and combined packet bytes are bounded; each
tree has at most 4,096 images. No source reads, merge subprocesses or Git index
writes occur in this object preparation. Native import verifies the handed-off leaf
hashes again and revalidates every known expected path before source replacement.

`copy/git.rs` imports regular files through the existing CAS, conflict merge, native
source/stage/parent ownership and durable journal executor. Version-4 intents retain
a distinct exact Git context with the original IDs; no copy IDs are manufactured.
Git contexts reject wrong versions, mixed/invalid identities and foreign Locations.
The complete-record ownership requirement applies to version 4 as well as version 3.
Legacy copy contexts omit the new optional field, preserving their encoded identity.
The owned adapter has the preparation/worker dispatch, but its ordinary Git
availability and lifetime reservation gates remain disabled. There is no new
production admission claim for this partial integration.

Nine added tests cover real Git image capture followed by native regular-file
creation/deletion/restoration and executable modes, retained displaced preimages,
user index preservation, owner-edit merging, stale handoff refusal, exact glob-shaped
subdirectory scope, historical ignored-file protection, corrupt handoff bytes,
packet byte bounds and legacy context compatibility. A positive test removes the
shadow repository after object preparation; native execution still succeeds without
Git access. Native fixture keep/rollback reuses those actual preimages, retains Git
context in its independent resolution intent and returns the same request receipt
after a later owner edit without replay. Fixture origins are synthetic and provide
no runtime acknowledgement authority or process-lifetime evidence. A separate
negative test refuses symlink images before source or journal effects; that refusal
is explicitly not symlink compatibility acceptance.

The next work is symbolic-link image/native ownership support, complete expected
state scanning (including new paths and Git ignore semantics), actual Git worker
lifetime/reservation and runtime acknowledgement integration, then process-kill
acceptance. Current native import does not yet handle file/directory transitions or
literal Unix backslashes through the copy engine. These requirements prevent
enabling the Git gate. Full recovery cleanup, clients and platform acceptance also
remain open. No parent task is accepted by the regular-file fixtures.

Code review covered all changed adapter, codec, native import, tests and spec files.
The existing snapshot adapter/native engine split is preserved; no unrelated
architecture was introduced. Review required typed hash revalidation at import,
version-4 context/ownership refusal and enforcement of the availability gate inside
the direct owned port as well as its caller. These corrections are included. An
additional exact reviewed-path test restores only one file and refuses a similar
foreign scope prefix. No unresolved critical/major finding remains for this
disabled regular-file bridge; the activation gaps above remain mandatory.

Verification on local macOS arm64: locked snapshot tests passed 103 cases, with
two ignored subprocess fixtures (the pathspec fixture is invoked by its parent).
All-target snapshot Clippy passed with warnings denied after the import and direct
gate changes. Formatting, generated SDK consistency and diff checks passed. Strict
pinned OpenSpec passed all 117 items; spec lint reports 0 errors/25 warnings. Full
workspace tests, SDK execution, real CLI/TUI/App acceptance, native process-kill
Git acceptance and Linux/Windows execution were not run for this increment. The
backend fixture keep/rollback tests are not runtime lifecycle or crash evidence.


## Native symbolic-link images and retained preimages (2026-11-07)

`copy/links.rs` now pins symbolic-link inodes directly: macOS uses O_SYMLINK and
Linux uses O_PATH/O_NOFOLLOW. It checks unique inode identity, generation, bounded
target bytes (4,096 bytes), ownership/mode and supported metadata without following
the target. macOS reuses a new additive plain-link metadata helper in core; existing
binary regular-file behavior is unchanged. Linux's O_PATH descriptor cannot read
xattrs directly, so metadata-only llistxattr uses the kernel proc descriptor route
for the pinned parent, checks its identity, and checks parent/link generations
around inspection. Missing proc routing or unsupported metadata refuses. Linux
execution of this path remains unverified locally.

Native images distinguish file content from link target bytes. Existing file
entries omit the new field, and legacy journals omit the backend field; their
original serialization and immutable digest framing remain intact. Version-5
intents bind an explicit copy/Git backend and typed images in a new digest frame.
Version-4 Git contexts remain readable with their original frame. Older journals
cannot acquire link ownership merely by adding a kind field. Non-Git capture
continues to report links as unsupported and legacy copy manifests remain file-only.

The existing journal executor stages typed link entries, preserves displaced
inodes, verifies prepared source/stage identity, exchanges without following links
and revalidates the outcome. Recovery reads exact retained target bytes under
native observations, stages independent rollback entries and preserves unrelated
target file contents. Three-way text merging is restricted to regular files. New
Git-desired links are checked before staging, before publication and at completion/
consumption against the worktree and protected/private roots. Resolution rollback
may reinstate an exact owned preimage without following its target.

Review found and fixed an escape when lexical normalization cancelled an alias
before `..`; physical checks now resolve the original component order. Positives
cover link-to-link, file-to-link, link-to-file, create/delete and safe dangling
targets with rollback. Negatives cover escaping/protected aliases, same-target
foreign source and stage inodes, hardlinked links, bounds and link xattrs. Five
actual child-process kills at parent/replace/delete/create cuts reopen the private
store under a new owner, classify real typed inode slots without progress records,
retain preimages and keep the recovery fence. These native fixtures do not confer
runtime origin/acknowledgement authority and do not prove all Git service cuts.

Owner API images now carry `kind: file | symlink`; omitted legacy fields default
to file. OpenAPI and the TypeScript SDK are regenerated. The code-review skill
covered all changed core/adapter/codec/API/test/artifact files, including no-follow
semantics, old digest compatibility, exact private slots, quota accounting and
source-target distinction. The additive core helper does not change binary editor
contracts. No critical/major issue remains in this disabled bridge scope.

The Git availability/reservation gates remain disabled. Complete expected-state
scanning, file/directory transitions, literal backslash paths in the copy engine,
complex link-target compatibility, actual Git worker lifetime/receipt integration,
reference-aware reclamation, full clients and native platform/service-kill matrix
remain required. No parent task is accepted by this increment.

Verification on local macOS arm64: the final snapshot suite passed 113 cases with
two ignored fixtures (the pathspec fixture is invoked by its parent), including the
five real link kill/reopen cuts. The server unit suite passed 429 cases with one
ignored fixture; the final file-wire compatibility refinement also passed its
focused server regression. All five core binary-native regressions passed.
All-target Clippy for snapshot/server/core passed with warnings denied. OpenAPI
export and current-source consistency passed after refreshing the schema annotation
changed by omitting ordinary file kinds. SDK typecheck passed; SDK tests passed
134 cases with 10 configured real-server skips because CYBER_BIN was unset. The
TypeScript output stayed unchanged through that schema-annotation refresh.
Formatting, generated SDK and diff checks passed; strict pinned OpenSpec passed all
117 items and spec lint reports 0 errors/25 warnings. An initial test-only ImageKind
namespace compile error was corrected before the passing full runs. The ordinary
file-image API encoding remains byte-compatible by omitting its default kind,
while symlink images serialize the explicit kind.

Full-workspace tests, real CLI/TUI/App Git recovery, native Linux/Windows execution
and the full service/writer/acknowledgement/reclamation crash matrix were not run
for this increment. The Linux proc metadata path is implemented but not claimed
verified. Generic file-copy capture still reports links as unsupported. The complete
Git recovery workflow and parent acceptance tasks remain open.

Additional target-node checks compare protected reserved siblings and configured
private ancestors by device/inode identity. Tests cover actual case aliases when
the filesystem supports them, including a direct node-check test independent of
canonical spelling. The native macOS resolver already normalizes the tested casing;
an initial Python-only probe was not equivalent and is not claimed as a native
resolver defect. One test initially expected only the identity-check error text; it
was corrected to accept the earlier protected-path refusal as well. The final full
snapshot run and three-package all-target Clippy pass include these checks.


## Native Git namespace and rule-input validation (2026-11-07)

`copy/inventory.rs` now scans the eligible Location namespace before source content
planning. It compares all eligible leaf names with the expected image set, so new
files, nested files and links cannot evade validation merely because they were
absent from every prepared tree. Unknown leaf kinds are not opened and source
directory links are not followed. Empty directories do not become Git file images.
Known expected exclusion paths are checked for disappearance and directory/leaf
type changes. Traversal is bounded by 65,536 nodes, depth 64 and 8 MiB of path/mark
metadata. Directory generations are checked during scanning and again after
content planning, before creating the intent. These marks are transient race
checks, not native source/recovery ownership.

`copy/git_rules.rs` uses bounded read-only global/info/ancestor/nested Git ignore
inputs, including negation precedence and Git's case setting. The trusted adapter
resolves configuration locators before dispatch; source and configuration reads
remain in the synchronous filesystem worker, with no Git process inside it. Each
input is limited to 1 MiB, combined inputs/path metadata to 8 MiB and bindings to
65,536. Input hashes are revalidated at the end of planning. Source rule traversal
uses retained worktree descriptors and no-follow descendant opens; external Git
configuration files retain their existing read-only Git semantics. Their bytes are
used only for matching, never source mutation authority or model instructions.
Pattern errors use generic diagnostics rather than echoing private patterns.

Testing with actual Git established that `ls-files --ignored --directory` can
compact an all-ignored directory and omit ignored children in a mixed directory.
Recorded exclusion paths alone therefore cannot classify current eligibility. The
new scan uses Git rules instead of assuming every unrecorded file is eligible or
blindly treating a compacted historical directory as permanently opaque.

Prepared target/recorded images still apply the complete historical exclusion
union. Expected images now use only their own exclusions, allowing stale-state
checks for a currently eligible owner file that was excluded historically while
keeping it out of the mutation plan. Shared native path validation now accepts
literal Unix backslashes and explicitly rejects empty/dot/traversal components.
Git Location/file-name and actual non-Git capture/restore regressions exercise this.

Nine additional tests cover new leaf/nested/link refusal, mixed ignored directories,
empty directory preservation, namespace/rule changes after inventory, historical
exclusion preservation versus current-state validation, subdirectory ancestor and
negation rules, global/info inputs and rule-only changes, known exclusion deletion/
type changes, and literal backslashes. All source refusals retain current files and
create no intent. The existing 113 snapshot tests remain included.

The code-review skill was applied to all source, test and spec changes, focusing on
no-follow input traversal, bounded parsing/inventory, readonly configuration roles,
separate mutation/state exclusions and preservation of the admission gate. No
critical/major issue remains in this disabled inventory scope. It does not establish
a full current-tag proof: version-1 tags lack capture-bound rule/raw-state data,
and ignored-inventory identity can differ from eligible-graph identity. Attributes/
filters, gitlinks, file/directory transitions, capture-bound configuration, actual
Git lifetime and runtime acknowledgement integration remain required. The ordinary
Git availability/reservation gates remain disabled and parent tasks remain open.

Verification on local macOS arm64: locked snapshot tests passed 122 cases with two
ignored fixtures; the pathspec fixture is invoked by its parent. Snapshot all-target
Clippy passed with warnings denied after handling the ignore builder's fallible
case-policy setter. The first scan-only build's unused import was corrected. Full
workspace tests, SDK execution, real CLI/TUI/App acceptance and Linux/Windows
execution were not run for this increment. Final format/diff, SDK-generation and
OpenSpec validation are recorded below. No API/schema shape changed here.

Final formatting/diff checks and generated SDK consistency passed. Strict pinned
OpenSpec passed all 117 items; spec lint reports 0 errors/25 warnings. The real-Git
compaction probe and current tests are local evidence only; no CI or complete Git
recovery admission is claimed.


## Capture-bound Git policy fingerprints (2026-11-07)

Version-2 immutable Git tags now bind a BLAKE3 policy fingerprint collected before
and after Git tree capture. The fingerprint includes application ignore/size
settings, private roots, effective global ignore locator and case policy, bounded
rule input hashes, and canonical ignored names. Missing source rule files are
excluded from the persistent digest so empty-directory creation does not change
Git file identity, but their absence is still checked during each transient scan.
Unsupported initial collection retains a legacy capture with a generic unavailable
proof diagnostic; a changed or failed second collection refuses publication.
Legacy version-1 tags and bare trees remain readable without policy proof. Native
import refuses an expected checkpoint lacking proof before recovery/journal access.

Five new native importer regressions cover ignored-name changes in mixed dirs,
ignored rule-file edits, absent legacy proof, external content/locator/case changes,
and application ignore/size setting changes. One history regression exercises v2,
v1 and bare-tree reading, invalid writer fingerprints and forged v1 policy fields.
The native object independence fixture now removes only the Git object directory:
removing all shadow metadata correctly invalidates its bound info/exclude input.

The code-review skill was applied to every changed source/test file and artifacts.
Review covered cfg boundaries, tag framing and typed object verification, bounded
no-follow traversal, deterministic fingerprints, configuration versus effect
ownership, unsupported fallback, and absence of an admission-gate bypass. No
critical/major finding remains for this disabled policy-validation increment.
The effective-setting resolver's source configuration files are not bound across
resolution/worker dispatch. Raw file bytes, attributes/filters, gitlinks and complete
exclusion parity are not proven by this fingerprint. Existing Git capture may run
configured clean filters. This work does not establish process isolation, Git
worker lifetime, post-effect acknowledgement or reclamation authority.

Local macOS arm64 verification: locked snapshot tests passed 128 cases, with two
ignored fixtures (the pathspec fixture is invoked by its parent). Snapshot
all-target Clippy passed with warnings denied. An earlier run exposed the obsolete
whole-shadow-removal expectation, corrected above; an earlier Clippy run found a
collapsible conditional, corrected before the passing run. No full workspace,
Linux/Windows, real CLI/TUI/App or source-bound CI acceptance was run for this
increment. Ordinary Git availability/reservation gates stay disabled and all
parent recovery tasks remain open.

Final format/diff checks and SDK generation consistency passed. Pinned strict
OpenSpec validation passed all 117 items; spec lint reported 0 errors/25 warnings.
These results do not provide source-bound CI or complete recovery acceptance.


## Exact Git working-tree representation graphs (2026-11-07)

Supported captures now collect exact eligible file bytes, executable classification
and symbolic-link target bytes before and after canonical Git capture. Source
reads use retained directories, no-follow unique regular-file opens, plain metadata
checks and source generation/identity revalidation; links reuse the pinned-inode
reader without following targets. Each collection is bounded by 4,096 files,
64 MiB per regular file and 200 MiB combined payload, plus existing namespace/rule
bounds. Before/after collections coexist for comparison; this is not a 200 MiB
process-memory claim or a completed retention cap. Changed successful collections
refuse publication; an unsupported initial collection returns a legacy checkpoint
with explicit unavailable policy/raw-state proof.

Raw bytes are written with `hash-object --no-filters`, then literal NUL-delimited
`mktree` records build a separate worktree-relative graph. A version-3 tag points
to an envelope with exactly two typed tree links: canonical and raw. The reader
verifies the tag, envelope hash, exact links and each traversed raw directory/blob
hash. Thus ordinary Git GC retains both graphs through the existing immutable
checkpoint ref rather than relying on a raw tree ID embedded only in JSON.
Native preparation now requires raw proof for target/recorded/expected and retains
original checkpoint IDs in journals. It never promotes a bare/v1/v2 canonical
blob into raw proof. Earlier records remain readable through the public Git API.

Public diff and projected preview-index construction explicitly resolve the
canonical tree. Existing blob/change/exclusion behavior stays canonical. The first
full suite exposed envelope use in diff and preview construction; those operations
were corrected, and corruption fixtures now deliberately resolve canonical graph
paths while still testing envelope/tag corruption. Fixtures do not weaken hash or
source/index-preservation assertions.

Five new native/real-Git regressions cover CRLF restoration plus aggressive GC,
normalization-equivalent later owner-byte refusal, UTF-16LE representation with
SHA-256 shadow graphs, corrupt raw blob refusal, and a deliberately configured
fixture filter that changes only its temp repository during capture. The last
case verifies no new checkpoint ref is published after changed raw state. History
coverage includes v1/v2/v3 and bare reading, policy-only native refusal and a valid
forged tag whose canonical root cannot substitute for the envelope. Existing
scoped/literal paths, link images, merge/recovery, exclusions and Git source/index
preservation tests remain included.

The SHA-256 user-repository experiment revealed public libgit2 worktree discovery
returns no root, selecting the non-Git backend. This increment does not fix that
compatibility gap. The passing graph-width fixture explicitly creates a SHA-256
shadow over an ordinarily discovered worktree; it proves the raw graph codec, not
public SHA-256 repository discovery. Its initial failed discovery expectation is
retained here rather than characterized as accepted support. A test compile failure
from calling a private helper was fixed by invoking fixture-owned Git explicitly.

The code-review skill was applied to all changed source/tests/artifacts. Review
checked route/type/identity validation, unknown leaf nonblocking opens, file/link
payload bounds, no-filter object writes, graph reachability, typed graph hashes,
exact envelope framing, legacy rejection and the public canonical compatibility
paths. No critical/major issue remains in this disabled raw materialization scope.
Still required: preview/path-selection parity for normalization-only changes,
configuration-resolution races and complete attribute/filter behavior, public
SHA-256 discovery, gitlinks, file/directory transitions, remaining link/large-file
compatibility, actual Git worker lifetime and durable acknowledgement, reference-
aware reclamation, performance and real client/platform acceptance. Canonical
capture still executes configured Git filters; this is not process isolation.
Ordinary Git availability/reservation remains disabled; parent tasks stay open.

Final local macOS arm64 verification: locked snapshot tests passed 133 cases with
two ignored fixtures (the pathspec fixture is invoked by its parent). Snapshot
all-target locked Clippy passed with warnings denied. Format/diff and generated
SDK consistency passed. Pinned strict OpenSpec validation passed all 117 items;
spec lint reported 0 errors/25 warnings. Full workspace/SDK execution, Linux,
Windows, real CLI/TUI/App and source-bound CI acceptance were not run here.


## Native Git preview and raw path selection (2026-11-07)

`Store::preview_git` now calls the same native raw-image planner as restore, with
full expected namespace/policy/image validation, historical mutation exclusions,
selected paths, merge/conflict behavior and pre-effect directory checks. Rendering
uses the verified expected packet for before images: inspected Git source bytes
are not necessarily in the native CAS. This was caught during review and corrected
without reopening source or following links. Preview may populate private CAS
objects but publishes no journal or source effect. Git file paths and unified diff
headers are worktree-relative, including subdirectory Locations.

The GitSnapshots preview and selected-preview ports use this implementation only
when ordinary owned Git is admitted. `changed` similarly selects verified raw
catalogs behind that same gate, retaining normalization-only path differences.
The actual gate remains closed for Git; legacy public behavior remains unchanged.
An async adapter retains the worktree mutation lock across bounded immutable
preparation and the synchronous native preview callback. No Git subprocess runs
inside that callback. No preview-only activation or worker authority is added.

Two backend/adapter fixtures exercise normalization-only CRLF differences, raw
changed-path selection versus canonical compatibility, preview/apply path parity,
source/journal preservation, scoped selected paths/patch headers and stale owner
edits outside selection. The fixtures explicitly verify ordinary Git admission
remains disabled. These are not real CLI/TUI/App or full runtime acceptance.

The code-review skill was applied to the complete source/test/artifact diff,
including expected-packet rendering, source versus private CAS effects, scoped
path translation, lock retention, full-state versus selected-mutation checks and
the unchanged admission gate. No critical/major finding remains in this disabled
preview scope. Raw summary/client parity, complete worker lifetime/checkout
acknowledgement, reclamation, Git compatibility and platform/crash acceptance still
block ordinary admission; all parent tasks remain open.

Final local macOS arm64 verification: locked snapshot tests passed 135 cases with
two ignored fixtures (the pathspec fixture is invoked by its parent). Snapshot
all-target locked Clippy passed with warnings denied. Format/diff and generated
SDK consistency passed. Pinned strict OpenSpec passed all 117 items; spec lint
reported 0 errors/25 warnings. An initial fixture compile error assumed the request
was Clone; the test now constructs independent identical requests without changing
the production API. Full workspace/SDK execution, real CLI/TUI/App, Linux/Windows
and source-bound CI acceptance were not run for this increment.


## Git lifetime metadata reservation (2026-11-07)

The public native lifetime port no longer refuses Git Locations before reaching
the backend-independent version-2 lifetime engine. Reservations still canonicalize
and protect the Location, bind exact activity/Session creation/worktree ownership,
publish complete private native records and retain the actual file lock. The
ordinary owned Git availability check and restore-owned direct-call gate remain
unchanged and closed. Metadata reservation does not create an intent or source
admission, acknowledge a journal or native checkout, or authorize reclamation.

Two real-Git port regressions verify exact retained lifetime ownership, live
reserve/reacquire refusal, exited reacquisition, durable empty complete checkout
sets for an unmanaged root, foreign Session-creation refusal, scoped Locations and
refusal to bind an empty set for a requested managed worktree. They preserve owner
files, assert no journal was published, and explicitly check source availability
remains false. These port tests do not create an actual managed checkout, runtime
Session/source callback or consuming Store receipt; full Git runtime integration
and post-effect native checkout acknowledgement remain required.

The code-review skill was applied to the source/test/spec diff, focusing on
reservation versus source authority, canonical/private Location validation,
unchanged native owner checks and separate admission gates. No critical/major
finding remains in this metadata-only reservation scope. All parent recovery
and acceptance tasks remain open.

Local macOS arm64 verification: locked snapshot tests passed 137 cases with two
ignored fixtures (the pathspec fixture is invoked by its parent). Snapshot
all-target locked Clippy passed with warnings denied. Format/diff and generated
SDK consistency passed. Pinned strict OpenSpec passed all 117 items; spec lint
reported 0 errors/25 warnings. Full workspace/SDK execution, actual Git runtime
source/receipt and CLI/TUI/App acceptance, Linux/Windows and source-bound CI were
not run for this increment.


## Ordinary restore checkout evidence in consuming receipts (2026-11-07)

New ordinary restore receipts optionally carry a version-1 checkout section with
the exact version-2 native lifetime and complete retained checkout-set digest. The
section is captured from the worker's original lifetime and native Location proof,
with exact activity/Session creation/worktree/Location/root/namespace checks.
The completion first verifies the retained lifetime and its original published
checkout set. The paired writer reads the retained resource set again before
admitting the Session/native append. Lost, changed or incomplete proof refuses that pair before
native acknowledgement. Lookup compares the receipt's lifetime with attested
original native history, alongside existing exact journal and Session-batch proof.

A versioned SHA-256 acknowledgement frame covers the complete receipt, validates
its checkout owner and basic batch/digest bounds, and refuses historical receipts
without checkout evidence. The optional field is omitted for legacy receipts;
reading or hashing older receipts never invents resource proof. Existing journal
reconciliation behavior is unchanged: this increment does not publish post-effect
checkout acknowledgements or authorize cleanup. Native lifetime/checkout pins and
consuming receipts still need full actual-Git runtime and reconciliation acceptance.

A new runtime test uses instrumented lifetime/Location guards to commit and look up
the exact checkout evidence, to lose the original resource publication before
commit, and to lose the complete resource proof between initial capture and the
paired writer. The refusal leaves Session state unchanged, records
unknown native outcome, performs no native acknowledgement and publishes no restore
receipt. Positive assertions cover original lifetime/digest identity, invalid batch
or hash frames, foreign Session creation, changed resource digest and legacy wire
omission/no hash authority. These guards are instrumented; no real managed checkout
or actual Git source callback is claimed by this test. An existing fixture namespace
was aligned with the native origin so the new exact namespace comparison remains
meaningful. The backend's manually constructed legacy receipt explicitly uses None.

The code-review skill was applied to source, tests, schema artifacts and spec deltas,
focusing on additive historical encoding, complete-set capture versus writer
revalidation, original native-history attestation, no source/Store reentry from
native verification, bounded hash frames and preservation of ordinary admission
and unknown fences. This receipt evidence is not complete native acknowledgement
acceptance. All parent recovery and delivery tasks remain open.

Final local macOS arm64 verification: locked server library tests passed 430 cases
with one ignored fixture; locked snapshot tests passed 137 cases with two ignored
fixtures (the pathspec fixture is invoked by its parent). Generated OpenAPI was
regenerated and its current-document test passed. SDK tests passed 134 cases with
10 CYBER_BIN-dependent end-to-end skips. Three-package all-target locked Clippy
(server/snapshot/tools) passed with Rust warnings denied; native C compilation
reported sandbox_init/sandbox_free_error deprecation warnings. Their origin was
not compared with main, and no pre-existing attribution is made. An earlier
Clippy run caught the enlarged acknowledgement enum; boxing the optional checkout
section corrected its in-memory size while preserving JSON shape. Final format/
diff and SDK generation consistency passed. Strict pinned OpenSpec passed all
117 items; spec lint reported 0 errors/25 warnings. Real Git source/managed-checkout
publication, CLI/TUI/App, Linux/Windows, full workspace and source-bound CI
acceptance were not run here. No post-effect native checkout marker is published
by this increment and no parent recovery task is accepted.


## Ordinary native checkout publication and reconciliation (2026-11-07)

A consuming ordinary restore now verifies its candidate receipt through the retained
Location guard inside the paired writer, before any acknowledgement metadata is
created. After the durable pair is observed and Session state applied, the original
claims publish idempotent markers bound to that receipt's complete-resource digest
and versioned checksum. Candidate failure refuses the pair; publication failure
reports committed runtime state and leaves exact original evidence for explicit
metadata reconciliation. Journal acknowledgement follows successful checkout
publication in the ordinary completion path. Existing foreign markers refuse.

The actual host now consumes owned CheckoutLease values into descriptor-retaining
SnapshotClaim proofs. Version-2 acquisition no longer calls generic settlement:
original UseRecord bytes remain unsettled, so worker exit without a consuming marker
still blocks native removal. Version-1 acquisition retains its former settled
encoding without gaining new marker authority. Core conversion performs no writes
and exposes no generic settlement method. Live and reacquired claims share the same
native verification/publication engine; the live activity wrapper delegates the new
receipt callbacks while retaining its LSP ownership.

Explicit reconciliation of a new receipt locks its original lifetime, reads its
published complete set, reacquires exact native claims and checks existing markers.
The acknowledgement worker verifies native resources and durable authority before
journal metadata work and again before checkout publication. No new Session claim
is substituted for the original resources. Original claims are released before the
original lifetime lock; admission/Stop completion follow resource release. Legacy
receipt and recovery-receipt metadata paths retain their existing behavior.

A journal-less managed completion cannot yet produce a positive consuming checkout
receipt. Committing it after preserving unsettled claims would leave an unresolvable
settled owner, so this path refuses before the pair and retains pre-effect/unknown
evidence instead. This is an interim refusal, not accepted managed no-op behavior;
a positive no-effect protocol remains required before ordinary Git admission.
Recovery-resolution checkout receipts/publication are also still incomplete.

Native core coverage adds an owned-claim/removal regression with and without a
matching marker, asserting original bytes stay identical. The actual host test now
checks unsettled acquisition bytes, read-only candidate verification, ordinary marker
publication/retry, foreign receipt and legacy refusal, plus existing pre-effect
acknowledgement coverage. Its lifetime/receipt inputs are adapter fixtures; it is not
a real consuming Store/runtime/source proof. Instrumented runtime tests exercise
candidate refusal, committed publication failure, successful ordering, repeated
explicit reconciliation without source replay, missing original publication refusal,
and journal-less managed refusal versus unchanged unmanaged no-effect behavior.
Actual Git backend source callbacks and managed no-op positives remain unverified.

The code-review skill was applied to every source/test/artifact change, focusing on
preserved unknown/removal fences, read-only writer callbacks, original ownership
versus generic settlement, durable-commit publication ordering, partial/idempotent
markers, legacy limits, cancellation/resource disposal, and metadata-only original
resource reacquisition. Admission, cleanup and full parent acceptance remain gated.

Final local macOS arm64 verification: locked core worktree-repository tests passed
74 cases with one ignored fixture, actual host worktree-setup tests passed all 40,
and server library tests passed 432 cases with one ignored fixture. Four-package
all-target locked Clippy (core/server/snapshot/tools) passed with Rust warnings
denied. Native C compilation emitted sandbox_init/sandbox_free_error deprecation
warnings; no main comparison or pre-existing attribution was made. Format/diff and
generated SDK consistency passed. Strict pinned OpenSpec passed all 117 items;
spec lint reported 0 errors/25 warnings. Full snapshot/SDK/workspace execution,
real CLI/TUI/App, actual Git backend consuming callbacks, Linux/Windows and
source-bound CI acceptance were not run for this increment. No-op success,
recovery-resolution checkout acknowledgements and reference-aware cleanup remain
open, as do all parent recovery acceptance tasks.


## Real owned no-op journals and distinct native namespaces (2026-11-07)

Both native restore adapters now publish a real empty journal when the revalidated
plan is empty and a worker is present. They use the normal bounded intent budget,
worker begin/origin, empty prepared ownership, durable completion and immutable
receipt path. The source-effect loop is empty, so neither database nor native source
admission occurs. No fake origin or journal identity is introduced. Ordinary
consuming receipts and checkout markers can therefore settle an owned no-op; the
writer still refuses a managed adapter that supplies no journal receipt. Legacy
unowned no-op calls still return without creating a journal. Git source admission
remains gated independently of this backend change.

Real client verification exposed a receipt-validation mistake in the recent
checkout evidence work: lifetime.namespace_identity identifies the private Location
namespace, whereas origin.binding.namespace_identity identifies its journals
collection. Equating them caused actual owned copy restores to fail with 'Restore
receipt checkout ownership differs'. The existing real CLI code-rewind test failed
on unchanged local 08bc1d9e with that exact error before this fix. Earlier mock
fixtures used equal strings and missed the distinct native directories. This is
our local receipt regression, not a claim about main's result.

Receipt validation now retains exact owner/Session creation/worktree/Location/root
checks while allowing the two independently verified namespaces to differ. Native
lifetime and journal guards continue validating their own paths, identities and
records; lookup still attests the original lifetime and receipt, and native checkout
resource digests still bind that lifetime. The runtime lifetime fixture deliberately
uses a different namespace and asserts the receipt retains it.

A new real CLI/service/scripted-provider test puts source bytes at the rewind target
before preview, then commits conversation rewind through an empty owned journal.
It verifies retained origin/receipt, empty changes and ownership, false database and
native source admission, no native admitted origin, unchanged owner files and no
inference. It then resets acknowledgement metadata and twice reconciles the original
receipt after introducing a later owner symlink, verifying no source replay. Shared
real-client assertions now compare native source phase with durable activity
admission, allowing a real unadmitted empty intent without weakening normal effect
checks; they also assert distinct namespace identities in actual receipts.

The code-review skill was applied to source/tests/artifacts, focusing on real versus
synthetic intent evidence, zero source admission, metadata budgeting, shared guard
completion and acknowledgement, separate namespace semantics, foreign proof
rejection and unchanged legacy/admission behavior. The real positive is file-copy
with an unmanaged Location; actual managed Git source/checkout no-op integration,
recovery-resolution resource acknowledgement, retention and complete client/
platform/cut acceptance remain open. No parent recovery task is accepted here.

Final local macOS arm64 verification: all 17 real CLI branch/recovery tests passed,
including actual partial-restore kill/restart keep/rollback, pre-origin/pre-source
cuts, restage/clear, disabled snapshots and the new no-op case. These remain local
unmanaged/file-copy and legacy-Git evidence, not owned managed-Git acceptance.
Locked snapshot tests passed 137 cases with two ignored fixtures (the pathspec
fixture is invoked by its parent); server library tests passed 432 cases with one
ignored fixture. The focused distinct-namespace receipt test also passed after
its explicit assertion was added. Three-package all-target locked Clippy
(server/snapshot/CLI) passed with Rust warnings denied. Native C build emitted two
sandbox API deprecation warnings; no main comparison or pre-existing attribution
was made. Clippy initially reported cognitive complexity 17 against limit 15 for
the expanded receipt assertion helper. The cognitive-complexity skill's extraction
workflow was applied using Rust's native Clippy analyzer (Rust is outside that
skill's dispatcher); splitting receipt binding assertions removed the violation.
The four affected real CLI cases passed again after this helper refactor.

Final format/diff and generated SDK consistency passed. Pinned strict OpenSpec
passed all 117 items; spec lint reported 0 errors/25 warnings. Full workspace/SDK
execution, real TUI/App, owned managed-Git source/checkout callbacks, Linux/Windows
and source-bound CI were not run for this increment. Native recovery-resolution
resource acknowledgement and reference-aware cleanup remain required; parent
tasks and ordinary Git admission stay open/gated.


## Retaining interrupted-owner resources through resolution (2026-11-07)

Recovery now reads the attested original activity's lifetime before creating the
resolution lifetime or publishing its owner. For version 2 it locks the exact
original lifetime, requires its complete published resource set, reacquires those
native claims read-only and verifies owner/Session creation/Location/root context.
The original lifetime and claims remain in the synchronous worker alongside the
distinct resolution owner's resources. Source authorization and recovery completion
revalidate original publication and retained bindings. No original claim is created,
settled or rewritten by acquisition. Version-1/missing historical lifetime evidence
stays distinct without new adoption; unsupported lifetime versions refuse.

SnapshotCompletion now disposes its native result guard before its worker. This
keeps native guard cleanup under the existing lifetime locks rather than releasing
the worker proof first. Original claims drop before their original lifetime; the
resolution lifetime remains the worker's final ownership resource. Retention alone
commits no receipt and acknowledges no native fence.

Two runtime regressions use instrumented lifetime/Location ports. One holds both
owners, checks distinct activity identity and verifies that changed original
publication prevents callback dispatch; dropping an unconsumed completion leaves
the original activity unknown. Another native result Drop guard asserts the worker
lifetime is still retained during its cleanup. The unstarted-resolution fixture
now provides two semaphore permits for the two independent owners; native storage
uses distinct per-activity lock files. These tests do not claim actual managed
checkout resource-set or native cross-process proof by themselves.

The code-review skill was applied to source/tests/artifacts, focusing on acquisition
before dispatch, no retrospective legacy adoption, unsupported-version refusal,
read-only resource proof, lifetime/claim/native guard disposal order, cancellation
and preservation of unknown evidence. Full recovery receipts still need both
resource sets bound and consumed at the writer, with exact native marker publication
and lost-reply reconciliation. Safe cleanup, actual managed-Git callbacks and full
client/platform/cut acceptance remain open; no parent task or Git admission gate
is accepted by this retention increment.

Final local macOS arm64 verification: locked server library tests passed 434 cases
with one ignored fixture; the complete 17-case real CLI branch/recovery suite
passed, including actual kill/restart, keep/rollback, stage/clear and no-op paths.
These clients use unmanaged file-copy and legacy Git paths, not owned managed-Git
source callbacks. The final focused worker suite passed 45 cases after the explicit
unsupported-version branch was added. Four-package all-target locked Clippy
(server/snapshot/tools/CLI) passed with Rust warnings denied. Native C compilation
reported sandbox_init/sandbox_free_error deprecation warnings; no main comparison
or pre-existing attribution was made. Format/diff and generated SDK consistency
passed. Pinned strict OpenSpec passed all 117 items; spec lint reported 0 errors/
25 warnings. Full snapshot/SDK/workspace execution, actual managed checkout
cross-process retention, real TUI/App, Linux/Windows and source-bound CI were not
run for this increment. Consuming recovery resource evidence, exact native marker
publication/reconciliation and reference-aware cleanup remain open.


## Recovery checkout receipt and native disposition continuation (2026-11-07)

Recovery now enriches the native resolution receipt with runtime-attested checkout
evidence: exact resolution origin, its version-2 lifetime and complete resource-set
digest, plus the original version-2 lifetime/resource-set digest when available.
Missing legacy original proof remains absent. The runtime verifies the immutable
native receipt before enrichment; native journal verification compares native
intent fields only, while Store verification attests the added runtime evidence.
The receipt checksum covers both kinds of evidence. It is a disposition binding,
not independent acknowledgement or source admission authority.

The consuming paired writer verifies both retained claim sets and proposed markers
read-only, checks exact original/resolution lifetime history, and refuses missing
version-2 evidence. After observing the durable pair, it publishes resolution
markers before original markers. Exact original claim bytes remain unchanged.
Failure after commit leaves the durable pair available to lookup and explicit
reconciliation. Reconciliation first verifies the committed pair, locks the exact
original and resolution lifetimes, reacquires their complete published resource
sets, and performs bounded metadata work without invoking source restore. Resource
or marker changes refuse. Existing matching markers are idempotent.

The code-review skill was applied across runtime ports/writer, native journal and
host adapters, tests, schema/SDK and spec changes. Review covered independent
owner identity, no legacy adoption, original-before-resolution lock acquisition,
claim-before-lifetime disposal, read-only candidate verification before the atomic
pair, publication ordering, exact committed-pair lookup, and retry without source
replay. The runtime regression uses actual Store operations with instrumented
lifetime/Location ports; the native host regression uses actual managed checkout
claims/pins/markers with fixture lifetime/receipt context. Neither alone proves
the full managed Git runtime path. Real CLI recovery fixtures use unmanaged
Locations and verify receipt/native lifetime correspondence.

Actual managed Git source admission remains disabled. Partial-constructor
recovery, reference/reader/lease-aware reclamation, complete native cuts and
platform/client acceptance remain required. No parent task or either issue is
completed by this continuation.


Local macOS arm64 verification for this continuation:

- Locked server library tests: 435 passed, 1 ignored. The new recovery case
  covers candidate refusal, successful paired disposition, interrupted original
  marker publication, committed-but-lost WAL reply, read-only lookup, missing
  publication refusal, repeated metadata reconciliation and reopened Store retry.
  The lost-reply test initially attempted a prohibited mutation through the
  failed Store; it now uses read-only lookup/reconciliation and explicitly opens
  the same database again, preserving production Store failure semantics.
- Locked snapshot suite: 137 passed, 2 ignored across unit/integration binaries;
  doc-test binaries had no tests. Native intent comparison and legacy receipts
  retain their existing behavior without treating runtime checkout context as
  journal-supplied evidence.
- Locked complete native worktree suite: 41 passed. The added managed-claim test
  checks read-only candidate preparation, wrong-role refusal, exact two-role
  marker publication, idempotence, changed receipt refusal, legacy refusal and
  byte-for-byte preservation of both original claim files.
- Locked real CLI branch/recovery suite: all 17 passed with new assertions for
  both receipt owners and actual worker lifetime/source-phase publications.
  Existing partial restore kills, keep/rollback, restart/retry, code/conversation
  stage/clear and no-op cases executed. These Locations are unmanaged; this does
  not accept managed Git source integration.
- OpenAPI regeneration test passed. Generated TypeScript consistency and separate
  SDK typechecking passed. SDK tests with CYBER_BIN pointing at the freshly built
  CLI passed 142 cases, with 2 Linux-only protected-file/vault cases skipped on
  macOS. The earlier run without CYBER_BIN passed 134 with 10 skipped.
- Formatting and diff checks passed. Pinned strict OpenSpec passed 117 items;
  spec lint reported 0 errors and 25 warnings, without baseline attribution.

Native C compilation emitted sandbox_init/sandbox_free_error deprecation warnings;
no unchanged-main comparison or pre-existing attribution was made. Full workspace
execution, Linux/Windows native runs, real TUI/App acceptance and source-bound CI
were not performed for this continuation. Full recovery/cleanup and both issues
remain open.


Four-package all-target locked Clippy (server/snapshot/tools/CLI) passed with Rust
warnings denied after correcting an Option source-phase path in the CLI assertion
helper. Server all-target locked Clippy passed again after the lost-reply test was
corrected. No warning allowances or production Store recovery changes were added.

Review verdict: approve this local consuming-receipt continuation. Critical: 0;
major: 0; minor: 0; nits: 0 unresolved findings. The changes preserve the existing
runtime/host port boundary and native adapter ownership. Executable Cargo unit,
integration/native and real CLI tests cover the added behavior; generated SDK
checks and real-client tests pass as recorded above. This review does not approve
the full recovery delivery increment or close the explicitly retained managed Git,
partial-construction, reclamation, crash/platform/client and CI acceptance gaps.


## Git object-format routing and retained-history inspection (2026-11-07)

A new regression against production code at 0a7cfad4 initialized an actual SHA-256
user repository and captured through the public Snapshots adapter. It failed with
a 69-character copy identity rather than a 64-character Git identity: libgit2's
read-only worktree discovery refused this repository, so non_git incorrectly
selected the copy backend. The former test manually initialized a SHA-256 shadow
inside a SHA-1 user repository and therefore did not prove public discovery.
This is a comparison against the prior local implementation, not an assertion
about unchanged main.

Read-only ancestor marker checks now prevent fallback admission for unsupported,
malformed, dangling or unreadable Git metadata. Canonicalizing the Location covers
relative paths and nested Locations; a marker does not supply repository, native
worker or mutation authority. Classification creates no identity cache or snapshot
namespace. Capture reports unavailable repository discovery explicitly instead of
silently returning a new copy checkpoint.

Before shadow initialization, bounded read-only Git metadata queries determine the
exact common repository storage format. Only SHA-1/SHA-256 are accepted; existing
shadow formats must match before capture/index changes. New shadows explicitly
initialize with the matching format and borrow only matching-format object stores.
The storage selector is documented in
[Git's versioned rev-parse manual](https://git-scm.com/docs/git-rev-parse/2.31.0#Documentation/git-rev-parse.txt---show-object-formatstorageinputoutput).
No new native Git source admission or checkout/reclamation authority is enabled.

Pre-fix SHA-256 Locations may retain copy checkpoints. Changed-path/diff inspection
now dispatches by typed pair identity, rejects mixed-backend comparisons explicitly,
and opens only an existing Location-bound store. Historical exclusions group copy
and Git IDs separately before combining reports. Missing stores refuse without
creating metadata. This prevents routing correction from hiding old read-only
history; cross-backend restore/migration is still unimplemented and must be resolved
before complete managed Git delivery. Original checkpoints/journals are retained,
not rewritten or relabeled.

Regression coverage uses actual SHA-256 user repositories and a linked worktree,
including nested Location scoping, UTF-16 canonical/raw images, exact native import,
legacy public reviewed restore of supported plain images, unchanged user index and
config, original gitfile preservation, malformed/dangling marker refusal,
mismatched existing shadow refusal and existing-only history inspection. Managed
Git admission remains disabled in all fixtures. These tests do not prove the full
runtime-owned Git callback, crash/recovery/platform or migration contract.


The code-review skill was applied to routing, initialization, read ports, tests
and spec changes. Review found and corrected a native reader-lifetime regression:
inspection-only Store opening lacked the lock formerly held by file_copy reads.
The existing-only read helper now holds the original native namespace lock through
its callback. A real held-Store regression refuses inspection with storage-busy,
then succeeds after the actual owner drops. Missing-store reads create no state.
This retains existing fencing; complete reference/reader/lease-aware reclamation
still requires its separate implementation and acceptance.

Local macOS arm64 verification: the full locked snapshot suite passed 141 cases
(101 unit, 18 file-copy integration, 22 Git integration), with 2 ignored subprocess
fixtures; no doc tests were present. The extended linked-worktree public-restore
case, missing-store/no-creation case and native reader-contention case also passed
focused final-source checks. All-target locked snapshot Clippy passed with Rust
warnings denied. Format/diff and generated SDK consistency checks passed. Pinned
strict OpenSpec passed all 117 items; spec lint reported 0 errors and 25 warnings
without unchanged-main attribution. Native C compilation reported sandbox_init/
sandbox_free_error deprecation warnings without origin attribution.

GitHub state was rechecked through the GitHub skill: #92 and #93 are open, no open
PRs were returned, and fetched origin/main remains db9e171e. There is no new main
composition to merge. No PR/push, source-bound CI, native Linux/Windows, TUI/App,
full workspace or separate SDK execution is claimed for this format-routing
continuation. Public API/schema shapes did not change.

Review verdict: approve the local format-routing and inspection correction;
0 critical and 0 major findings remain unresolved. The native reader-lock finding
was fixed and regression-tested. Existing snapshot adapter/native Store boundaries
are preserved. Full managed Git integration, cross-backend restore/migration,
configuration/filter races, partial-constructor recovery, reference-aware cleanup
and full client/platform acceptance remain open; neither issue nor parent tasks
are complete.


Final-source locked real CLI branch/recovery acceptance passed all 17 cases,
including partial-replacement kills, keep/rollback, restart/retry, stage/clear,
disabled snapshots and ordinary no-op receipts. These cases preserve unmanaged
file-copy and legacy Git behavior; the new SHA-256 fixtures above separately cover
actual repository-format discovery and scoped materialization. No managed Git
runtime activation or full goal acceptance is inferred from these results.


## Capture-bound Git configuration locator observations (2026-11-07)

A regression against the preceding local production implementation at 4d801312
prepared a valid native Git restore, then changed the private config's
core.excludesfile from one empty fixture file to another. The restore proceeded:
only resolved ignore inputs were retained, so changing the selecting configuration
after preparation escaped revalidation. The new test failed before implementation
and passes now, with unchanged source bytes and no published journal. This is a
local-head regression comparison, not attribution to unchanged main.

Supported policy collection now obtains bounded origin metadata and selecting
root candidates, including nonexistent Git-reported system/global config files,
the shadow config/config.worktree/HEAD, explicit config environment and include
targets resolved relative to their origin files. Configuration files are regular,
size-bounded stable observations with content digest, native generation, canonical
route and parent identity. Environment values enter only a digest; full configuration file contents and
cleartext environment values are not retained in the handoff or checkpoint tags.
Resolved policy paths remain necessary private handoff metadata.
Physical worktree/private-directory identities bind Git-directory conditions.
Root/input state is rechecked around resolution and in native inventory before
intent publication, without starting Git inside the synchronous filesystem worker.

Review corrected a value-selection race: querying settings independently of their
origin stream could choose a different include during resolution. Values now come
from the bounded inventory itself. Git expands explicit supplied overrides, with
include paths batched under argument/output limits and literal NUL framing. It
cannot silently reselect a different setting. Unsupported named-user home expansion
is refused rather than depending on an unbound account database. File alias checks
also bind canonical parent identity, including same-inode hardlink aliases.

The policy fingerprint frame is now git-policy-capture-v2. Historical checkpoint
encodings remain readable; their old fingerprint cannot gain current config proof.
An old expected native checkpoint requires a fresh captured review instead of a
retroactive upgrade. This changes private policy evidence, not public API shapes.

Native policy collection admits the app-created standalone private shadow with
unborn HEAD; common-dir/worktree/reftable substitutions, linked HEAD or loose/packed
branch refs are unsupported. Packed snapshot refs are allowed so ordinary Git GC
does not invalidate the policy merely by packing snapshots. Remote-URL conditional
include dependency closure and general alternate shadow reference storage are not
implemented: affected captures retain explicit unavailable policy/raw-state proof
and canonical legacy-readable checkpoints. They do not acquire native source,
checkout acknowledgement or cleanup authority. Full configuration/metadata ABA,
dependency and compatibility acceptance remains open; this is bounded optimistic
revalidation, not a claim of a globally atomic external-config snapshot.

Git's own documentation describes
[system/global candidate paths even when absent](https://git-scm.com/docs/git-var#Documentation/git-var.txt-GITCONFIGGLOBAL),
and its [versioned config implementation](https://github.com/git/git/blob/v2.51.0/config.c)
shows the Git-directory/branch/remote dependencies that motivate these bounds.
Capability failure keeps legacy capture readable; no new production support floor
or managed Git source gate was selected.

Shared bounded metadata execution now covers reads and process exit with a ten
second deadline. Timeout/output errors kill and await the exact subprocess before
returning. A real FIFO configuration fixture exercises timeout and subsequent
reader reuse. This does not extend that guarantee to every legacy capture/filter
subprocess; external filter effects remain a separate admission/compatibility gap.

Verification on local macOS arm64: the locked full snapshot suite passed 153 tests
(113 unit, 18 file-copy integration, 22 Git integration), with four ignored helper
fixtures across binaries; the environment/global-root helpers execute through their
parent tests. The final focused capability case also passed after extending its
legacy-readability assertions. Real cases cover locator changes after preparation,
missing include/global creation, included file changes, conditional HEAD changes,
environment binding without cleartext values, file replacement/size limits,
symlink/hardlink retargeting, physical directory context, original private reference
layout and packed snapshot preservation. All-target locked snapshot Clippy passed
with Rust warnings denied. Full locked real CLI branch/recovery acceptance passed
all 17 cases, preserving actual kill/restart, keep/rollback, stage/clear and no-op
behavior. These CLI cases use unmanaged copy and legacy Git paths; they do not
accept managed Git runtime source integration.

The code-review skill was applied across collectors, native observations,
subprocess cleanup, handoff/fingerprint, tests and spec deltas. It led to bound
value expansion, physical context and unsupported-layout/dependency refusals above.
Review verdict: approve this local gated metadata prerequisite, with no unresolved
critical/major finding for its exposed scope. Full managed Git activation still
requires the recorded dependency/ABA/filter, backend/runtime/checkout, migration,
crash/platform/client and reclamation acceptance; no M2–M8 or parent task closes.
Native Linux/Windows, TUI/App, full workspace, separate SDK runtime checks and
source-bound CI were not run for this continuation. Native C compilation reported
sandbox_init/sandbox_free_error deprecation warnings without origin attribution.


Final format/diff and generated SDK consistency checks passed. Pinned strict
OpenSpec validated 117 items; spec lint reported 0 errors and 25 warnings without
unchanged-main attribution. Public schema/SDK shapes remain unchanged. The
production managed-Git availability guard remains disabled, and no PR/push or
parent-task acceptance is made by this local prerequisite.


## Mixed copy/Git historical image materialization (2026-11-07)

The native Git planner now accepts retained copy target and/or recorded checkpoints
with a fresh raw Git expected checkpoint. Immutable Git preparation retains typed
copy placeholders without attempting Git object lookup for copy IDs. Inside the
synchronous native store, original copy manifests are loaded under the existing
namespace lock, including their original Location/root binding and immutable tree
digest. Original object bytes are checked against BLAKE3 identities; Git images
retain typed SHA-1/SHA-256 verification. These image kinds are explicit, including
when both content hashes happen to be 64 characters. IDs remain unchanged: a copy
checkpoint is never relabeled as a Git checkpoint or given an invented Git blob.
Git image buffers are borrowed; copy buffers are materialized under shared total
and per-file/count limits. Limits conservatively cover the full prepared/imported
image set; this is not complete selected-path/large-image migration acceptance.

Git worktree/prefix and exact native Location still bind the handoff. Historical
exclusions from copy and Git constrain target/recorded mutation; expected eligible
images remain in the complete current-state check even when a historical exclusion
protects them from mutation. Originally skipped files that are now eligible remain
unchanged. Copy manifests cannot supply caller-populated image packets instead of
actual manifest authority. Corrupt objects and copied-in foreign manifests refuse
before intent publication or source effects.

A new version-6 Git context records target/recorded/expected image-source kinds and
original IDs. The physical journal backend remains Git; its native payload objects
still use the existing BLAKE3 CAS. Version-6 validation requires a Git expected
checkpoint, at least one copy historical input, consistent Git ID widths and exact
kind/ID correspondence. Version-4/5 encodings omit the optional section and retain
all-Git interpretation; annotation or version downgrades refuse instead of adopting
mixed authority. Version-6 immutable digest framing is distinct, and new recovery
intents preserve the context and original identifiers. Existing older digest
regressions still pass.

Native backend regressions cover all three mixed source combinations at both root
and nested Locations, using an actual SHA-256 repository and retained copy stores.
They check preview/restore, original IDs, typed annotations, historical exclusions,
user-index preservation and old-version/mismatched-kind refusal. Separate cases
refuse corrupt copy objects, a valid foreign manifest copied into the current
namespace, stale source bytes and combined budget overflow without a journal or
source modification. The existing native keep/rollback test now exercises both
all-Git and mixed version-6 contexts, preserving unrelated edits and never replaying
source changes for a repeated request.

A further real-filesystem case stops execution after the first actual namespace
replacement, verifies the second file is untouched, drops/reopens the native Store,
and rolls back only proven retained effects. Unrelated owner files and edits made
after resolution survive; repeated lookup returns the same receipt without replay.
This is injected interruption and reopened native storage, not an OS kill/restart
or runtime-bound checkout test. Synthetic fixture origins grant no runtime Store
pair or actual worker-lifetime acknowledgement authority.

The cognitive-complexity skill was consulted. Its dispatcher does not support
Rust; native Clippy identified preparer complexity 17 against the repository's
limit of 15. Extracting historical Git exclusion collection into a small helper
resolved the diagnostic without allowances or policy changes. An incorrect FileDiff
field in the new test was also corrected before verification.

The code-review skill was applied to source kinds/hashes, manifest/root validation,
combined limits, exclusion/stale-state separation, pre-effect unknown-journal
fencing, native preimages, version/digest compatibility, recovery/retry and tests.
Review verdict: approve this local gated backend continuation, with no unresolved
critical/major finding in that scope. Public client migration and runtime-owned Git
integration remain incomplete, alongside complete config/generated-rule-source,
filter and remaining compatibility, constructor/native kill/platform and GC
acceptance. No Git availability guard, parent task or issue is marked complete.

Local macOS arm64 verification: the final full locked snapshot suite passed 156
cases (116 unit, 18 file-copy integration, 22 Git integration), with four ignored
helper fixtures across binaries and no doc tests. The focused interrupted mixed
replacement case passed independently. All-target locked snapshot Clippy passed
with Rust warnings denied. The full locked 17-case real CLI branch/recovery suite
passed, preserving existing unmanaged copy/legacy Git kill/restart, keep/rollback,
stage/clear and no-op behavior. These CLI cases do not exercise public version-6
migration because managed Git admission remains disabled. Public API shapes are
unchanged; no separate SDK, full workspace, TUI/App, Linux/Windows native execution
or source-bound CI is claimed for this continuation. Native C compilation emitted
sandbox_init/sandbox_free_error deprecation warnings without origin attribution.


Final-source denial coverage also rejects a caller-populated, internally
hash-consistent copy image packet instead of its original manifest; that focused
case passed after the full suite. Final format/diff, generated SDK consistency and
all-target snapshot Clippy checks passed. Pinned strict OpenSpec validated all 117
items, and spec lint reported 0 errors/25 warnings without baseline attribution.
No PR/push or native Git source activation is made by this backend increment.

## Generated Git exclude source binding (2026-11-07)

A new real-Git regression reproduced a source-policy gap on local `5be7ab93`:
changing the user common repository's `info/exclude` after native preparation still
allowed source restoration. The regression failed its refusal assertion before the
production fix. The first invocation accidentally selected no tests with an
incomplete exact name; the fully qualified rerun executed and reproduced the gap.

Supported native collection now verifies a bounded version-1 generation recipe
against fresh original source observations, exact application settings, skipped
literal paths and regenerated output bytes. The source locator is derived from
the trusted Shadow common directory, not deserialized metadata. Source-file absence
and common-directory identity are retained. The combined configuration proof also
retains generated output/recipe file observations and the shadow info directory.
The synchronous planner rechecks this combined proof without invoking Git. A new
`git-policy-capture-v3` frame does not promote older checkpoint policy evidence.

Generation uses descriptor-relative atomic replacement beneath a no-follow info
directory. Existing output links are replaced without opening their target for
writes; unchanged output and recipe are not rewritten, preserving checkpoint
identity. Original source bytes are bounded and preserved, including non-UTF-8
patterns, rather than silently dropping unreadable text. Unsupported/nonregular
source files fail generation explicitly. Windows retains its existing legacy
implementation and has no new native proof from this change.

Review identified and corrected an initial compatibility regression: generated
source observation must not require a supported native configuration environment.
File-only observations now remain independent of environment interpretation, while
actual native configuration collection and merged proofs retain full environment
binding. An isolated multiline-configuration fixture verifies ordinary canonical
capture remains available with explicitly unavailable native proof. Another case
verifies non-UTF-8 exclude bytes retain canonical capture without native authority.
An unavailable Rust FIFO helper and a private-method reference in test fixtures
were corrected before final validation. Changed-settings coverage now checks early
recipe refusal and stale native policy after regeneration under fresh settings.

Tests additionally cover changed user excludes before effects, stable repeated
checkpoint IDs, common-source changes for scoped SHA-256 linked worktrees, absent
source creation, physical common-directory replacement, recipe/output tampering,
settings mismatch, symlink-safe publication, parent-link refusal and over-limit/
FIFO inputs. This is not a proof of user-index or large-untracked eligibility,
full configuration ABA/dependency/filter compatibility, actual managed Git runtime
ownership/checkout, complete crash/platform acceptance or reference-aware cleanup.
Managed Git availability and M2–M8/parent acceptance remain unchanged and open.

The code-review skill was applied to all changed source, fixtures and spec deltas,
using the existing adapter architecture. Review checked bounded reads/rendering/
parsing, trusted locator derivation, symlink/alias and directory replacement,
source-generation agreement, combined proof limits, native environment retention,
legacy/canonical compatibility, checkpoint stability and absence of new mutation/
acknowledgement authority. The environment compatibility finding above is fixed
and regression-tested. No unresolved critical/major finding remains within this
local generated-source-binding scope. This does not accept the wider managed Git
or complete recovery feature.

Final local macOS arm64 snapshot verification passed 163 cases (123 unit, 18
file-copy integration, 22 Git integration), with five ignored subprocess fixtures
across test binaries and no doc tests. The isolated multiline environment fixture
is invoked by its passing parent. A lifetime fixture child emitted UnexpectedEof
during real-process death testing; both parent cases passed. Final all-target
locked snapshot Clippy passed with Rust warnings denied. The initial parallel
17-case real CLI suite passed 16 and failed one origin-before-source kill case at
its 30-second reservation-observation deadline. It did not reach recovery assertions;
no claim about the timeout's origin is made. The final-source serial CLI run subsequently passed all 17 cases in 204.49 seconds
without overlapping heavy filesystem fixtures. This supplies serial acceptance,
not an explanation or acceptance of the earlier concurrent observation timeout.

Final format/diff and generated SDK consistency checks passed. Pinned strict
OpenSpec validated 117 items; spec lint reported 0 errors and 25 warnings without
unchanged-main attribution. Native C compilation emitted sandbox_init/
sandbox_free_error deprecation warnings, and the CLI linker reported an oversized
unwind table warning; no origin attribution is made. This continuation did not run
native Linux/Windows, separate SDK runtime suites, full workspace, TUI/App or
source-bound CI. GitHub refresh found #93 open and no open PRs; fetched origin/main
remains db9e171e. No PR/push or parent-task acceptance is made by this local fix.

## Typed restore recovery CLI (2026-11-07)

The approved `sessions recovery` command now lists retained journals without a
journal argument, reviews an exact journal/action, and confirms only with both the
reviewed lowercase fingerprint and caller-retained stable request ID. Clap refuses
malformed and incomplete combinations before server startup. The separate
`sessions recovery-request` command performs durable lookup; explicit
`--acknowledge` uses the existing metadata-only native reconciliation route.
No snapshot worker, journal format, permission boundary or backend availability
guard is changed. Existing API/SDK operations remain the source of recovery logic.

Replies deserialize into the shared runtime contract. Reviews must match selected
Session/journal/action, and a confirmable review cannot lack original-worker proof.
Results must match Session, request ID and original journal; confirmation must
match its full request content. Missing legacy evidence remains readable without
confirmation authority. Text shows image kinds, hashes, modes, ownership and native
identity, historical exclusions/created parents and conflicts. Runtime state is
reported separately from observed journal acknowledgement. Confirmation exits 1
unless runtime settlement is acknowledged; explicit reconciliation additionally
requires an observed native acknowledgement. Read-only lookup may successfully
report a pending/unknown state. Repeated confirmation remains server-side lookup,
including after later owner edits; no client automatically chooses another ID.

Error hints retain the exact request with shell quoting and an end-of-options
marker, or JSON-escaped identities when controls prevent safe literal display.
The CLI rejects exact `.`/`..` request IDs because URL clients normalize those path
segments; this is documented rather than silently looking up a different route.
Other spaces, slashes and URL punctuation are percent-encoded with their original
identity retained. Source paths and displayed request IDs use escaped quoted
literals so newline/control characters cannot collapse distinct names or affect
the terminal. The operator guide and generated command reference are updated.

Real service-kill tests now execute typed CLI listing/review/confirmation for both
keep and rollback after a proven partial namespace replacement. They verify stale
fingerprint and foreign Session/request refusal, unchanged transcript/inference
count, original/resolution native receipts, later owner edits, repeated
confirmation and request lookup after another service restart. Explicit typed
reconciliation restores missing journal acknowledgement metadata while retaining a
later source symlink and its target bytes. The final fixture additionally includes
a real newline-containing source filename and checks its unambiguous text display.
API preparation tests remain separate; they are not typed preparation-client proof.

The code-review skill was applied to command admission, scope correlation,
transport ambiguity/retry identity, terminal escaping, exact URL segments, response
state/acknowledgement semantics and all changed tests/docs. Review corrected the
initial lossy path display and retained unavailable legacy/native proof explicitly.
No unresolved critical/major finding remains for this restore-journal CLI scope.
The cognitive-complexity skill was consulted; its analyzer does not support Rust.
Native Clippy reported the expanded integration fixture at 21 against 15. Extracted
review and foreign-request assertions keep responsibilities clear and satisfy the
native threshold without allowances. An initial sibling-helper visibility error
was also corrected before verification.

This advances M7 without accepting it: preparation/TUI recovery, actual managed
Git source/runtime integration, partial-constructor recovery, reference-aware
cleanup/cap/retention and full native/platform/client acceptance remain required.
No parent task or issue closes, and no PR/push or source-bound CI is claimed.

Local macOS arm64 verification: the full serial real CLI branch/recovery suite
passed all 17 cases in 201.36 seconds. After the display correction and fixture
helper extraction, the focused process-kill case passed both keep and rollback
in 51.67 seconds, including the real newline filename, punctuation-bearing request
IDs, foreign/stale refusal, restart lookup and metadata-only acknowledgement.
Final-source CLI unit tests passed all 84 cases, including argument validation,
response scope, escaped labels/hints and generated-reference consistency. Final
all-target locked CLI Clippy passed with Rust warnings denied. All Cargo builds,
checks, tests and Clippy used CARGO_INCREMENTAL=0 and --locked.

Final format/diff, generated SDK consistency and pinned strict OpenSpec checks
passed (117 items). Spec lint reported 0 errors/25 warnings, with no unchanged-main
origin attribution. Native C deprecation warnings for sandbox_init/
sandbox_free_error and the CLI oversized unwind-table linker warning were
observed without origin attribution. No separate full snapshot/workspace suite,
SDK runtime suite, TUI/App acceptance, native Linux/Windows execution or
source-bound CI ran for this command-only continuation. Managed Git source
availability remains disabled and all broader completion gates remain open.

## Terminal restore recovery and retained-request discovery (2026-11-07)

Rewind C now opens a recovery panel without inference. Journal selection performs
read-only request discovery before permitting a new attempt. K/B reviews an exact
keep/rollback selection; Enter confirms only a confirmable fingerprint and generates
one retained request identity. Confirmation immediately drops its preview. An
ambiguous response retains that identity and R performs lookup, never source
replay. A separately reconciles native metadata for an acknowledged runtime result.
A positively cancelled pre-effect request, or one resolved by another owned
request, can lead to a fresh explicit review of the original restore. Pending and
unknown outcomes cannot license another attempt. No force-clear is introduced.

The controller uses rewind's existing Session/Location/UI-generation/overlay guard
and never invokes rewind's composer-replacement finish path. It validates full
confirmation identity and original Session/directory/journal on results; reviews
must match their selected action/journal and cannot confirm without original
worker identity. Closing or switching scope invalidates late replies without
cancelling or replaying an admitted worker. Display preserves exact quoted paths
and request IDs with escaped controls, distinguishes absent historical evidence
and separates runtime settlement from observed native acknowledgement. Review and
outcome capabilities are dropped on failed/mismatched replies. A failed refresh of
previously empty discovery also disables fresh-attempt keys until discovery
succeeds, rather than treating cached emptiness as current evidence.

New authenticated GET recovery/requests with required journal_id discovers up to
4096 retained confirmation identities for the current Session creation and exact
journal. It reads latest native snapshot-worker records, excludes binary document
owners, validates request syntax and recomputes each exact request-owner identity.
Overflow refuses without deleting history; discovery is not acknowledgement or
native origin authority. It does not open native namespaces or inspect source
files. This server-side discovery lets a fresh TUI App recover a lost request
without making an in-memory ID or local preference file its sole retry identity.
OpenAPI, generated TypeScript operations/types, SDK tests and the operator guide
are synchronized. Query metadata now declares the required journal selector.

Two real production-App/native scenarios create 1280 changed files, start a
reviewed rewind through the TUI action port, observe an actual first replacement
with the last file still untouched, and interrupt the exact live worker. The
runtime correctly retains uncertainty and returns recovery-required; the original
fixture's expectation of successful interruption was corrected. Both keep and
rollback are then reviewed and confirmed through the TUI/public API. Assertions
verify their distinct first-file results, unrelated owner data, the original draft,
no transcript movement and no inference. After discarding the successful recovery
reply, a freshly loaded TUI App discovers the existing request, reads its durable
outcome and explicitly reconciles native acknowledgement while preserving a later
owner edit and new terminal draft. These are native cancellation and fresh-App
cases against a live server, not OS process kills of the TUI or server. The updated
real CLI service-kill case separately verifies discovery after server restart and
empty discovery for a foreign Session. Existing runtime cancellation coverage also
verifies discovery of an exactly owned cancelled request, journal filtering and
invalid selector refusal.

The code-review skill was applied to bounded/parameterized discovery, Session
creation and native-owner filtering, response correlation, UI generation fencing,
duplicate/ambiguous requests, proof availability, terminal display, independent
native acknowledgement and all changed tests/generated artifacts. Review fixed
cached-discovery admission and the missing fresh-review route after another owned
request resolves an attempt. No unresolved critical/major finding remains for
this local restore-journal UI/discovery scope. The cognitive-complexity skill was
consulted; its dispatcher does not support Rust. Native Clippy required boxing the
large review reply and splitting a fixture's source-observation wait (16 versus
15); final checks pass without allowances. Test helper reborrows, a clone of a Copy
state and an initially missed guard edit were corrected before final verification.

Verification on local macOS arm64: the full four-thread TUI library suite passed
669 tests with three configured ignores in 325.60 seconds. These include existing
rewind/undo/redo, composer, theme, accessibility and screen regressions. Subsequent
focused recovery checks passed five cases including the strengthened native
keep/rollback/unrelated-owner case (44.13 seconds), plus the final settled-attempt
fresh-review state test. The full HTTP suite passed all 79 tests, and the focused
runtime cancelled-request/discovery case passed. The actual CLI partial-kill
keep/rollback test passed in 86.26 seconds, including post-restart and foreign
request discovery. Final all-target locked server/TUI Clippy passed with Rust
warnings denied. All Cargo checks/builds/tests used CARGO_INCREMENTAL=0 and --locked.

SDK typechecking passed. The initial SDK run passed 135 with ten real-binary cases
skipped because CYBER_BIN was absent; rerunning with the built binary passed 143
with two platform skips. TUI ignores include the isolated pure-mode child fixture
and two Linux temporary-protected-file cases that avoid native vault writes on
macOS. Native C sandbox_init/sandbox_free_error deprecation and oversized unwind
linker warnings were observed without unchanged-main attribution. OpenAPI generation,
SDK consistency, format/diff, pinned strict OpenSpec (117 items) and spec lint
(0 errors/25 warnings) passed. No native Linux/Windows execution, full workspace,
OS TUI-kill matrix or source-bound CI is claimed. Preparation clients, actual managed
Git source/runtime integration, partial-constructor recovery, reference/reader/
lease-aware reclamation and full cap/platform/client acceptance remain open. No
parent task, issue, Git availability guard or delivery status is promoted; no
PR/push is made by this local continuation.

## Typed preparation recovery CLI (2026-11-07)

`sessions recovery-preparation` now lists candidates or reviews an exact activity.
Cancellation requires both its fingerprint and caller-retained request ID. The
separate `sessions recovery-preparation-request` performs committed receipt lookup;
explicit `--acknowledge` invokes existing native metadata reconciliation. No server
operation, native admission rule or journal encoding changes. Shared fingerprint,
request-ID and typed-response helpers retain their existing validation. Replies
must match Session/activity/lifetime and any published origin's creation, worktree,
directory and root identity. Namespace identities with different roles are not
conflated. Unsupported review lifetime/phase evidence and admitted-effect receipts
refuse. Historical receipts retain read semantics without acquiring new proof.

Text distinguishes durable cancellation from native acknowledgement observation,
quotes paths/IDs, reports unavailable checkout proof and preserves independent
uncertainty. Ambiguous confirmation errors retain an exact shell-quoted or
JSON-escaped lookup hint. No force option exists. The operator guide and generated
CLI reference are synchronized. Grouped recovery dispatch keeps the Session
router readable; the existing deletion handler remains unchanged.

Both actual pre-origin and published-origin-before-source service-kill cases now
use typed listing/review/confirmation, repeated confirmation, request lookup after
restart and explicit reconciliation. They verify stale/foreign refusal, exact
resource digests/tombstones, later owner symlinks/files, unchanged messages and no
inference. Final local macOS arm64 checks passed: all 86 CLI unit tests, all 17
serial branch/recovery cases (187.37 seconds), all-target locked CLI Clippy,
format/diff, generated CLI/SDK consistency and pinned strict OpenSpec (117 items).
The two focused native cuts also passed separately in 61.63 seconds. Spec lint
reported 0 errors/25 warnings, without unchanged-main attribution. Every Cargo
build/check/test/Clippy used CARGO_INCREMENTAL=0 and --locked.

The code-review skill covered parser-before-startup refusal, exact response scope,
legacy evidence, replay/lookup separation, native acknowledgement limits and all
changed tests/docs. No unresolved critical/major finding remains in this CLI
scope. The cognitive-complexity skill was applied using native Rust Clippy because
its dispatcher does not support Rust; grouping recovery routing resolved 17/15
complexity without allowances. C deprecation and oversized unwind linker warnings
were observed without origin attribution. No separate SDK runtime, TUI/server/full
workspace, Linux/Windows native or source-bound CI checks ran for this command-only
continuation. Preparation TUI, partial constructors, managed Git source integration,
reference-aware reclamation and broader acceptance remain open; no parent task,
issue or Git availability guard is promoted. No PR/push is made by this local work.

## Preparation recovery in the terminal and committed receipt discovery (2026-11-07)

The rewind recovery panel now offers P for preparation candidates, including an
opt-in history of committed cancellations scoped to the current Session creation.
Default API listing remains pending-only. Candidate Enter discovers the original
receipt using a new read-only route before requesting any pre-effect review.
Absence itself grants no authority. Review/confirmation still require exact exited
native worker proof; no source-admission or legacy-fence policy changes.

The nested terminal controller uses the existing rewind Session/Location/UI
generation guard. It validates activity/lifetime/origin and confirmation scope,
retains the stable cancellation identity before POST, consumes the review and
makes outcome Enter inert. R looks up that exact request; A explicitly reconciles
native metadata only after a committed receipt. A receipt alone is never displayed
as observed native acknowledgement. Independent restore/process/checkout
uncertainty, draft, attachment payloads and conversation are preserved. J returns
to journal recovery. The HTTP transport stays separate from controller/rendering.

The new receipt endpoint discovers the retained activity request key and delegates
to existing exact paired-receipt validation. Its always-present nullable data field
is explicit in OpenAPI so the generated SDK returns the receipt or null. Discovery
never calls native reconciliation or accesses source files. The existing native
CLI crash fixtures now verify pending/history separation and receipt discovery
after a real service restart, plus refusal for an existing foreign Session.

Code-review skill inspection covered all changed controller/transport/rendering,
HTTP/Store queries, generated artifacts, tests and docs. Scope guards, busy gates,
consumed confirmation, ambiguous-reply behavior, native acknowledgement separation
and explicit null handling were checked. No unresolved critical/major finding was
identified in this increment. Rust complexity uses native Clippy because the
cognitive-complexity skill dispatcher does not support Rust; 18/15 findings in
the controller test and native CLI fixture were resolved by separating
lookup/reconciliation and history checks, without lint allowances. The shared managed Git guard remains disabled. Native preparation
recovery through an actual killed TUI process, full crash/platform cuts, partial
constructors, reclamation and remaining #93/#92 work remain open.

Local verification for this increment: all 675 TUI library tests passed (3 ignored
cases: the separately driven pure-mode child fixture and two Linux protected-file
agent/collaboration cases avoiding native vault writes on macOS); the native
partial-journal interruption/reopen regression passed within that suite. All 80
HTTP tests, the focused cancellation/history/independent-fence runtime regression,
and the two real CLI native pre-effect service-kill cuts passed. The final SDK
suite passed 144 tests with 2 Linux protected-file/native-vault platform skips;
typechecking, generated SDK consistency, all-target server/TUI/CLI Clippy, pinned
strict OpenSpec (117 items), and formatting/diff checks passed. Spec lint reported
0 errors/25 warnings, without unchanged-main attribution. Every Cargo
build/check/test/Clippy used CARGO_INCREMENTAL=0 and --locked. The nullable receipt
schema has a dedicated regression and the SDK test statically admits null.
C deprecation and oversized-unwind linker warnings were observed without origin
attribution. Full workspace, native preparation TUI process-kill, additional
Linux/macOS architectures, Windows and source-bound CI were not run here; this is
local evidence, not complete M7/M8 or accepted delivery. No PR/push is made by this
increment and no parent task or issue is closed.

## Production TUI process death with a committed preparation reply withheld (2026-11-07)

A new default Linux/macOS branch integration case drives the actual `cyber --resume`
binary through a POSIX pseudo-terminal, using production terminal setup, keyboard
dispatch, event loop and HTTP transport. Its Python standard-library driver lives
under CLI test support; the Rust fixture retains native source and Store assertions.
It uses the same real service-kill boundaries before origin and after origin but
before source admission, with exact retained lifetime evidence. No production fault
hook or recovery admission behavior changes.

A loopback proxy redirects only the fixture's private registration URL, forwards
the original owner authentication, and withholds all confirmation response headers
and bytes after receiving a successful committed receipt from the real server.
The test observes SIGKILL termination of the first TUI, launches a new production
TUI, and enters preparation recovery through Esc-Esc/C/P and actual Enter keys.
The new process discovers the existing receipt, makes outcome Enter inert, looks
up the original key with R and explicitly reconciles it with A. The proxy requires
exactly one cancellation POST across both terminal processes and no additional
review after committed receipt discovery. Positive lookup and acknowledgement
responses must be HTTP 200, and no acknowledgement request may precede explicit A.
Transient busy read-only reviews may be explicitly rediscovered/reviewed up to
three times before confirmation; no mutation is retried. Read-only C/P entry also
handles loading and journal-inspection refusal from an unconstructed pre-origin
namespace; that absence never supplies cancellation proof. The proxy restores registration and releases held responses in cleanup.

The saved draft and text-attachment payload survive the killed terminal and the
second terminal's graceful exit. The shared native fixture also verifies immutable
receipt/resource identity, restart lookup and idempotent confirmation, later owner
symlink/target bytes, other source bytes, unchanged messages and no inference.
The final focused two-boundary case passed locally on macOS arm64 in 74.23 seconds.

Code-review skill inspection covered private registration/credential handling,
PTY ownership and bounded diagnostics/timeouts, source assertions, exact request
correlation, read-only retry versus cancellation replay, child cleanup and honest
platform scope. Rust Clippy's new fixture 16/15 finding was resolved by separating
client confirmation dispatch. The cognitive-complexity skill ran complexipy 8.3.0
from an isolated temporary environment: the Python driver's maximum was 13 in its
PTY pump, with no analyzer warnings. No unresolved critical/major review finding
was identified. Other native platforms, writer/native-acknowledgement/reclaim crash
cuts, managed Git source admission, partial constructors and safe reclamation
remain incomplete; this local acceptance does not close M7/M8 or either issue.

Verification: the serial branch/recovery invocation passed all 17 existing cases
and failed the new PTY case because its driver incorrectly required successful
journal inspection before P. After correcting only that synchronization assumption,
the final focused case passed both native boundaries with the stricter positive
HTTP/explicit-acknowledgement checks. The full 18-case invocation was not repeated
after that driver fix; the existing cases do not use this Python driver. All-target
CLI Clippy, Python compilation, complexity analysis, format/diff checks and pinned
strict OpenSpec (117 items) passed. Spec lint reported 0 errors/25 warnings, without
unchanged-main attribution. C deprecation and oversized-unwind linker warnings
were observed without origin attribution. All Cargo tests/Clippy used
CARGO_INCREMENTAL=0 and --locked. No separate SDK/server/TUI-library/full-workspace
or additional native platform/CI checks ran for this test-only increment. It makes
no PR/push and closes no parent task or issue.

Reproduce the focused real-terminal case with Python 3 and its POSIX PTY standard
library available (it runs by default in Linux/macOS branch integration tests):

```sh
CARGO_INCREMENTAL=0 cargo test --locked -p cyber-cli --test branches_cli terminal_preparation_recovery -- --test-threads=1
```

## Ordinary acknowledged file-copy payload reclamation (2026-11-07, local)

The first M6 cleanup path reclaims displaced regular-file stages from completed,
natively acknowledged ordinary file-copy restores in an unmanaged Location. It
requires an immutable ordinary restore receipt, version-2 exited worker lifetime,
positively published empty checkout resources and unchanged native source phase.
It does not admit managed Git, keep/rollback resolution, cancelled preparation,
partial construction or legacy payload cleanup. M6 and parent tasks remain open.

A deterministic receipt-owned Store aggregate records a bounded immutable plan
before deletion. It freezes each stage leaf's original inode identity, metadata
generation, BLAKE3 identity and size. The application adapter re-attests the receipt
and complete reference view in its serialized writer transaction while the native
callback retains namespace, Location reader/mutation and lifetime locks. Native
intent/phase pins, current unmanaged checkout inventory, fresh retention policy
and every remaining leaf are checked. Missing leaves count as resumable progress
only for an already committed matching plan. No source image is read or installed;
intents, worker metadata, ordinary receipts and all referenced CAS images remain.
A separate completion event preserves cumulative accounting and idempotent retry.
A cap failure can invoke the same admission and retry capture once. Existing
fallback collection now reads references under the writer that excludes concurrent
reference commits. Its conservative worker/journal gates remain in place; complete
CAS/metadata/worker reclamation is still required.

The reference index migration backfills canonical copy manifest identities from
retained event values, then uses transactional SQLite insert/update triggers and
per-event deletion cascades. It avoids reparsing all conversations at every cleanup
admission. The old writer catalog refuses the new migration. Migration regressions
cover nested/duplicate/shared roots, invalid identities, original event byte
preservation, generic imports, transaction rollback, metadata repair, deletion and
older-catalog refusal. Two older compatibility tests initially failed because they
applied the entire later catalog while asserting their original marker's schema or
single unknown ID. They now test that exact marker migration; the separate index
case tests the new schema and older-catalog refusal.

Real-App integration uses actual scripted-provider prompts, file-copy snapshots,
reviewed rewind, paired Store settlement and native acknowledgement. It verifies
reclaimed stages, unchanged retained images/intent, later source symlink/target,
unchanged transcript/inference, receipt reconciliation, repeated cleanup, retention
age/zero/disabled capture, real reader/lifetime locks, independent unknown/missing
Location owners, aliases/replacements/missing leaves/unknown entries, contradictory
native source phase, cap-pressure retry and complete-reference bounds. Pending
plans still obey later retention changes and unknown-owner fences. Historical
projection replay remains possible after later uncertainty or Session deletion;
replaying records grants no filesystem or native acknowledgement authority.

Two actual process-death boundaries launch a separate test executable using the
same production App and native cleanup adapters. A test-only Policy wrapper pauses
inside the consuming writer callback before native deletion or after all private
deletions/fsyncs but before callback return/completion append. The parent observes
the boundary, sends SIGKILL, verifies signal 9, reaps the process, reopens the actual
file-backed Store and resumes through the normal policy. Both retain a pending
durable plan and complete idempotently without replaying source, including a later
owner's symlink and target. The ordinary suite helper is inert without its explicit
fixture environment. A separate partial-unlink state test is synthetic and is not
claimed as a process kill between individual unlinks. Native writer commit,
every-unlink, metadata replacement and all other crash/platform cuts remain open.

Code-review skill inspection covered all changed production/test/migration files:
Store/native ownership separation, reference atomicity, bounded plans and names,
receipt identity, native lock lifetime and no-follow private deletion, replay versus
new admission, retention and quota semantics, error/evidence preservation, process
cleanup and platform cfgs. Review corrected an inconsistent multi-query reader by
using explicit read transactions for maintenance receipt/plan lookup, and added
native phase contradiction checks and metadata pins before accepting deletion.
Rust uses the workspace Clippy cognitive-complexity threshold (15), without
allowances; the cognitive-complexity skill has no Rust analyzer. The complete M6
feature still needs managed resources, resolution/preparation construction classes,
CAS/metadata reclamation and the full native class/lease/platform/crash matrix.

Verification on local macOS arm64: the full Store suite passed (65 tests across
its unit/integration suites); the final App cleanup invocation passed all 14 cases
in 46.76 seconds, including its inert child-process helper. The subsequently added
periodic-maintenance entry-point regression passed separately in 4.80 seconds;
the combined 15-case suite was not rerun after that test-only addition. HTTP passed
all 80 cases, including the checked OpenAPI document. All-target Store/App/snapshot/
server Clippy passed after the production review corrections; App's final test
target also passed Clippy after the periodic case was added. Generated SDK
consistency, format/diff checks, strict OpenSpec (117 passed) and spec lint (0
errors/25 warnings) passed. Earlier native snapshot verification for this cleanup
implementation passed 163 tests with 5 ignored, before the final reference-index
migration and maintenance reader-transaction correction; that broader suite was
not repeated for those later Store/App changes. C sandbox deprecations and a linker
oversized-unwind warning were observed without unchanged-main origin attribution.
All Cargo test/Clippy commands used CARGO_INCREMENTAL=0 and --locked. No Linux,
Windows, macOS Intel, source-bound CI, full workspace or full SDK test invocation
ran for this cleanup increment. No goal task/issue is closed by these local checks.

A final incomplete-view audit corrected SQL NULL comparison semantics: a retained
native activity lacking its status field now protects cleanup as unknown rather
than falling out of the eligibility query. The expanded four-case independent
owner regression passed in 24.13 seconds and verifies unchanged retained event
bytes. All-target Store/App/snapshot/server Clippy passed again after this fix.
The full serial CLI branch/recovery suite passed 17 cases but failed its real PTY
preparation case after three HTTP 409 review responses; the driver previously
labelled every such response as busy without retaining its error. Investigation
now preserves bounded review error bodies and terminal diagnostics without logging
authorization headers. This failure is not attributed to unchanged main and the
cleanup increment was not represented as passing full CLI acceptance at that point.

With review-body diagnostics enabled, the focused real-terminal case passed both
boundaries in 79.89 seconds. This does not establish the cause or resolve the full
invocation's intermittent refusal. The driver now retries only a typed
SessionBusyError, allowing a bounded half-second pause between at most 20 fresh
read-only reviews while native inspection tears down. Any other refusal fails
with its bounded error/terminal evidence; mutation and exactly-once checks are
unchanged. Python compilation and the cognitive-complexity skill pass (maximum
13, complexipy 8.3.0, no analyzer warnings). The next full run again passed 17 cases and failed the PTY case in 244.84
seconds; its captured body identified the exact refusal as ConflictError with
message "Snapshot storage is busy". The bounded read-only retry now additionally
recognizes only that exact native-lock error; other conflicts still fail. This
is test synchronization, with production admission and mutation assertions
unchanged. The final complete serial invocation passed all 18 cases in 287.58
seconds, including both terminal process-death boundaries, with that correction.
The earlier failures remain recorded rather than attributed to unchanged main.

Review result for this ordinary-payload increment: no unresolved critical/major
code finding remains after the receipt-read, native-phase, incomplete-owner-view
and test-synchronization corrections. Existing snapshot ports and the application
Store adapter retain their project-specific dependency roles. The full recovery
feature still requires the acceptance gaps above; this is not approval of the
complete M6 feature or either issue. All final local checks are recorded above;
no PR/push or parent-task completion is claimed.

## Paired keep/rollback payload reclamation (2026-11-07, local)

The next M6 path removes original and resolution regular private stages together
only after the exact paired Store receipt, both native acknowledgements and the
parent's settlement link agree. It preserves both intents, immutable final slot
observations, worker/phase metadata, CAS images and lookup receipts. An untagged
internal receipt carrier retains ordinary version-1 serialization and aggregate
IDs; version-2 plans identify each leaf's exact original/resolution journal. Both
complete inventories are preflighted before any unlink, with both exited lifetime
locks, metadata pins and the shared namespace/Location guard retained. The Store
writer re-attests both owners and references around the bounded native callback.

Original leaves must match the resolution's immutable final private observations;
resolution leaves must match its own displaced originals. Missing leaves require
an already committed matching plan. Resume indexes frozen leaves by journal/name
rather than repeatedly scanning the full plan. Retention uses paired settlement;
zero/disabled settings, unknown native owners and incomplete proof still refuse.
An admitted original is required. A resolution with zero changes, as in keep,
must instead have an exact non-admitted phase with no origin; other resolutions
require admission. This does not generalize ordinary cleanup's positive phase
requirement or admit preparation construction, legacy evidence or managed Git.

The real CLI fixture kills a service after a partial source restore, performs
reviewed keep and rollback, and now reopens the stopped service's actual Store
for maintenance. It checks young retention, both independently held worker locks,
a durable plan cut before deletion, refusal on an unknown entry before any unlink,
synthetic progress of one planned unlink, complete retry, retained intent bytes,
unchanged later source/symlink/target and idempotent accounting. The plan cut and
single unlink are synthetic states, not additional cleanup process-kill claims.
The fixture restarts the actual service and verifies original request lookup and
explicit acknowledgement still work without inference or transcript movement.

Code-review skill inspection covered receipt serialization and legacy ID framing,
two-owner attestation, leaf/journal association and bounds, native phase admission,
full preflight, private metadata pins, no-follow unlinks/fsync and retained retry
receipts. The existing application policy owns database transactions; native
snapshot cleanup remains a filesystem adapter with no Store/source replay port.
Clippy identified test helper cognitive complexity 17/15 and 19/15; the fixture
was split by cleanup setup, interrupted-plan verification and lookup responsibilities,
without allowances. Native paired preparation/deletion passes the same threshold.

Initial local checks: the ordinary App cleanup suite passed all 15 cases in
43.99 seconds, and the real keep/rollback fixture passed in 81.50 seconds before
its final test-helper refactor. HTTP passed all 80 cases, strict OpenSpec passed
117 items and SDK generation consistency passed. Final source checks and the
complete serial CLI composition are recorded below when finished. This path does
not close M6: managed resource/Git cleanup, non-admitted original/preparation and
construction classes, CAS/metadata reclamation and the full native crash, lease
and platform matrix remain required. No PR/push or parent acceptance is claimed.

Review then tightened consuming-time inventory checks to allow only the durable
plan's leaves, rather than every possible stage index in the journal. A valid
resolution can have an intentionally empty stage (rollback recreating a deleted
file). The strengthened real-kill fixture includes such a file and injects owner
bytes into that empty index inside the consuming writer callback, after native
planning. Cleanup must report the unplanned entry before removing any planned
leaf, retain the injected bytes and resume only after explicit fixture cleanup.
This is a regression for that review correction, not an ownership/admission bypass.

Final local macOS arm64 evidence: the complete serial CLI branch/recovery run
passed all 18 cases in 346.57 seconds before the final inventory tightening. The
final focused keep/rollback case passed both actions and the strengthened empty-slot
regression in 84.39 seconds afterward; the full 18-case run was not repeated for
that paired-only correction. Ordinary legacy ID/serialization compatibility passed
separately in 5.37 seconds. All-target App/server/snapshot/CLI Clippy passed after
the final correction, with no complexity allowances. Format/diff checks, generated
SDK consistency, strict OpenSpec (117 passed) and spec lint (0 errors/25 warnings)
passed. HTTP's 80 cases passed before that native inventory correction. No full
snapshot-library/workspace/SDK execution, additional native platform or CI run was
performed for this increment. C deprecation/linker unwind warnings were observed
without unchanged-main attribution. All Cargo tests/checks/Clippy used
CARGO_INCREMENTAL=0 and --locked. No unresolved critical/major code-review finding
remains within this paired regular-payload scope; the broader acceptance obligations
listed above remain open and neither issue is complete.

## Cancelled prepared-target reclamation (2026-11-07, local)

The third M6 payload path handles positively cancelled pre-effect preparations
with a published origin, fully persisted prepared target inode identities, a
non-admitted version-2 native phase and an empty complete checkout set. It removes
only regular private target images. Intent, cancellation, lifetime/phase and request
metadata remain; source routes/images are never consulted for cleanup. Missing
intent/ownership, partial constructors, legacy proof, managed resources and Git
are not promoted into this admission and remain required/protected classes.

New cancellation review fingerprints bind the immutable prepared intent digest
from a metadata-only native lifetime port. The consuming writer rechecks it and
persists snapshot_preparation_intent_digest on the exact cancelled native activity.
A version-3 reclamation plan freezes that digest and every prepared target leaf's
original inode, current generation, BLAKE3 identity and length. The maintenance
writer matches the committed digest before unlinks, while namespace/Location/
lifetime guards and intent/cancellation/phase metadata pins remain retained. A
historical cancellation without this committed digest keeps its existing lookup
and acknowledgement path and cannot gain cleanup authority from current files.
Public preparation receipt/HTTP/SDK shapes are unchanged. Version-1/2 plans omit
the new optional plan digest field; their serialization and IDs remain stable.
Borrowed owner evidence avoids inventing ordinary-restore checkout receipts for
preparations. The larger preparation carrier is boxed without serialization change.

The actual CLI fixture holds a real SQLite IMMEDIATE transaction after observing
origin publication. It edits no row: writer contention prevents source admission
while the real worker writes all target images and persists prepared ownership.
The fixture observes that publication, kills the real service and verifies the
Store still positively records no source admission. Real reviewed cancellation
and explicit native acknowledgement then precede maintenance over the reopened
actual Store. The later source symlink/target, other bytes, transcript and inference
remain unchanged, and a new prompt still succeeds after cleanup.

Regression checks cover stale review after changing native intent, young retention,
a live original lifetime, replacement with identical bytes but a different inode,
a historical missing committed digest, changed intent after cancellation, unknown
entries, a committed plan cut before deletion, synthetic progress of one planned
unlink, retry/accounting and preserved intent/cancellation records. The latter two
cuts are synthetic cleanup states; the preparation service kill itself is real.
Lookup and metadata acknowledgement are exercised again after cleanup/restart.

Code-review skill inspection found that inode identity alone needed an intent
anchor in the committed cancellation pair. The native metadata port, version-3
review fingerprint and writer-bound digest check address that finding; current
native bytes never manufacture historical proof. Initial fixture assertions also
incorrectly required an explicitly serialized false acknowledgement flag; the
production serializer omits false and the fixture now respects that default.
Clippy identified test complexity 21/15 and 16/15; fixture setup, restart lookup,
plan interruption and digest checks were split by responsibility, with no lint
allowances. The Rust cognitive-complexity check uses workspace Clippy's threshold
15; the cognitive-complexity skill has no Rust analyzer. Existing project snapshot
ports and the application Store adapter keep filesystem and database ownership
separate. Full M6, native platform/cut/lease acceptance and both issues remain open.

Final local macOS arm64 checks: the complete 19-case serial CLI branch/recovery
invocation passed in 405.81 seconds, including prepared cancellation cleanup,
paired keep/rollback, original pre-effect cuts and terminal process-death recovery.
The focused digest-bound preparation case passed in 74.80 seconds before its final
test-helper refactor. All 15 shared App cleanup cases passed in 41.88 seconds and
all 46 runtime snapshot-worker tests passed in 4.85 seconds. HTTP passed all 80
cases in 4.09 seconds. Two initial library name filters selected zero tests; the
correct snapshot_work::tests filter produced the 46-case result above. All-target
App/server/snapshot/CLI Clippy, format/diff, generated SDK consistency, strict
OpenSpec (117 passed) and spec lint (0 errors/25 warnings) passed. All Cargo tests,
checks and Clippy used CARGO_INCREMENTAL=0 and --locked. C sandbox deprecations
and linker unwind warnings were observed without unchanged-main attribution.
No full workspace/snapshot-library/SDK test run, separate native platform or CI
execution ran for this increment. No unresolved critical/major review finding
remains in this fully prepared regular-target scope; raw construction, managed/Git,
metadata/CAS and full acceptance remain required. No PR/push or parent completion
is claimed.

### User Git index handoff evidence (2026-11-07)

A new real-repository regression reproduced a native backend gap: after preparing
an expected checkpoint with a 5 KiB tracked file and a 4 KiB untracked limit,
removing that file from the user's index still allowed the native restore. The
initial regression failed its refusal assertion; this is reproduced branch
behavior, not an unchanged-main attribution.

Native capture/preparation now binds the actual per-worktree index (including
absence), marker/admin routing and all bounded shared-index candidates into the
existing read-only metadata observations. User path discovery clears repository
redirection environment, disables optional locks, and uses the existing bounded
output/deadline/kill-and-await runner. The routing marker is observed before
resolution; resolved layout is checked again after index collection. The
synchronous native worker revalidates exact file observations and directory
identities without Git, index writes or source replays. A distinct v4 policy
fingerprint leaves older policy evidence unpromoted. The combined directory
context bound is eight so a linked worktree can retain its independent admin
identity alongside common/shadow/rule contexts. Existing 1 MiB per-file, 8 MiB
aggregate and 1024-file observation limits remain enforced.

The first split-index implementation used `git rev-parse --shared-index-path`.
Its new regression failed because this nominally read-only command refreshes
shared-index mtime/ctime even with GIT_OPTIONAL_LOCKS=0. A direct temporary
repository experiment confirmed that metadata change. Collection now enumerates
at most 4096 admin entries and binds shared-index candidates directly, avoiding
that command and leaving selection bound by the exact index bytes. This is a
conservative superset: an unused candidate changing can refuse native preparation.
Oversized observations retain ordinary canonical capture with explicit native
proof unavailability. A valid oversized index with an optional extension exercises
that fallback and verifies that capture preserves user index bytes.

Regression coverage includes post-preparation tracked-to-untracked changes,
absent-index creation, identical-byte inode replacement, shared-index dependency
changes, SHA-256 linked-worktree independent index changes, and successful split-
index preparation/restore preserving index/shared-index bytes, inode and mtime/
ctime. The linked-worktree positive also revalidates the retained policy after
native source restoration. Code-review skill inspection checked routing before
resolution, bounded subprocess cleanup, native verification without subprocesses,
old fingerprint handling and canonical fallback. No critical/major finding remains
in this index-handoff increment; it does not prove size-decision generation or
complete user config dependencies.

Local macOS arm64 validation: the full snapshot library run passed 126 tests with
four intentionally ignored subprocess fixtures in 44.16 seconds; the subsequently
added split-index metadata positive passed separately in 2.15 seconds. Integration
suites passed all 18 file-copy and 22 Git snapshot cases (one intentionally ignored
subprocess fixture). The Windows-only file_copy_platform binary selected zero
cases on this host and supplies no Windows evidence. Snapshot all-target Clippy,
format/diff, generated SDK consistency, strict OpenSpec (117 items) and spec lint
(0 errors/25 warnings) passed. Cargo invocations used CARGO_INCREMENTAL=0 and
--locked. No full workspace, CLI/TUI acceptance rerun, SDK execution, other native
platform or CI run was performed for this increment.

Managed Git source admission stays disabled. Fresh generated large-untracked
classification, complete user config/filter compatibility, tracked-large-file
limits, runtime/checkout/recovery/cleanup and full crash/platform acceptance still
require implementation or proof. M2–M8, parent tasks and both issues remain open;
there is no push/PR/merge acceptance claim.

### Generated size-exclusion revalidation (2026-11-07)

The new shrink-after-preparation regression failed before this change: a regular
untracked file omitted above 4 KiB could shrink to a small eligible file while the
native planner continued using its old literal ignore pattern and restored another
source path. No unchanged-main attribution is made for this reproduced branch
behavior. Generated-rule observation now returns the size-excluded names decoded
from the exact recipe whose bytes/output/source proof is verified. IgnoreInputs
retains those names separately from ordinary ignore patterns. A v5 policy frame
includes the new inputs rather than upgrading older checkpoint evidence.

Rules checks each size-excluded name at open and final capture/planning revalidation.
Parents are opened component by component without following links; leaf metadata
must still describe a regular file whose length exceeds the configured untracked
threshold. Omitted file contents are never opened or hashed by these checks. The
native planner rejects missing/smaller files, links, linked parents, directories
and FIFOs before intent/source admission. Bytes can change while the file remains
oversized; normal restoration of independent eligible paths preserves that file.
A fresh capture regenerates rules and includes a newly eligible raw image, while
historical target/recorded omissions continue to prevent fabricated deletion or
restoration. This keeps current expected validation distinct from historical
mutation restrictions.

Real-repository tests cover the reproduced shrink, exact-threshold change,
deletion, leaf/parent links to a preserved external oversized target, directory and
FIFO substitution after an initial successful inventory. Each late inventory,
preview and restore refuses and leaves source/history intact. Positives cover
nested literal bracket names, a still-oversized owner edit and a fresh expected
capture retaining the newly eligible image without rewriting it during historical
restore. Code-review skill inspection checked recipe/input byte binding, bounded
name/path handling, descriptor traversal, metadata-only omitted-file observation,
fingerprint compatibility and positives rather than universal refusal. No
critical/major finding remains in this size-revalidation scope.

The full local macOS arm64 snapshot library run passed 131 tests with four
intentionally ignored subprocess fixtures in 48.57 seconds. Strict OpenSpec passed
117 items, spec lint reported 0 errors/25 warnings and SDK generation consistency
passed. Complete check results for this increment follow below.

This does not yet bind original size-classification generation to the exact user
index/config inputs; collection-time state equality alone cannot manufacture that
provenance. A temporary repository experiment also confirmed that a private admin
copy of index/shared-index files can isolate Git's split-index timestamp refresh
(the original shared file remained unchanged, the copy changed), but complete
configuration/conditional-include fidelity is required before adopting such a
classifier. That experiment is not implementation or acceptance. Tracked-large-file
limits, full configuration/filter compatibility, managed Git runtime/checkout/
recovery/cleanup and full crash/platform evidence still gate source admission.
M2–M8, parent tasks and both issues remain open.

Final checks for this increment: snapshot all-target Clippy with -D warnings,
format and diff checks passed. Public integration passed 18 file-copy cases in
1.09 seconds and 22 Git snapshot cases in 13.92 seconds (one deliberately ignored
subprocess fixture). The Windows-only platform binary selected zero cases on this
host; it supplies no Windows evidence. All Cargo tests/Clippy used
CARGO_INCREMENTAL=0 and --locked. No full workspace, CLI/TUI acceptance rerun,
SDK execution, Linux/Windows/macOS Intel or source-bound CI run was performed.
The GitHub refresh still reports #93 and #92 open and origin/main remains
`db9e171e`, already contained in this branch. PR #115 concerns model fallback and
is outside this recovery increment; it was left untouched. No goal PR/push/merge
or parent completion is claimed.

### Bound size-classification generation (2026-11-07)

A new real-repository test reproduced another missing guard: changing the user's
classification config after native preparation still allowed source restoration.
Its initial refusal assertion failed on this branch; there is no unchanged-main
attribution. New version-2 generated-rule recipes retain a classification digest
covering the observed actual user index/routing, resolved user configuration and
rule observations. Fresh capture/preparation repeats bounded classification and
requires both matching digest and exact skipped names before granting proof.
Version-1 recipes remain canonical-readable without invented classification proof.
The native policy frame is v6, so older checkpoint proof is not promoted.

The classifier materializes only observed index/shared-index bytes in a private
temporary admin directory. Active config includes are resolved in the user's real
branch/gitdir context; ordered effective values (including null/empty and quoted
values) are serialized into a private config without re-evaluating includes there.
Global/info excludes are copied from bound observations, and source rule reads
remain no-follow. Config secrets stay out of command arguments and checkpoints.
The generic user-config profile retains roots, active/absent includes, HEAD and
physical context without applying the private shadow's empty-reference profile to
a real user repository. GIT_CONFIG is now cleared with repository redirects: it
redirects config commands rather than ordinary repository reads and could otherwise
make internal setup modify an unrelated file. A subprocess regression verifies
native capture/restore and preservation of that external config.

Private copied files use create-new/no-follow mode 0600 beneath a private temporary
directory. They are verified again after Git reads them. Only disposable shared-
index timestamps may refresh; byte digest, inode, stable attributes and physical
route still have to match. Original index/config inputs retain strict generation
verification. Rule reads now bracket content reads with generation checks, and
classification retains both bytes' hashes and generations. Directory marks guard
the collection window but stay out of checkpoint fingerprints and native handoff
verification, preserving harmless empty-directory changes. Existing native
namespace validation still runs. Blocking rule scans, metadata census and private
index materialization run outside async executor threads. Metadata copies retain
the existing 1 MiB/file and 8 MiB/aggregate input bounds; rule traversal, output,
entry count and process deadlines remain bounded. The canonical fallback's Git
listing now uses the bounded kill-and-await runner too.

Regression evidence includes original-config changes, effective gitdir/onbranch
includes in their original context, null/quoted/multiline config round trips,
disabled global excludes and original split-index metadata/bytes preserved across
capture, preparation and native restoration. An isolated real Git wrapper changes
the original index or the disposable private config after classification has read
its inputs. Both cases retain canonical-readable snapshots with explicit missing
native proof and no native journal/source effects. The original-index case still
captures its now-tracked large file; private-copy tampering retains the ordinary
size omission. This wrapper injects deterministic interference; it is not an OS
process-death cut.

Review corrected an overly strict first pass that rejected harmless new empty
directories. Generation checks are retained where they detect collection races,
while native planning uses existing eligibility/namespace rules. An older rule-
change test now keeps a prepared handoff before the edit to exercise native
refusal. A rule changed and restored to identical bytes has a new generation and
requires a fresh expected capture; its positive then restores correctly. The
conditional-include fixture initially used a trailing slash matching descendants
rather than the exact gitdir, and was corrected with an actual Git selection
assertion. A test initially attempted process-global environment mutation, rejected
by the workspace unsafe-code lint; it now derives the root from the child command's
existing worktree environment. Clippy's collapsible-if finding was fixed without
allowances. Code-review skill inspection covered original-vs-private proof
separation, no secret argv, recipe/version compatibility, bounded child cleanup,
no-follow paths, private-copy interference and preservation of positive behavior.

Local snapshot-library acceptance passed 136 tests with six intentionally ignored
subprocess fixtures in 85.64 seconds. A subsequently added disabled-global-excludes
positive passed separately in 4.24 seconds; production code is the same. Strict
OpenSpec passed 117 items. Remaining check results are recorded below. All managed
Git source admission and M2–M8/parent acceptance remain disabled/open. Full general
configuration/ABA/dependency and sparse/filter/limit combinations, native runtime/
checkout/recovery, partial construction, cleanup of metadata/CAS and orphan private
classification directories after process death, and full native platform/cut
acceptance remain required. These checks do not prove the two issues complete.

Final namespace review also requires an exact disposable admin inventory before
and after Git: only the constructed regular files and directories may exist, and
links/unknown routing files refuse without traversal. The interference fixture now
also inserts an unexpected commondir file, and removes its own output scratch file
before verification so that each intended guard is exercised independently. The
copied-file proof retains stable inode/attribute identity separately from full
metadata generation; its shared-index timestamp exception is used only on these
private copies, never on original user inputs. No unresolved critical/major finding
remains in this bounded classification increment; broader compatibility, process-
death cleanup and activation acceptance remain open as listed above.

The final complete package invocation passed all 137 library cases (six ignored
subprocess fixtures), 18 file-copy integration cases in 1.23 seconds and 22 Git
integration cases in 23.58 seconds (one ignored subprocess fixture). Library time
was 89.01 seconds. The Windows-only platform binary and doctests selected zero
cases on this host and provide no additional platform/doc execution evidence.
Final all-target snapshot Clippy with -D warnings, format/diff checks, generated
SDK consistency, strict OpenSpec (117 items) and spec lint (0 errors/25 warnings)
passed. All Cargo tests/Clippy used CARGO_INCREMENTAL=0 and --locked. No full
workspace, CLI/TUI rerun, SDK execution or source-bound native CI/platform matrix
ran for this increment. No push/PR/merge or parent/issue completion is claimed.

A final compatibility regression then reproduced changed checkpoint IDs after
adding only empty directories. Classification retained absent .gitignore bindings
for those new directories in its serialized digest. They now stay available for
verification but are filtered from serialization using the same source-absence
rule as native policy capture. This preserves both ordinary checkpoint identity
and classification identity while retaining mutation detection. The regression
failed before that correction and passes afterward; no baseline attribution is
made. The final package rerun supersedes the earlier counts above: 138 library
cases passed in 86.93 seconds, plus 18 file-copy cases in 1.14 seconds and 22 Git
cases in 23.56 seconds. Six library and one integration subprocess fixtures remain
intentionally excluded from direct top-level selection. Windows-only/doc binaries
again selected zero. Final all-target Clippy, format/diff, strict OpenSpec (117),
lint (0/25) and SDK generation consistency passed. No further scope or acceptance
claim changes: source admission and parent tasks remain open as stated above.

### Actual Git runtime acceptance candidate (2026-11-07)

The adapter already contained a Git branch in restore_owned, but its availability
gate made it unreachable. A non-default native-git-acceptance Cargo feature now
exercises that same production branch through the real App, runtime-issued workers,
Store settlement and acknowledgement. It is forwarded by cyber-app for acceptance
invocations. Normal builds and the release workflow (which does not select all
features) retain the existing Git gate. The marker-only candidate availability
check creates no project identity cache and cannot substitute for later Location,
source/image, lifecycle, original-worker or checkout validation. Private roots and
disabled snapshots still refuse. No public API/schema or force-clear path is added.

Owned Git preview and restore now use a distinct bounded raw-image limit (up to
64 MiB and the combined budget) rather than applying max_file_bytes to tracked
files. That setting continues to govern Git untracked omission. Non-Git options
retain their old limit. The new App fixture uses a binary tracked image above
2 MiB with an explicit 4 KiB omission threshold, a link and an independently omitted
8 KiB owner file. Real prompt capture, rewind preview/confirm and native Store
receipts restore the tracked bytes/link while preserving that omitted file and the
user index/config. The receipt retains complete versioned lifetime/checkout proof;
these cases use the complete empty resource set, not managed-checkout acceptance.

The first App invocation reached both SHA-1 root/scoped restore/receipt/reconcile
cases, then failed its root-vs-scope lock assertion for SHA-256. Workspace
coordination used only libgit2 discovery and therefore chose different domains when
that format was unavailable to libgit2. A read-only ancestor .git marker fallback
now conservatively widens the process-local lock domain. It does not grant Git
validity, permission or source authority and does not run Git or create identity
caches. A dedicated default-build regression verifies one shared live lock for a
SHA-256 root and scope and no cyber-project-id creation. The subsequent first full
App invocation passed its three cases in 55.81 seconds; the later reopened-App
extension and final results follow below.

The runtime fixture replaces a restored file with an absolute symlink to later
owner bytes, repeats committed lookup/explicit acknowledgement, and compares
messages and inference counts. It then shuts down the actual runtime, constructs
a fresh App over the same Store and repeats only lookup/metadata reconciliation.
This is an App shutdown/reopen, not an OS process-death cut or lost writer-reply
injection. Existing immutable receipt and native ownership gates remain in force.
The native CI workflow now schedules both default snapshot proof and separate
candidate-adapter/App invocations on Linux x64/ARM and macOS Intel/ARM and retains
separate logs. That workflow edit is not evidence that those jobs ran.

Clippy caught an inefficient double-ended iterator and a 24/15 test helper;
receipt roundtrip, source preservation and reopened-runtime responsibilities were
split without lint allowances. Code-review skill inspection checked default-vs-
candidate behavior, release feature selection, no metadata writes from availability
or coordination, backend image limits, complete resource proof, exact receipt
consumption and no replay after later owner edits. Managed worktrees in the default
application data layout still need resource-verified storage-root integration:
copy::Store currently rejects Locations under its data root, and the shadow's broad
data exclusion also needs reconciliation for those legitimate source trees. The
new empty-set cases do not prove that support. Keep/rollback after actual Git
partial replacement, all native cuts, cleanup of managed/Git payloads, complete
configuration/filter/limit compatibility and source-bound platform acceptance
remain required. No M2–M8 or parent task is accepted by this candidate increment.

Final local macOS arm64 checks: the reopened-App acceptance test passed all three
parameter cases in 47.32 seconds. The default snapshot package passed 138 library,
18 file-copy and 22 Git integration cases; the candidate package passed the same
138/18/22 cases. Their library times were 60.47 and 87.90 seconds, respectively.
Both modes intentionally exclude six library and one integration subprocess
fixtures from top-level selection. Windows-only/doc binaries select zero on this
host. All three workspace tests passed in 0.04 seconds, including the dedicated
SHA-256 regression, and all 46 snapshot-worker regressions passed in 4.70 seconds.
A later lint-only user-message iterator change uses rfind; the App execution above
preceded that equivalent helper change. Final candidate App/server/snapshot
all-target Clippy, format/diff, SDK generation consistency, strict OpenSpec (117)
and spec lint (0 errors/25 warnings) passed. Workflow YAML syntax was checked with
Ruby's YAML library after Python PyYAML was unavailable. Every Cargo test/Clippy
invocation used CARGO_INCREMENTAL=0 and --locked. C sandbox deprecations and a linker
unwind warning were observed without unchanged-main attribution. No full workspace,
CLI/TUI acceptance rerun, SDK execution or source-bound CI/other-platform run was
performed. No push/PR/merge, default activation or parent/issue completion is claimed.

### Managed checkout source within default application data (2026-11-07)

Starting from b2f35a23, the actual App `/worktrees` default layout reproduced
`Private owner state cannot be a snapshot Location` during rewind preview.
Registration now permits only observation/private metadata reservation for its
ready source subtree; other data, the snapshot namespace and explicit private
roots remain protected. The native Store retains and revalidates registration.
Source-intent construction additionally requires the actual runtime-issued worker
and its verified complete native checkout set, matching activity, Session creation,
Location and registration. Registration alone cannot admit an unowned restore.
Git raw capture protects snapshot storage without silently excluding the registered
source subtree. No default Git source gate is enabled.

The first application run after that correction stalled during managed native
admission. A live process sample showed no executing native worker; a bounded
confirmation regression then reproduced the timeout. Checkout policy lookup needs
the same Session mutex held by the admission caller. Ordinary restore and recovery
now pass an immutable captured Session revision across host admission without
retaining that mutex. Admission checks the current Store revision before reserving
native ownership; existing worker per-effect and consuming writer checks remain.
The stale-revision regression performs a real Session rename and proves refusal
publishes no native events and releases the admission flag.

The candidate App test now creates a real managed linked checkout through the
public route in the default data directory, runs two scripted prompts, previews
and restores a tracked file, and checks the actual non-empty native checkout
resource publication against the consumed receipt digest. User index/config bytes
remain unchanged. Repeated explicit acknowledgement, including a fresh App over
the same Store, preserves later source symlinks/owner bytes, messages and inference
counts. This is App reopening rather than process death. The managed case and
three existing unmanaged root/scoped SHA-1/SHA-256 cases passed (two top-level
tests, 56.40 seconds). The first passing managed-only run took 15.54 seconds before
the final resource/reopen assertions were added; the final full App run includes
those assertions.

Native registration tests prove unregistered under-data roots and explicit private
roots refuse, registered observations work, other data/link escapes remain private,
unowned journal preparation cannot publish an intent or mutate source, and changed
readiness invalidates the retained Location. All 47 worker tests passed in 4.69
seconds. The default snapshot package passed 139 library, 18 file-copy and 22 Git
cases (library 60.63 seconds, Git 16.35 seconds); six library and one integration
subprocess fixtures were intentionally excluded from top-level selection. The
Windows-only and doc binaries select zero on this host. App/server/snapshot
all-target candidate Clippy passed with warnings denied. SDK generation consistency,
format/diff, strict OpenSpec (117 items) and spec lint (0 errors/25 warnings) passed.
Cargo test/Clippy commands used CARGO_INCREMENTAL=0 and --locked. Native sandbox
deprecations and the linker unwind-size warning were observed without main
attribution. The candidate snapshot package also passed 139 library, 18 file-copy
and 22 Git cases, with the same intentional fixture exclusions and zero selections;
its library/file-copy/Git times were 61.70/1.17/16.38 seconds. Both package runs
include the final native registration regression.

Security review followed the code-review skill over the changed runtime/native
paths and their tests: the exception is registration-bound, source admission uses
retained native proof rather than IDs alone, and private boundaries and exact
metadata-only acknowledgements remain enforced. Full managed partial replacement,
keep/rollback/process cuts, partial construction/reclamation and native platform
acceptance remain open. No full workspace, CLI/TUI acceptance rerun, SDK execution,
source-bound CI, other-platform execution, push/PR/merge or parent/issue acceptance
is claimed by this increment.

## Non-Git rename, unsupported entry and concurrent editor tests (2026-11-14, local)

Test-only additions for task 3.4; no product code changed and no defect was found.

- `crates/cyber-snapshot/tests/file_copy.rs`
  - `rename_is_restored_as_delete_plus_create_and_preserves_unrelated_owner_edits`:
    a captured `a.txt` (mode 0640) renamed to `b.txt` is restored by deleting
    `b.txt` and recreating `a.txt` with its mode; when the owner edited `b.txt`
    after the rename the whole restore refuses with `Conflict(["b.txt"])`,
    neither name changes and no journal remains. An unrelated owner edit survives
    both cases.
  - `fifo_socket_and_device_entries_are_disclosed_unsupported_and_never_restored_or_deleted`:
    FIFOs and a bound Unix socket are reported as `unsupported: <path>` in the
    capture and in `exclusions`, are absent from the preview, and a bounded
    restore neither opens a FIFO, recreates a removed FIFO nor deletes or replaces
    (same inode and type) the current FIFO and socket. Device nodes take the same
    non-regular branch but are not created here (they need privilege).
  - `a_concurrent_editor_write_between_replacements_refuses_the_rest_and_keeps_the_edit`:
    the admission callback parks the restore after the first actual replacement;
    the test writes the not-yet-replaced `b.txt`, resumes, and gets
    `Restore native ownership conflicts: b.txt`. The journal is retained
    incomplete with `applied == ["a.txt"]` and the displaced `a.txt` preimage,
    later capture reports `requires recovery`, and the editor bytes survive.
- `crates/cyber-cli/tests/branches_cli.rs`
  `non_git_rewind_with_rename_and_delete_round_trips_through_cli`: through the
  real binary in a Location without `.git`, the agent renames `a.txt` and deletes
  `b.txt` with a shell `apply_patch` heredoc; a reviewed `sessions rewind --target
  both` recreates both and removes `renamed.txt`, and `sessions checkout <message>
  --code` undoes it without inference while an unrelated owner file is kept.

Verification on local macOS arm64: `cargo nextest run --profile ci -p cyber-snapshot
--test file_copy` (34 passed) and `-p cyber-cli --test branches_cli` (23 passed);
each new test passed three consecutive runs. Platform acceptance (Linux x64/ARM,
macOS Intel, Windows limits), remaining crash cuts and native filesystem privacy
remain open, so 3.4 and the M2–M8 parent obligations are not accepted.

## Cap visibility and staged/branch retention through real clients (2026-11-14, local)

Test-only additions for task 3.3; no product code changed and no defect was found.

- `crates/cyber-app/tests/support/snapshot_cap_retention.rs` (in the
  `snapshot_reclamation` binary)
  - `lowered_cap_disables_code_rewind_visibly_and_keeps_conversation_rewind`:
    with `snapshots.fallback_max_bytes = 1` in the App configuration, `code`,
    `both` and a repeated `code` preview all fail with
    `Non-Git snapshot storage cap reached; conversation rewind remains available`
    (the repeat comes from the recorded unavailable reason). A conversation-only
    preview has no `code` section and its confirmation removes the active
    messages; source bytes and inference count are unchanged.
  - `cap_pressure_retirement_frees_only_acknowledged_payloads_then_capture_succeeds`:
    two settled restores share one Location. With neither past retention, cap
    pressure refuses capture and retires nothing. After only the first is aged and
    its payload reclaimed, cap-pressure capture succeeds by retiring that
    metadata; the second restore's journal files are byte-identical and it has no
    reclamation record.
  - `collection_preserves_staged_undo_and_inactive_branch_roots_past_retention`:
    all events, manifests and objects are aged past 7 days. The roots added by an
    inactive branch's second turn and by a staged code undo (an unrelated owner
    file makes its baseline unique) keep their manifests, as does every other
    referenced root. An untracked capture's manifest survives the first pass,
    while one journal remains, and is removed on the second pass once none
    remain. Each periodic pass settles one journal per Location. The staged undo
    then clears, restoring the staged file and keeping the owner file.
- `crates/cyber-tui/src/view/rewind.rs`
  `a_cap_refused_code_preview_shows_the_reason_and_the_conversation_only_hint`:
  a code preview refused with the cap error renders
  `Code unavailable/conflicting: <cap reason>` and the conversation-only hint in
  the choose phase.

Verification on local macOS arm64: `-p cyber-app --test snapshot_reclamation`
(48 passed), `-p cyber-snapshot --test file_copy` (34 passed), cyber-snapshot
`collection` (20 passed) and cyber-tui `rewind` (12 passed); each new test passed
three consecutive runs. Crash cuts at the writer/journal/reclaim boundaries and
platform results remain open, so 3.3 and M6 are not accepted.

## Rule approval fencing at settlement (2026-11-14, local)

Nested rule discovery resolves the rule gate after the successful effect, but the
settlement only rechecked the Location, Context Epoch and native receipts. A trust
revocation or re-review between discovery and commit still delivered and consumed
the rule. Each discovered project rule now keeps the directory its approval was
checked from; under the Session settlement lock (group settlement and exact
`/approve` replay) the runtime re-resolves the Location's rule gate and keeps the
nested reminder only when every project binding still authorizes with the same
checkout root, approval generation and definition digest. Otherwise the tool
result settles without the reminder and no rule identity is recorded.
Owner-global bindings need no approval. `Runtime::configure_settlement_probe` is a
hidden test seam that runs after discovery and before the settlement lock.

Evidence: `a_binding_from_a_revoked_generation_is_not_authorized` (unit) and
`a_rule_revoked_while_its_read_settles_is_neither_delivered_nor_consumed` (real App,
scripted provider), which revokes trust in that window, then re-approves and sees a
later read deliver the rule once. With the recheck disabled the integration test
fails at the delivery assertion. `cyber-server` library (474) and `auto` (12) tests,
`cyber-app --test instructions` (20), Clippy with warnings denied, strict OpenSpec
and spec lint passed. Withdrawal of rules already delivered in the Epoch,
revocation/restart acceptance and platform acceptance remain open.

## Rule leakage, precedence and estimate consistency tests (2026-11-14, local)

Five real-App tests with the scripted provider close the remaining 4.4 test cases
except platform acceptance: directory precedence and Claude opt-out from a nested
Location; no reuse or supply of rule delivery across a worktree move; child and
cut-fork Sessions start their own delivery record without changing the parent's;
budget-truncated reminders record only the rules they label (body or whole list
entry); and inspection estimates rebuilt from the provider's actual request body.
`last_prepared_request` matches the sent request exactly, including the rule
reminder. `next_request` adds two separately rounded parts and sizes the stored
projection with an empty tool list (2 bytes), so it can differ from the request
sent by one token; the test pins that composition. No product change was needed.
Evidence: `cyber-app --test instructions` (29) and `cyber-server --test
context_compaction` (27) under nextest `ci`, Clippy with warnings denied. Platform
acceptance remains open.

## Documentation and evidence reconciliation (2026-11-14, local)

This round's increments on `feat/managed-recovery-owner` are recorded in the task
notes for 4.1–4.4 and 6.2–6.4; this entry only reconciles them with the guides.

Delivered locally (no task checked off; every one still needs native platform
acceptance):

- 4.2: rule delivery fenced to the current approval generation at settlement
  (entry above); restart/replay coverage over a file database with a second App
  (three `*_restart` tests in `crates/cyber-app/tests/instructions.rs`). This
  supersedes the "revocation/restart acceptance" item left open above.
- 4.3: inspection lists delivered rules with their current approval
  (`current`/`changed`/`revoked`/`unknown`) and withheld rule definitions by
  reason, without opening either; `cyber doctor` counts scoped rules and lists
  withheld ones. OpenAPI/SDK were regenerated in those commits.
- 4.4: leakage, precedence, truncation and estimate-consistency tests (entry above).
- 6.2/6.4: descendant requests stamped with the root input cohort; the JSONL
  driver owns background-subagent and `PreToolUse` hook asks; scoped interruption
  stops an owned background subagent subtree; stale, wrong-kind and cross-driver
  replies are refused.
- 6.3: secret-redacted JSONL `error`/`result`/`end` fields. An earlier
  `--include-hook-events` commit here was reverted in favour of the separate
  `add-exec-hook-events` change (PR #132).

Guides updated to match: `docs/features.md` (Project instructions, Driven exec)
and `docs/local-core-guide.md` (settlement fencing and restart behaviour replace
the stale "remain in progress" list; per-source sizes in `/context` are no longer
listed as missing; descendant hook receipts are listed as not reported; the
background-descendant paragraphs no longer call delivered work open). Generated
artifacts were checked, not changed: `cyber help-markdown` equals `docs/cli.md`,
`openapi_document_is_current` passes and `scripts/generate_sdk.py --check` is clean.

Still open in code: withdrawal of a rule already delivered in the current Epoch
after revocation or change; withheld non-rule instruction files as typed entries;
foreground-subagent ownership; acknowledged interruption of a subagent with an
in-flight tool call; Session-scoped and descendant hook receipts; user-configured
redaction patterns on JSONL lines and the raw provider message in
`session.step.failed`.

Externally blocked:

- Product decision: how a revoked or changed delivered rule leaves the active
  context (elide on replay, or force an Epoch rebuild/compaction).
- Product decision: whether an attached JSONL driver continues across a server
  restart.
- Product/security decision: the removed-Location maintenance and
  construction-evidence cleanup protocol revision (not covered by the 3.0 approval).
- Product decision: enabling default managed Git source admission.
- Native platform CI (macOS Intel/ARM, Linux x64/ARM, Windows host) on the final
  pushed source for 4.1–4.4, 5.4, 6.2–6.4 and the managed-recovery tasks.
