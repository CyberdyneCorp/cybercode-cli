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


## Server Location identity checks preserve the same lock boundary

Follow-up review traced user-input scope capture and handler membership through
`runtime/file_observation_roots.rs`. Both still used `Handle::from_path` on Unix,
which could open and close a regular file after a Location or alias was replaced.
Three new real SQLite regressions independently failed before correction: direct
input capture from a database path, a Session alias naming a database while the
canonical root remains intact, and the canonical root replaced by a database hard
link. Each subprocess ran exactly one lock probe and reported that the owner lock
had been released.

These production Unix probes now use the server's existing safe rustix dependency
to open with `O_DIRECTORY | O_CLOEXEC`; a regular file cannot be opened and later
closed as part of directory identity validation. Existing alias/identity matching
still applies. Other platforms retain their native handles and explicitly reject
non-directory metadata. No new project dependency, durable record or API is added.
The actual-source harness now passes 75 tests with two subprocess helpers ignored
in the parent suite and invoked by the lock regressions. Production server and
platform CI remain required, and Windows behavior cannot be verified on this local
macOS host.


## Shared-membership delivery lifetime correction

At `e15197e1`, the complete Linux FileChanged job passed: native components (56
passed and one parent-suite helper ignored), four runtime-root tests, 17 durable
server tests, 19 HTTP dispatch tests, both AppHost tests, the missing-fix controls
and all 30 MCP tests. Windows passed its native component and durable-runtime
stages, including the unchanged sidecar-turnover assertion, but its HTTP suite
passed 17 of 18 tests and timed out preparing the readmitted Session's durable
scope after capacity was released. This is a distinct observed failure; its cause
is not established solely by the earlier sidecar correction.

Review found that an observation's captured Session list could outlive membership:
removing a Session did not cancel its already-captured delivery or skip it after
another callback awaited. That could admit effects for an ended membership and
hold later membership preparation behind obsolete callbacks. Membership now owns
a child cancellation token per Session. Removal cancels that token and clears
pending scope/renewal work. Every batch captures its recipients' tokens before
commit, rather than resolving a fresh token after earlier callbacks settle;
readmission cannot revive the old batch. Parent cancellation still reaches all
members. Pending scope/renewal processing also skips removed memberships.

Three real HTTP regressions cover removing a queued member while another callback
is held, cancelling the gated member while another remains covered, and readmitting
a member without delivering its old held batch. An owner unit test checks that
removal cancels held tokens without stopping other members and that a fresh token
does not revive old delivery. Linux CI removes only membership cancellation and
must execute/fail the named queued-member assertion, then restore the fix before
running the complete positive suite. The existing capacity test keeps its 45-second
deadline and assertion, adding only eight bounded recent FileChanged notices and
journal counts when it fails.

These owner/HTTP regressions have not run locally: the small harness excludes the
production owner and integration bridge, and available disk prevents a full local
workspace build. The 75-test harness result applies to the scanner, records,
private identity and server-root sources tested earlier. Formatting, PR diff and
workflow YAML/bash/exact-source mutation checks passed for this increment. Full
production/native CI and final composed review remain required; this correction
is not acceptance evidence for the Windows timeout until those paths execute.


## Membership regression complexity correction

The complete Linux focused job passed at `b5dd82f0`, including the three new HTTP
regressions and the missing-cancellation control. Broad Linux and Windows Clippy
rejected the shared test helper with cognitive complexity 22 against the configured
limit of 15. The helper is now split into event lookup, membership removal and
post-removal verification, preserving all scenarios, deadlines, assertions and
the negative control's expected assertion text.

With additional local disk available, scoped Clippy was run against the actual
production test target using the existing shared build cache:
`cargo clippy --locked -p cyber-tools --test file_observation_dispatch --target-dir /Users/edvane/work/cybercode-cli/target -- -D warnings`.
It passed in 54 seconds with the repository's unsafe-code and complexity lints
unchanged. This check compiled production owner/host dependencies and type-checked
the real integration test target; it did not execute HTTP tests or the full
workspace suite. Native acceptance for the refactored revision still requires CI.
The cognitive-complexity skill does not provide a Rust analyzer; the project's
Clippy result is the authoritative check for this refactor.


## Observation identity in readmission acceptance

At `70224077`, native focused CI passed on macOS and Windows. Linux passed 21 of
22 HTTP tests and failed the readmission test's filename assertion. That log
reported the path, not observation IDs, so it could not establish which batch
was delivered. A local control deliberately changed the same file after readmission
while the first callback was held. The filename assertion failed even though the
retained member received observations A/B and the readmitted member received only
new observation B. This reproduced an invalid assertion without old-batch revival;
it does not establish the original Linux notification sequence's cause.

The regression now captures the first held `observation_id` before membership
ends and requires its absence from the ended/readmitted member's delivery. The
readmission case also deliberately changes that same pathname and requires an
observation with a different ID, proving that valid future delivery continues.
The missing-cancellation controls for both ended and readmitted cases still
execute and fail at the identity assertion, showing the held ID among delivered
IDs. Source is restored before the positive suite. All 22 actual HTTP tests then
passed locally; scoped actual-target Clippy passed with the configured complexity
and unsafe-code lints.

The full focused local set at `70224077` also passed 148 tests: core records 13,
envelope 1, native components 57, runtime roots 6, durable server 17, HTTP 22,
AppHost 2 and MCP 30. Two subprocess helpers are ignored in parent suites and
explicitly exercised by their SQLite regressions. The existing App plugin suite
passed 14 cases on that integration revision and on unchanged main `2ae7957d`;
see [the baseline evidence](baseline.md#passing-unchanged-main-native-baseline-2026-10-28).
Those local native runs are on Darwin 25.6.0 arm64, with CI/platform results tracked
separately. Full CI for the identity-precise regression remains required before
this draft is ready.


## Known-private replacement retirement from composed review

At `6385cabe`, all 16 broad CI jobs and all seven focused producer jobs passed.
Final composed review then reproduced a distinct observation error: replacing a
previously public baseline file with an alias of the live private database caused
a public FileChanged removed callback. A known private directory replacing a
public subtree similarly manufactured descendant deletion callbacks. The scanner
filtered private identities before alias accounting, so those omissions were
mistaken for absence rather than private scope.

The native scan now records known-private entry/subtree paths separately from
ambiguous aliases and byte-budget omissions. After verifying its pinned root,
the owner retires previous public preimages under those private prefixes without
dispatch or feedback charge, before folding the remaining scan. This also applies
when unrelated traversal is incomplete. Directory-scope revalidation removes
private exclusions whose containing scope lost proof. Ambiguous aliases keep
their existing unknown-state behavior; they are not silently retired by this fix.
No additional durable format or schema change is introduced by this follow-up.

Two actual HTTP regressions cover a known database inode replacing a public file
and a retained private directory replacing a public subtree. They require silent
retirement, continued public observation, and no private-leaf callback. The file
case also requires an unrelated real public deletion, preventing an implementation
that simply labels every filtered scan incomplete. Removing only early retirement
in a local source control makes both named tests execute and fail their expected
behavioral assertion; restored source passes both. Linux CI repeats this control
and preserves its logs before the full positive suite.

The native component suite passes 57 tests, with one ignored subprocess helper
explicitly invoked by its SQLite-lock regression. The 24-case HTTP suite and scoped
actual-target Clippy pass locally on Darwin 25.6.0 arm64. Full CI for this new
review correction remains required; the green `6385cabe` result is historical
acceptance of the preceding revision, not evidence for this source.
