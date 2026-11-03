# Binary workspace implementation progress

Baseline: main `6099e1327a79f32ed33fd5f418f1123f5ed0bded`. On 2026-11-01 the
user prioritized issue #94 ahead of the unfinished first two workstreams. Their
scope and the editor's shared integration requirements remain unchanged.

## Current increment: live byte viewing

- `cyber-core::binary` owns exact decimal-string offsets, pinned regular-file
  descriptors/directory identities, observed generations, bounded pages and literal
  search. It performs no writes. No whole-file hash/copy delays the first page.
- The runtime owns a bounded document registry shared by its routers. Owner routes
  refresh Session/Location/read permission on every read, revalidate native bindings
  after permission waits and IO, and use bounded blocking workers. Close and Session
  deletion release clean views; obsolete opened UI replies issue explicit close.
- The TUI supplies a virtualized hex/ASCII viewport, integer/float/bit inspector,
  exact goto, bounded literal hex/UTF-8 search, named bookmarks, byte copying and
  configured bindings. `/hex`, explorer `x` and `cyber hex` start no model Turn.
  Session/Location changes clear cached content. The direct command binds its
  expected project root; it never rebinds an existing Session.
- Generated OpenAPI/SDK operations plus `client.binary.open()` use lossless offsets,
  bounded decode and page evidence validation. Existing text content routes stay
  compatible.

Live revisions are observed metadata, not immutable content evidence. Live search
is best-effort. It checks cancellation and elapsed scan budget between bounded IO
chunks, without claiming that the OS can preempt a blocked filesystem read.

## Verification and native matrix

The unchanged-main browsing baseline ran before implementation: `cargo test -p
cyber-core files::tests --lib`, 13 selected tests passed on macOS arm64. The filter
also selects matching configuration/import module names; it is not 13 dedicated
binary tests.

The core cases cover exact JSON offsets through u64; every byte value/empty/EOF
pages; sparse positioned reads above the edit ceiling; file/parent/alias replacement;
search overlap across chunks and requests; cancellation, resource limits and path
admission. A macOS-native sparse-file test reads and searches four bytes at the odd
offset 9007199254740993, proving native IO as well as lossless transport. A Linux-only
procfs regression ensures that a stat-zero virtual regular file cannot return bytes
outside the observed length; this case requires Linux execution.

The current production App file-route suite passes 16 tests, including approval-wait
source swaps, canonical alias permissions, policy revocation during an approval wait,
owner auth/Origin, read-denied/edit-allowed, cross-Session IDs, per-Session/global
capacity and cleanup after Session deletion. Close requires the matching stored
Session but no old Location/read permission; it discloses no bytes or metadata.
The policy-wait regression was checked with a negative control: disabling only the
current-policy denial check returned HTTP 200 instead of the required 403. The
check was restored and all 16 tests passed again.

TUI integration uses production owner routes and real byte fixtures. Snapshots
cover wide/narrow/light/NO_COLOR; tests cover permission-overlay priority, obsolete
reply cleanup, repeated reopen and Session-change disposal. The client checks page
bounds before decoding and validates canonical encoding, offset, revision and digest.
Current macOS checks pass: 325 core unit tests, 626 TUI unit/integration tests and
16 production App file-route tests. Three TUI cases are explicitly ignored: one
child-process test runs through its parent; two require Linux protected-file
fixtures to avoid local native vault writes. Earlier CLI unit/reference checks
passed 78 tests; CLI parsing and generated documentation did not change afterward.

A real macOS arm64 PTY run used a 9007199254741249-byte sparse file (8192 bytes
allocated) on a 24 GiB machine. It navigated to the odd offset 9007199254741245,
rendered the final bytes and integer inspector, and made zero inference requests.
First view took 7.649 seconds including fresh private app/TUI/server startup; RSS
at first view was 248544 KiB, not a peak measurement. The unoptimized executable
used SHA-256 `842da52ae8d9e66dc8d93b97c50040be0af4645c8fe0e095a25077db815e1093`;
its Rust sources match commit `39cdcf7f`, including the final authorization,
disposal and page validation refinements. This is
local viewer evidence, not a current-head platform/performance acceptance result.
The [terminal frame and measurements](../../../docs/evidence/binary-view/README.md)
retain the executable identity and limitations.
The 16 MiB editing benchmark, input/cancellation latency and peak RSS remain open.

The new SDK suite typechecks; its complete unit/mock suite passes 121 tests with
ten external end-to-end tests explicitly skipped without `CYBER_BIN`. The new
binary-specific end-to-end test also passed separately against the rebuilt executable:
every byte, literal search/EOF, cross-Session refusal, stale-file refusal, repeated
close/reopen capacity release and zero inference requests. The nine unrelated
external SDK tests were not run locally. The cognitive-complexity skill's TypeScript analyzer
reports maximum score 5 for the new helper. Rust uses the repository's Clippy gate,
since the skill does not provide a Rust analyzer.

