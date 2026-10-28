# Hook producer integration

This draft integration combines PR #56 (`27f561ba`), PR #57 (`84c964bd`) and
PR #59 (`5aa08018`) without merging any PR into main. Individual acceptance
does not prove the composed runtime. DirectoryAdded, Setup completion, plugin
enforcement/protocol work and later workstreams remain open.

## Shared transition

`exit_idle_worktree` captures the committed CwdChanged observation and resulting
SessionInfo from the same acknowledged exit. After releasing the Session lock
and deletion fence, it refreshes host-owned file-observer membership, releases
its shutdown admission, then dispatches or defers CwdChanged through the existing
worktree owner mechanism. File callbacks follow the returned Location without
a new Turn; the committed CwdChanged producer retains stable transition identity
and cannot veto the move. Both event names remain in runtime/unavailable lists.

The real HTTP-hook idle-return regression now enables both event types together.
It checks exactly one enter and one exit observation with the moved Session and
resulting envelope, future delivery at the returned Location, exclusion at the
old worktree and no manufactured model Turn. Existing worktree and isolated-child
suites cover their own failed/no-op/replay/permission and pre-Git veto paths.

## Regression controls

Whole-file transplants from main now erase APIs required by another increment.
The dedicated workflows therefore retain independent implementations while
removing the specific production fix under test:

- Child admission control removes only WorktreeCreate dispatch from the current
  implementation, retaining permission checks and the current owner/observer APIs.
- Cwd control suppresses the committed observation constructor while retaining
  worktree ownership helpers and file membership.
- Idle membership control removes only its refresh call, preserving CwdChanged
  production references and callback behavior.

These are intentional missing-fix controls, not unchanged-main checks. Each
requires the expected test to execute and fail at its designated behavioral
assertion; compilation failure cannot satisfy the control. The earlier standalone
PR workflows retain historical unchanged-base evidence.

## Verification

Local Rust formatting and workflow YAML parsing are checked before publication.
Strict OpenSpec and spec lint are also run. Full local Rust builds are not run
because available disk cannot accommodate the workspace build. Focused native
Linux/macOS/Windows and broad CI on this exact integration revision are pending;
no composed runtime acceptance or final security approval is claimed yet.


## Private-root rename correction

Review reproduced a metadata leak in path-only exclusions: after a private root
was renamed under the Location, its new subtree entered the metadata inventory.
The source-level before-fix test observed
`relocated-owner-data/credential-metadata`. It did not read private file contents.

The host now primes retained private identities at construction and trusted root
attachment, and refreshes them before scan admission. At most 128 private
identities are held globally per host, independently of watcher count. Snapshots
retain their own references. Directory handles (and Windows file handles) can
release registry slots when their metadata proves zero remaining links. Unix
regular-file identities are recorded using stat without opening those files;
these records remain until host shutdown, including after unlink, because a
missing pathname cannot prove retirement. Inode reuse can conservatively exclude
an ordinary replacement. Heavy file-identity turnover can fill this budget even
when earlier files were removed. An identity error or full budget latches
observation unavailable for that host, requiring restart instead of silently
losing unknown ownership. No durable schema or public API changes are introduced.

Path exclusions still reject configured trees/files and canonical aliases. The
scanner additionally rejects pinned private metadata before traversal and checks
an opened directory again before creating its iterator. A path/handle identity
mismatch cannot supply a new subtree. Pinned objects are retained for the live
host; this is not a durable provenance catalog for arbitrarily relocated historical
storage from before host initialization.

Tests cover renamed private trees with ordinary files still visible, private-file
aliases, a private directory replacing public pathname metadata before open,
proven directory retirement, retained Unix file history, budget exhaustion and
latched uncertain capture.
The existing application-host test now renames the actual private cache inside
the Location and requires zero private metadata in its real journal. Linux CI
removes the identity filter as a missing-fix control and must execute/fail that
application assertion before restoring the fix. Full application/platform results
for this correction remain pending.

