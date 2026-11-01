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

### Candidate native storage matrix

| Platform | Core proof | Production editing admission |
| --- | --- | --- |
| macOS/APFS, same filesystem | Native exchange, metadata/xattrs/executable bits and extended ACL, process-death recovery exercised locally | Not activated; integration gates remain |
| Linux, same filesystem | Native exchange and POSIX ACL/xattr preservation implemented; native CI pending | Not activated; filesystem/platform evidence required |
| Windows | Private DACL/frozen-base and draft support implemented; native CI pending; no Windows replacement implementation | Viewer remains available; editing unsupported |
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