Local execution so far is macOS arm64. Linux/Windows native view/search results
remain unverified until current-head CI executes them. The Windows nextest profile
includes the production App binary file cases, all core unit tests and TUI tests.
Native replacement, displaced-preimage and durable save/recovery support is not
yet implemented or proved on any platform. No write support matrix is accepted.

## Remaining work and acceptance

All original unchecked tasks remain open: live view primitives do not complete
edit/durable document authority or immutable search. Next prove private immutable
base preparation and native exchange/replacement/recovery, then implement expected-
byte splice batches, draft undo/redo, stable review and idempotent save/restore.
Dirty drafts and unknown receipts must survive restart. Complete owner mutation
routes, model operations with read+edit/Mode/plugin intersections, selection/mouse/
accessibility and visual polish, native terminal captures, performance measurement,
Git/non-Git snapshot retention and acknowledged FileChanged integration.

This increment does not reduce issue #94 to a viewer and does not complete the
combined goal. Native proof and actual acceptance continue to gate write paths.

## Review and local checks for this increment

Substantive code/security review used the code-review skill across core IO, runtime
ownership, owner routes, CLI entry, TUI state/rendering, SDK helpers, generated
contracts, snapshots and documentation. The project keeps native byte IO in core,
authority/ownership in the server and rendering separate from TUI state/requests;
no unrelated architecture refactor was required.

Resolved findings: policy changes during an approval wait; obsolete UI replies and
Session switches leaking document slots; permission overlays hiding or retiring a
view; encoding work on the event loop; pages exceeding observed file length; and
client decoding/validating page evidence too late or incompletely. Tests exercise
these boundaries, including the policy-wait negative control described above.
The SHA-256 dependency is a normal TUI dependency, not only a test dependency.

- Rust: core 325 passed; TUI 626 passed, three explicit ignores; production App
  file routes 16 passed. Earlier unchanged CLI definition/reference suite: 78 passed.
- Clippy: all targets for core/server/TUI/CLI/App passed with Rust warnings denied.
  The native compiler emitted macOS sandbox API deprecation warnings; test linking
  also reported a large `__eh_frame` compact-unwind warning. No origin is assigned.
- SDK: typecheck passed; 121 unit/mock tests passed and ten external real-binary
  SDK tests skipped in the mock/unit run because it did not set `CYBER_BIN`;
  the new binary-specific external test passed separately with the rebuilt executable.
- Generated OpenAPI test passed; SDK generator check and Rust formatting passed.
- Strict OpenSpec 1.13.2: 114 items passed, zero failed. Spec lint: zero errors,
  25 warnings. TypeScript helper cognitive complexity maximum: 5, remeasured after
  the final bounds refinement.
- Linux/Windows native cases and repository-wide remote CI remain pending. This
  is review of a live-view increment, not acceptance of the complete editor.

## PR #95 Windows regression and README capture

The Windows job at head `abf43ad1` passed 3,807 tests and failed the new parent
replacement test before its validation assertion: renaming a retained directory
returned `ERROR_SHARING_VIOLATION` (32). The test assumed Unix rename semantics.
Production pinning remains unchanged. The regression now distinguishes Unix
replacement detection from Windows prevention: the native Windows case requires
rename refusal, verifies the original bytes, closes the document, then requires a
successful rename and reopening of the replacement with the original preserved.
The Windows test remains selected by the existing core nextest profile.

Local macOS verification after the change: nine binary core tests passed and
core all-target Clippy passed. The screenshot harness's 11 tests passed; its new
`--only hex` path was exercised against the real executable and its PNG inspected.
README feature claims now explicitly match the read-only viewer, including actual
entry points and unfinished #94 scope. Native Windows execution must confirm the
new platform-specific regression in CI; local macOS results do not establish it.
Issue #94 remains open until its full scope and acceptance are complete and the
implementing changes have been correctly merged; merging the viewer alone does
not meet that condition.

## Dashboard CI response-body regression

The dashboard job `124921966692` at `3fa40abf` passed 29 browser cases and failed
integration submission while evaluating an assertion's diagnostic message. The
message eagerly called `response.text()` after Chromium had evicted the body.
The assertion now uses retained HTTP status metadata, preserving the success
condition without requesting a disposable body. The integration scenario runs
both normally and with immediate navigation after submission; the latter freshly
attaches the owner, reopens the durable queue entry, and completes the original
host-verification/no-publication checks. With the original assertion, the new
navigation case reproduces the exact protocol error; with the correction, both
focused cases pass. No retries, skipped cases or production authority changes are
introduced. Dashboard E2E typechecking and static artifact currency checks pass.