At the earlier integration revision `1b2ebe71`, all seven focused jobs passed:
FileChanged on Linux/macOS/Windows, and child admission plus CwdChanged on
Linux/macOS. The new private-identity correction passes 69 source-library tests
and scoped Clippy (`-D warnings -A dead_code`) in the small harness. That harness
compiles actual project scanner/privacy/root/native/core record sources but does
not compile the production host/owner or AppHost bridge. Those paths require
the new focused/broad CI; no full local Rust workspace build was run due to disk.
Formatting, workflow YAML/bash/control checks, strict OpenSpec (106 passed) and
spec lint (zero errors, 25 warnings) also passed before publication.

Scoped code/security review followed the privacy inputs through host priming,
scan admission, metadata exclusion, post-open directory checking and bounded
retention. It identified and corrected two risks in the first candidate: unknown
capture must stay unavailable instead of resuming without provenance, and
pathname metadata alone cannot authorize opening a replacement private directory.
Database-path lookup errors now withhold observation rather than omit protection.
A private root itself cannot become an observation Location. The regression tests
above cover the scanner/retention changes; real owner/platform verification and
final combined-feature review remain pending.


## Broad acceptance: MCP no-wait admission and browser diagnostics

All seven focused jobs passed at `22365228`, including 19 real HTTP dispatch
tests on Linux/macOS, 18 on Windows, two AppHost tests per platform, and the
Linux path-only missing-fix control. Builds, SDK and lint checks also passed.
Broad Rust acceptance remained incomplete: Linux passed 5,107 of 5,108 tests and
macOS passed 5,104 of 5,105, both failing the same MCP no-wait receipt case twice.
The test received a completed `second answer` where the existing contract requires
the fresh admission acknowledgement. These are observed results, not an
assignment of failure origin.

A fresh MCP admission now carries its queued acknowledgement with the original
MCP admission timestamp and zero turns. `wait: false` returns that snapshot instead
of querying a model run that may already have finished. Waiting and replayed
calls retain their ordinary durable status lookup. The regression checks no
terminal fields in the initial receipt, stable admission time on the later wait,
and completed status on an exact replay. The focused workflow now runs the actual
MCP protocol suite on all three platforms. No transport or output schema changes
are introduced. Full Rust/MCP validation of this correction awaits CI.

The browser suite passed 29 of 30 tests at `22365228`: the native integration
crash-recovery test timed out while waiting for exactly one durable check intent.
The same suite subsequently passed on main at `8093aaf6`; this does not establish
the candidate timeout's cause. The candidate's macOS release artifact from that
CI run (embedded build stamp `6b09e23`) also passed the same API-only crash/restart
sequence locally with one check intent and no model calls. The browser assertion
remains unchanged. On a recurrence, diagnostics now record the evidence count,
batch/report status and bounded check outcome/limitation instead of only a generic
timeout. It is not counted as fixed solely because another host passed.

The branch is reconciled with merged PRs #56, #58 and #60, preserving both Setup
and observation scenarios under their proper requirements. Local E2E typecheck,
syntax, workflow YAML, Rust formatting, PR diff checks and strict OpenSpec
validation passed for the new increment. Full local Rust compilation is still
not performed due to available disk; production protocol and broader acceptance
remain required before this draft is ready.


After PR #56 merged, the old child control tried to pair main's new Option-based
child API with the pre-fix boolean resume call. CI correctly rejected that as a
compilation failure, not regression evidence. The control now removes only the
WorktreeCreate dispatch call from the current candidate and still requires the
real veto test to execute and fail at its behavioral assertion. It no longer
assumes that the PR base lacks an already-merged fix.


## SQLite lock preservation correction

