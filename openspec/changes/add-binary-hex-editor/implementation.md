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