The complete local browser run finished with 30 passing cases and one failure in
`transport.spec.ts`'s TLS server-reuse check: the reused CLI reported that its
server did not become healthy. The same TLS check failed in a focused diagnostic
run; temporary diagnostic instrumentation was restored. No origin is assigned to
that failure and no TLS behavior or acceptance assertion is changed by this fix.
The reported CI failure and its navigation negative control are corrected; local
full-suite results do not claim all browser acceptance passed.


## Follow-up increment: draft and native save core (2026-11-01)

PR #95 merged at `9d3e708a`. The unchanged merged-main baseline selected nine
`cyber-core binary::` tests on macOS arm64; all passed. The follow-up adds core
primitives only. Production owner routes and the CLI/TUI remain read-only until
fresh authority, private-root admission, recovery and integration are wired.

The core piece table stages expected-byte overwrite/insert/delete batches in
original coordinates, with monotonic revisions, atomic undo/redo, bounded history
and restart replay. Reviews hash both private base and proposed content, page
original-offset ranges and disclose metadata/native replacement effects. Equivalent
hex case and independent splice order share one operation payload identity.

Private storage retains pinned directory/file identities, private permissions/ACLs,
exclusive document locks, bounded records and disk budgets. Private base creation
and verification stream in bounded chunks and honor cancellation. Atomic record
publication flushes file and directory; uncertain publication fences further IO
until reopen. Existing unsafe directories are refused rather than repaired.

Native save preparation verifies live identity, metadata and full source digest,
streams a private replacement and persists an intent before one native exchange.
Commit rechecks the source content after preparation. Acknowledgement requires
installed and displaced identities, full content proofs and durable namespace
flushes. A response loss or pending intent never automatically re-executes exchange.
Unknown outcomes preserve evidence. Restore Last Save reviews a separate retained
preimage and refuses dirty drafts or externally changed source content.

Core scope does not supply application authorization: callers must retain fresh
owner/read+edit/Mode admission and lifecycle/checkout leases through settlement.
It does not activate a model tool, HTTP mutations or notifications. Private-root
protection before child launch, owner/model isolation, discard/retention, snapshot
integration and the acknowledged observation outbox are still required.

### Native storage matrix (updated after PR #96 merge)

| Platform | Core proof | Production editing admission |
| --- | --- | --- |
| macOS/APFS, same filesystem | Native exchange, metadata/xattrs/executable bits, extended ACL and process-death proof passed locally and native CI in #96 | Private owner drafts supported in the API follow-up; production save integration remains gated |
| Linux, same filesystem | Native exchange, POSIX ACL/xattr preservation and process-death proof passed native CI in #96; distinct-filesystem refusal also passed | Private owner drafts supported in the API follow-up; production save integration remains gated |
| Windows | Private DACL/frozen-base and draft proof passed native CI in #96; no Windows replacement implementation | Viewer remains available; owner editing unsupported |
| Cross-filesystem or missing exchange/private storage guarantees | Refuse preparation; no copy-and-delete fallback | Unsupported for editing |

Special files, symlink mutation aliases, multi-link targets, read-only targets,
privileged metadata that cannot be preserved, unsafe private ACLs and exceeded
budgets are refused. The native exchange protocol does not claim compare-and-swap
against arbitrary outside writers; failures after intent remain queryable unknown
outcomes with retained evidence.

Local verification at this increment: `cargo test --locked -p cyber-core --lib`
passed 363 tests with one ignored child helper. The parent crash tests explicitly
execute that helper in owned subprocesses at preparation/intent/exchange/ack cuts.
The binary selection passed 45 tests before the final owner/Location regression;
the full run includes it. Strict OpenSpec validation passed all 114 items.
Formatting, diff whitespace and core Clippy checks passed. Native Linux/Windows
workflow results remain pending until CI executes; they are not inferred from
macOS.

Code/security review used the code-review skill with the repository's existing
core/native boundary. Findings resolved during implementation: full-content
revalidation at commit after preparation, private-base integrity verification at
review, cancellation inside private verification, canonical operation payloads,
and owner/Location binding at creation/reopen. Regression tests cover these paths.
The native adapter and its owned macOS ACL allocation wrappers were inspected for
path admission, bounded allocations, identity retention and native resource disposal.
This core increment makes no production authorization or full-editor acceptance
claim; the integration gaps listed above remain explicit delivery requirements.


Bootstrap follow-up: the App now prepares and pins the binary private root before
any untrusted launch, registers its original/canonical paths with the existing
home-scoped unreadable-root protection, and configures the runtime only when the
private root is safe. Unsafe storage retains viewer availability. Project overlap
and a replaced prepared root are refused. A production App regression creates a
draft file after bootstrap, proves model `read` cannot disclose it even with broad
read grants/readable-path configuration, and includes a successful public-file
read control. The 16 existing production file-route tests and this new regression
passed locally; affected core/server/App Clippy passed. Initial Windows CI found
unused Unix-only access and needless Windows returns; those branches were corrected.