A native macOS record-lock probe and then the actual source-library regression
reproduced a correctness defect in descriptor-based file pinning. Opening and
closing a duplicate database descriptor released an existing process POSIX record
lock. `O_EVTONLY` did not preserve that lock either. This matches SQLite's warning
about [separate descriptor closes](https://www.sqlite.org/howtocorrupt.html#posix_advisory_locks_canceled_by_a_separate_thread_doing_close_).
This evidence proves a defect in the candidate; it does not establish the cause
of either outstanding CI timeout.

Unix regular-file capture now uses metadata only, with the conservative bounded
history described above. Directory capture uses `O_DIRECTORY` so replacing a
known directory with a file cannot trigger a regular-file open/close. The owner
root's current-incarnation check also opens only directories: review found that
`same_file::Handle::from_path` otherwise opened a database replacement as a file.
The lock regression independently failed for both the original pin capture and
that root check. It uses a separate test process to attempt the native record
lock before capture, after repeated capture, after registry drop and after a
Location is replaced by a hard link to the locked file. The subprocess must run
exactly one test; an unmatched filter cannot count as evidence.

The sidecar turnover regression permits only explicit identity-budget exhaustion,
never a stat/open race reported as unknown capture or silent ownership eviction.
This is a deliberate bounded-history limitation, not a claim of unlimited live
observation through arbitrary private-file generations. Production owner/AppHost,
full protocol, platform and browser validation of this revision remain required.

At `3685cd2b`, the full Linux and macOS Rust jobs passed, as did both CwdChanged
jobs and the macOS/Windows FileChanged jobs. Linux FileChanged failed its real
HTTP observer-restart liveness assertion; the browser crash-recovery test again
remained in `checking` with zero durable check intents. Those failures remain
unresolved pending investigation and checks of the locking correction.

Local verification of the correction: 72 actual-source harness tests passed,
with one subprocess helper ignored in the parent suite and explicitly exercised
by the lock regression. Scoped Clippy (`--lib --tests -- -D warnings -A dead_code`),
Rust formatting, workflow YAML/bash/exact-source control checks and PR diff checks
passed. Strict OpenSpec passed all 107 items; spec lint reported zero errors and
25 warnings. This harness excludes production owner/AppHost/MCP integration; no
full local Rust workspace build was run because available disk is insufficient.
Scoped review checked file/directory classification, directory-only replacement
handling, descriptor lifetime and subprocess execution. Remaining merge gates are
full production/platform CI, the unresolved restart/browser failures, and final
review of the composed feature. No overall security approval is claimed.


## Current CI failure correction after 0b7c4a3a

The full browser suite and both child-hook jobs passed at `0b7c4a3a`. The Linux
and macOS focused suites could not compile the new raw-fcntl test because the
workspace denies unsafe code; both broad Rust jobs failed at Clippy for the
same reason. The standalone harness had omitted that workspace lint. Linux
artifact-upload errors followed the earlier compile failure and do not establish
additional runtime defects. Windows reached the native component tests: 45
passed and sidecar turnover failed with unavailable identity capture.

The lock regression now holds a real SQLite exclusive transaction and uses a
separate SQLite connection in its subprocess to require `DatabaseBusy`; it has
no unsafe blocks or new project dependencies. The harness also enforces
`unsafe_code = deny`. Restoring an independent file open/close and then restoring
the original Location check each made this new SQLite regression execute and
fail at the intended lock assertion, independently; both controls were restored.
The fixed actual-source harness passed 72 tests with its one subprocess helper
ignored in the parent suite and invoked explicitly by the regression; scoped
Clippy passed.

Windows capture now requests only `FILE_READ_ATTRIBUTES`, not file-data read
access. A new Windows-native regression captures metadata while an existing
writer denies data sharing, and verifies the writer can still write. Capture
retries access/sharing errors up to eight times with two-millisecond intervals.
A pending delete can map to access denied through [CreateFile](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew),
but that does not prove the observed error's exact cause. The candidate never
treats an ambiguous denial as an absent file: a persistent error still latches
unavailable and test-only diagnostics now record its raw OS error. The sidecar
turnover assertion remains in place. Native Windows and the full production
checks for this correction remain pending; the earlier Linux restart assertion
was not reached at `0b7c4a3a` and is not counted as resolved.
