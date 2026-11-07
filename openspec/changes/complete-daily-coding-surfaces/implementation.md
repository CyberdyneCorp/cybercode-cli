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