Linux native CI at the first core commit exposed `fsync` on cap-std's `O_PATH`
directory routing handle. Directory flush now obtains a readable descriptor for
`.` relative to that retained handle, including the target parent at settlement.
A Linux-only regression proves the direct descriptor fails with `EBADF`, then
flushes the pinned directory successfully after its ambient pathname is replaced.
The remote full storage/save proof must pass before Linux support is accepted.


Recovery follow-up adds an atomic bounded owner header before preparation, paged
restart discovery that works while documents are exclusively open, explicit manual
versus model-invocation namespaces, and separate exact Session Location/project-root
bindings (including nested Locations). Explicit discard checkpoints unsaved commands
without source mutation or deletion of receipts/preimages; unresolved intents refuse
discard. New regressions cover scope isolation, active-document listing, pagination,
interrupted preparation, restart discard and idempotent discard retry.

At `38214334`, dedicated Linux and macOS native CI passed. Windows native execution
exposed startup-container flushing: an inherited data namespace is not a private
payload directory. The follow-up opens that selected namespace with read/write
metadata access and uses the existing native namespace flush, while private roots
use private flushes and deny-delete directory pins. Windows results at the corrected
head remain pending. Local full core tests before this Windows-specific adjustment
passed 368 tests (one child helper); the later binary selection passed 51 tests
(one child helper). These counts include the new recovery regressions.


Shared mutation admission now uses one bounded process-local workspace lock for
Git snapshot capture/restore and in-process text write/edit/patch operations. Git
Locations below the same checkout share its canonical root. This is additional to
existing per-path and native Location ownership, and will be retained by binary
save workers through settlement. It does not claim coordination with arbitrary
external processes. The lock is released before LSP diagnostic feedback.

Local regressions passed: two retained-worker/checkout identity tests, two actual
text-write/patch serialization tests (including stale preimage refusal after a
queued external edit), and all nine real Git snapshot integration tests including
restore waiting for the shared workspace owner. Native save admission is still not
exposed by production routes.


The file-admission host now treats `edit` as a mutation rather than a synthetic
read. Current/ancestor Plan ceilings, declared edit rules, ownership mutation paths,
read-only sandbox profiles, hidden paths and protected workspace metadata remain
effective. Unknown file actions refuse. The new gate regression includes successful
read/edit controls and explicit Plan/private/read-only refusals without writing the
source. All three text/patch/admission tests passed; they are included in the native
Windows curated suite. Production binary writes are still not activated.


Dedicated native proof at `2f8ae42f` passed on Linux, macOS and Windows. This accepts
core storage/draft proof on Windows, not Windows replacement support. An additional
explicit Linux `/dev/shm` cross-filesystem refusal proof is configured for the next
CI run; its required distinct-mount negative control cannot be inferred locally.

Owner edit authorization marks its permission review as requiring confirmation,
so an owner operation cannot invoke the Auto-mode classifier. A production App
regression verifies the exact manual permission request and zero provider requests
before/after approval. Broad core task items 3.1/3.2 now reflect implemented draft
and review engines. Save admission and all delivery/integration tasks remain open.

## Owner draft API follow-up

PR #96 merged at `037fdc4b` after all 39 current-head checks passed, including
Linux/macOS/Windows native proof and the real Linux distinct-filesystem refusal.
The next increment exposes authenticated owner status, preparation, atomic patches,
undo/redo/discard, paged byte review, receipt lookup, retained-draft discovery and
explicit recovery. It does not expose source installation or imply terminal editing.

A recorded canonical resource is used for fresh permissions without resolving a
later alias into a different policy resource. Parent pins survive live-view disposal
and recovery. Private journal uncertainty refuses stale reads and receipt absence
until reopening. Draft search shares the bounded literal engine; wire receipts and
reviews have platform-independent schemas. Preparing requires enforced workspace
sandbox isolation, both read/edit admission and immutable per-handle read-only ceilings.

Binary responses bypass generic HTTP idempotency response storage/replay. The SDK
never automatically retries binary POST transport failures; explicit patch/history
requests retain their original revision and stable operation ID. Status refreshes
update handle metadata, and in-flight pages/reviews from superseded generations refuse.
Closing releases cached state while retaining private commands/receipts. Recovery
reopens the exact manual Session Location and grants no implicit edit admission.
Session deletion fences and stops native scopes first, then refuses dirty/unknown,
locked or corrupt retained journals before SessionEnd, including closed documents.

Filesystem-only workers retain lifecycle, native checkout, workspace/path and
capacity guards through actual blocking-worker completion after HTTP disposal.
Their execution settlement is distinct from a document's file-effect receipt; no
subprocess/hook callback may escape this scope. Runtime tests exercise disposal,
Stop timeout, shutdown waiting and worker unwind. Restart reconciliation of their
SQL activity records is still required before production save activation.

Scoped review uses the code-review skill against the final diff, retaining the
existing Rust core/host/runtime/HTTP layering. Findings addressed include frozen
permission resources, retained parent identities, stage-byte verification before
native intent, stale journal disclosure, lost-response retry handling and Session
orphaning. Clippy supplies the Rust cognitive-complexity gate; the skill's standalone
analyzers do not support Rust. Platform CI and final check results must be recorded
at the delivered head. No full editor, save/restore, model, shared binary snapshot,
FileChanged, terminal-editing or visual/performance acceptance is claimed here.


Local checks for the owner API increment: binary core 58 passed (one explicitly
invoked child helper ignored in the ordinary selection), production App file routes
23 passed, retained worker/deletion regressions 3 passed, actual text/patch/admission
regressions 5 passed, hex TUI selection 9 passed, SDK typecheck and 124 unit/mock tests
passed (10 external tests not run in that selection), affected five-crate library/
test Clippy passed, formatting/generated source/diff checks passed and strict
OpenSpec validated all 114 items. Later platform CI remains required. The existing
macOS native sandbox deprecation and linker messages were observed; no origin is
assigned without a corresponding main-branch comparison. The CLI read-only flag
now reaches immutable server admission. Recovery header lookup opens existing
namespaces only and does not create a document for an absent caller-provided ID.

Initial PR #97 Windows lint found unread fields in the common draft wire requests
because native edit handlers are unavailable there. Requests now support serialization
as well as deserialization, with a cross-platform round-trip regression that checks
an offset above 2^53. A Windows production-route regression explicitly checks native
edit refusal alongside successful live viewing and unchanged source bytes; that
Windows-only test remains subject to native CI. The local wire regression passed.

## Save integration follow-up work

The shrinking-restore quota regression reproduced a native replacement followed
by an unknown receipt when the displaced old source exceeded private quota.
Preflight now reserves displaced logical size and both intent/outcome publication
peaks, preserving the source/draft on refusal. The regression also reserves enough
for intent and displacement but too little for the acknowledgement, then verifies
successful restore after releasing capacity. Sparse filler exercises logical quota
without allocating 2 GiB of payload.

Explicit owner recovery now connects an actual borrowed document-lock proof to
matching tagged SQL filesystem activity. The proof must belong to the runtime's
configured private root. Active local workers, another private root and generic
native activity are not settled. This records execution completion only and never
acknowledges an uncertain file effect. Runtime proof/isolation regressions passed;
platform checks and the remaining [save activation work](owner-save-integration.md)
still apply. Source installation remains unavailable through production routes.

The follow-up's local binary suite passed 60 tests (one child helper ignored by the
ordinary selector and exercised by its parent). Four runtime ownership/proof tests,
all 23 production file-route tests, affected core/server library/test Clippy,
formatting/generated checks and all 114 strict OpenSpec items passed. Review checked
the actual acknowledgement record shape in the capacity forecast, lock-file binding
and refusal to recreate missing recovery locks. Linux/macOS/Windows CI must still
validate this follow-up. PR #97 merged at `d6a9b1ae` after all 24 current-head checks
passed, including its Windows-specific live-view/unsupported-edit regression.

## Acknowledged saved-version evidence

Native save/restore intents now retain the proposed immutable before/after base
references and original-offset range mapping. Only a durably acknowledged changed
operation can publish an immutable saved-version record. Publication is idempotent;
an existing different/corrupt record refuses replacement. The next native operation
must preserve the preceding acknowledged evidence before replacing Last Save.
Interrupted/unknown intents cannot publish successful history. Evidence contains
offsets, hashes and source-version metadata, never transient draft command buffers.

The core exposes bounded, cancellable saved-version discovery, byte pages and
original-coordinate review under a currently authorized source's pinned root/path.
These views do not acquire another owner's draft lock or read their journal. Both
immutable images are verified on open, including for non-git binary sources.
Private namespace traversal and mismatched source paths refuse access. Identical
full byte content checkpoints without native replacement or a saved-change record,
even when piece-table edits changed the internal layout.

The local binary suite passes 65 tests (one child helper invoked by parent tests
is ignored by the ordinary selector), and core library/test Clippy passes. New
regressions cover repeated saves/restart, excluded transient bytes, private dirty
draft isolation, source binding, pagination/cancellation, no-op saves and refusal
to publish unknown/corrupt evidence. This is core history infrastructure: public
save/restore/history integration, retention, durable FileChanged dispatch and the
remaining terminal/model/acceptance work are still outstanding. Platform CI remains
required for this increment.

## Final worker authority checkpoint

The retained filesystem worker now carries its captured ancestor authority,
original Turn/Location, frozen resource, granted read/edit decisions and Stop token.
Its trusted policy-only checkpoint refreshes current Turn and both decisions on
the actual blocking worker while keeping native Location, workspace/path and
lifecycle ownership. This query cannot prompt or run hooks/models/subprocesses.
Patch/history invoke it before their private journal effects. Core native commit
accepts a final checkpoint after full source/stage verification and capacity
reservation, immediately before intent publication, and rechecks cancellation.
Production save routes must supply the retained worker checkpoint there.

Regressions verify policy revocation after preparation while path ownership stays
held, refusal without source changes/pending intent/fabricated receipts, Stop at
the final check and an explicitly authorized successful follow-up. Local results:
66 binary core tests, five runtime worker/proof tests and all 23 production file
route tests passed; affected core/server library/test Clippy passed. This does not
activate public save or complete the outstanding history/notification integrations.

## Native observation handoff

Only a durable acknowledged changed source operation can mint an opaque handoff
proof. It retains native pins and the configured private-root identity, publishes
its immutable saved-version evidence first, and confirms the installed source's
metadata before the producer claims it. Draft-only/no-effect/unknown operations
cannot manufacture this proof; another configured private namespace is refused.

The SQL handoff journal records queued, claimed and terminal observed/suppressed
states. Claims bind the existing native watch generation, Location, relative path
and exact stamp. An observed disposition references a real durable matching native
delta/checkpoint; unrelated paths and fabricated references are rejected. Repeated
publication/recovery links retained evidence without dispatching another callback.
An unobserved claim whose original native generation is gone is suppressed as
interrupted, with no new UserInput or callback replay.

The BuiltinHost queues bounded metadata hints on its existing observation actor.
Its normal privacy scan, committed delta, feedback budget, membership incarnation
and hook admission checks remain the producer. No configured observer, expired
membership, a superseded source and incomplete observation have explicit pending
or suppressed dispositions. Production callers must publish after the filesystem
worker releases mutation ownership. Owner save routes and core notification receipt
settlement are not yet connected.

Actual workers durably queue known acknowledgements and retain their proof through
a post-commit handoff even if the HTTP future is dropped. They release native
Location/path mutation owners before enqueueing with the observation actor; the
lifecycle/capacity owners remain until enqueue finishes. A worker regression verifies
both HTTP disposal and that the host receives the handoff only after path release.
Explicit private staging can resume pending metadata for a previously acknowledged
source save; draft-only operations never mint a source change. Native pins remain
retained through the actor's reconciliation and durable reference settlement.

The real BuiltinHost/HTTP-hook integration regression passes: an actual native
binary replacement produces one callback, three receipt retries produce no extra
observations/callbacks, the bytes remain exact and inference is unused. Runtime
regressions cover durable reference linking, terminal-state fencing, wrong-path
references and superseded sources. Further recovery/negative cases and final scoped
checks are being completed; this does not claim full editor acceptance or retention.

Current local checks pass 68 binary core tests (one parent-invoked child helper is
ignored by the ordinary selector), 11 runtime worker/handoff tests, all 23 production
file-route tests and the real native producer/HTTP-hook integration. Affected four-
crate library/test Clippy, formatting/diff checks and all 114 strict OpenSpec items
passed after the post-commit lifetime changes. The independent namespace regression
was split from the larger source-proof test to keep the complexity gate enabled.
PR #98 merged at `69372bab` after all 22 checks passed. PR #100 carries the independent
saved-version/commit-authority predecessor; the producer follow-up is not merged.

## Retention safety work

Saved views now retain shared private history leases. Exclusive cleanup refuses
while any view is alive, while normal saves and immutable version publication
continue. Missing reader locks beside published history are not recreated. The
lease uses the existing [Rust file locking contract](https://doc.rust-lang.org/std/fs/struct.File.html#method.try_lock_shared).
Prepared native save timestamps are fixed at the commit boundary before capacity
forecasting, so holding a prepared token does not prematurely age its history.

Review also strengthened saved-view routing: every source parent is bound and
pinned, and page/review operations check that route before and after reading.
Replacing a source parent refuses both fresh and existing views; restoring the
original route permits the retained immutable view again. Regular source leaf
replacement by a later save remains compatible with reading prior versions.

The binary core suite passes 72 tests locally, with the usual parent-invoked child
helper ignored in the ordinary selector. Cleanup selection, reference preservation,
crash-safe retirement/reclamation and production integration are still in progress.

Cleanup now retires eligible acknowledged versions to compact tombstones before
reclaiming unreferenced images. Tombstones retain exact candidate references for
idempotent recovery after interrupted publication. Current/dirty bases, Last Save,
checkpoint-protected versions, pending notification evidence and live prepared save
tokens remain protected. Altered displaced files keep their complete version and
both preimages. Native intents and uncertain journals refuse cleanup.

An exclusive history lease plus complete acknowledged-version/stage-ticket coverage
can identify allocator-owned preparation artifacts that never reached native intent;
these can be reclaimed after their token closes while preserving the durable draft.
Unclassified names, incomplete evidence and older ticketless histories are retained.
Trusted authority and cancellation are rechecked before each namespace change.
The core suite passes 79 tests, including seven cleanup regressions; core library/
test Clippy passes. The configured seven-day policy, checkpoint reference adapter,
owner routes and operator UX still need integration before production activation.


### Reviewed owner save and terminal editing (work in progress)

Owner save/Restore Last Save routes bind the review fingerprint to a stable
operation ID. The actual retained worker rechecks authority immediately before
publishing native intent. The production-router regression covers save retry,
restoration and no inference; negative checks cover stale fingerprints,
cross-Session scope and revoked edit permission. TUI controls now exercise real
prepare/patch/history/review/save/restore routes with explicit second-step save,
partial-nibble cancellation, stale replies and retained-state reconciliation.
Shared snapshot/history adapters and model binary operations are still pending;
this update does not complete issue #94.

Local verification on macOS arm64: 82 binary-core tests passed (one child helper
ignored), 24 file-router tests passed, 12 binary-runtime tests passed, 15 targeted
hex/TUI tests passed, and the OpenAPI current-artifact test passed. The SDK passed
type checking and 125 unit/mock tests (ten external tests skipped in that selector);
the separate real-binary binary SDK test passed, including reviewed save, stable
retry and Restore Last Save with zero inference. Strict OpenSpec validated all
114 items. Core/server/TUI all-target Clippy passed after splitting command,
selection and response decoding helpers. Eleven screenshot-harness tests passed.
The README includes actual synthetic-fixture live-view and reviewed-edit terminal
captures. Remaining platforms rely on subsequent PR CI, not this local report.

Notification disposition lookup now uses immutable acknowledged version identity
without requiring the old source inode to still occupy the path. It can settle an
existing durable observed/suppressed result after a later save; it cannot enqueue
an observation or mint callback authority. Current native observation still needs
its retained source proof. Review pages preserve the full fingerprint while the
terminal can visit every returned range and request subsequent bounded pages.


### Shared acknowledged history adapter (in progress)

The native storage service now discovers published saved-source versions across
Session namespaces using small source headers and immutable version evidence,
without opening private command journals. Source-relative root/path/parent checks
remain mandatory. Enumeration is bounded to 64 matching namespaces and 64 returned
versions per page, including empty continuations. Owner-authenticated history and
saved-review routes reuse fresh Session/Location/read admission and cancellable
workers. A stale live viewer must reopen after source replacement. Generated
OpenAPI/SDK operations and explicit helpers expose the same metadata and byte diff.
Session snapshot/diff references, saved-view UX and the retention policy adapter
remain pending; this is not full binary editor acceptance.

Shared-history verification: the binary core selector passed 83 tests (one helper
ignored); the focused cross-Session/paging regression also passed after making
continuation anchors omit unsaved private document IDs. The production-router
regression proves unsaved history exclusion, reopened-reader access to saved byte
diffs and fresh read-denial refusal. Core/server all-target Clippy, SDK typecheck,
125 unit/mock SDK tests (ten external tests skipped), a separate real-executable
SDK history/save/restore test and strict OpenSpec validation passed. The shared
snapshot/rewind and retention adapters remain pending.


### Shared Session diff references (in progress)

The shared Session diff now reads published acknowledged source references from the
immutable catalog independently of Git and notification delivery. It checks current
read decisions before returning metadata, verifies the original root/path/parent
binding, and rechecks Session/Location and permission state. No command journal or
byte image is opened for summaries. Existing text patches remain compatible; the
optional `FileDiff.binary` array carries per-save lengths, fingerprints and durable
version identities rather than fabricating a combined text/byte patch. The TUI
labels these rows and renders their saved metadata. Enumeration is bounded to 512
references, with paged history available separately. Checkpoint/rewind and the
configured retention adapter still remain pending.

The production-router regression also injects failed notification-handoff journal
appends after native acknowledgement: the shared Session diff still exposes the
immutable saved reference, while no successful notification handoff is fabricated.
Fresh read denial hides the reference. The focused catalog regression, 15 targeted
TUI tests (including the shared diff overlay), OpenAPI artifact test, SDK typecheck
and 125 unit/mock tests, core/server/TUI all-target Clippy and strict OpenSpec
validation passed. No new database migration is required for this adapter.


### Model binary tool (follow-up to merged PR #103)

The built-in `binary` tool now offers inspect/read/search and exact expected-digest
patches. Read pages default to 256 bytes and cap at 4096; inspect streams live
SHA-256 evidence through 512 MiB without preparing a private draft. Plan mode can
read/search, while patches additionally require edit authority and a native writable
workspace sandbox. Fully denied reads hide the tool. Text tools retain their
existing semantics.

Each patch has a separate invocation/payload-bound private namespace. Its small
owner header is authenticated before a prior journal/base is loaded. Expected
whole-base and per-splice bytes, 128-splice/64-KiB limits and canonical payload
identity are enforced by the shared core. Permission approval receives the exact
byte review before mutation ownership. The retained filesystem worker validates
its dispatched built-in call, current Session/Location/ancestors, current tool and
read/edit policy and sandbox boundary immediately before intent. A dedicated
`bin_` checkout lease avoids reacquiring the active actor's Session lease and
cannot acknowledge generic subprocess activity. Native receipts and post-commit
notifications use the same save engine as the owner editor. The tool is serialized
and never automatically retries an uncertain replacement.

Verification so far: 83 binary-core tests passed (one process-death helper ignored
in this selector); the independent checkout-owner regression, five real runtime
model-tool regressions, the Plan read/search regression and all 45 general tool tests passed. Coverage includes
exact growth bytes and saved history, digest/expected-byte refusal, read/edit denial,
Plan refusal, approval byte metadata, fresh policy revocation during approval, Stop
during approval and separation from the manual draft namespace. Strict OpenSpec
validation passed all 114 items. All six existing server-worker regressions and
core/server/tools all-target Clippy passed. Review identified and corrected the
production AppHost forwarding of the independent worker lease; its dedicated
regression and app all-target Clippy passed. Formatting and diff checks passed.

This is not full #94 acceptance. Dedicated model receipt recovery, configured
retention/checkpoint/rewind adapters, remaining UI recovery/accessibility and named
performance/platform acceptance remain open. No external provider was called by
these model-tool tests. Linux and Windows verification for this follow-up must come
from CI; local verification is macOS only.


CI follow-up, 2026-11-03: Linux/macOS Rust and Windows build failed the
canonical tool-registry/golden coverage checks because the new tool was listed only
in the active proposal catalog and lacked a golden fixture. The canonical catalog
now lists the implemented tool. A real Plan-mode binary read golden retains bytes,
bounds, fingerprint and source metadata, normalizing only the live revision and
JSON key order. All 14 golden tests and strict OpenSpec validation (114 items)
passed locally. Cross-platform CI must verify the corrected head before merge.


### Model operation receipt recovery (local follow-up)

Separate `/files/binary/model` owner routes discover only the current Session and
canonical Location/root's model namespaces, query individual operation receipts and
explicitly reconcile retained native proof. Model headers do not enter manual
recovery or produce editable document handles. Discovery includes only explicitly
read-allowed names; known-ID lookup uses the ordinary read approval flow. Auth,
Origin, current read/ancestor scope, exclusive journal ownership, lifecycle/worker
capacity and response `no-store` remain effective. Absence is returned only from a
validated journal; locked/missing/fenced state is refused.

Reconciliation never exchanges the source again or dispatches callbacks. Unknown
intent and notification-pending state remain explicit. Resume/reconciliation can
cancel bounded image/hash work while retaining proof. A shared resource-bound
resume checks the journal target against the already authorized header resource
before opening a base, for both manual recovery and model calls/recovery.

The SDK adds `client.binary.model.list/operation/reconcile`, preserving identities
and the existing no-automatic-retry rule. Generated OpenAPI and SDK operations are
updated. Tests cover own/manual/other-Session/Location namespace isolation, paging,
missing operations, read denial and revocation after approval, auth/Origin/no-store,
repeated reconciliation with unchanged file identity/bytes, cancellation preserving
an installed unknown intent and explicit refusal of a different authorized resource.
The broader retention/checkpoint/rewind and remaining UI/platform/performance
acceptance work is still open; this increment does not close #94.


Local verification for model recovery: 20 production-router binary tests and 84
binary-core tests passed (one process-death helper ignored in that selector).
Core/server/app all-target Clippy, formatting/diff checks and the current OpenAPI
artifact test passed. SDK typecheck and 127 unit/mock tests passed; ten configured
external SDK tests were skipped, and no new real-executable SDK run is claimed for
these recovery endpoints. Strict OpenSpec validation passed all 114 items. Source
review checked namespace/resource binding, permission waits, private-buffer
separation, cancellation and the existing filesystem-only recovery proof. Linux
and Windows recovery acceptance remain for CI after the preceding PR merges.


### Named 16 MiB core performance evidence

The reproducible `binary_editor` harness ran five serial 16 MiB trials on the
Apple M2/24 GiB reference, with full hash verification after native save and
restore. Both unoptimized and optimized results and process peak-RSS evidence are
in [the measurement record](../../../docs/evidence/binary-editor-16mib/README.md).
Optimized medians were 0.54 ms for open/first 64 KiB page, 141 ms for immutable
preparation, 101 ms for byte review, 553 ms for native save, 545 ms for native
restore and 0.064 ms for cooperative search cancellation. Optimized process peak
RSS was 7.45 MiB, excluding filesystem cache. These core-only observations exclude
server, permission, terminal rendering/input and provider work; they do not close
the broader UI/platform/performance acceptance task. The benchmark target passed
core all-target Clippy.
