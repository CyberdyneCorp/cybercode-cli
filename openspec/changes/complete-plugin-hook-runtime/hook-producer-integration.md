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

- Child admission control restores the base child implementation and only the
  matching resume call signature, retaining observer additions to BuiltinHost.
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
attachment, and refreshes them before scan admission. At most 128 live native
pins are held globally per host, independently of watcher count. Snapshots retain
their own references. Fully unlinked objects can release registry slots; moved or
linked objects cannot. An identity error or full live budget latches observation
unavailable for that host, requiring restart instead of silently losing unknown
ownership. No durable schema or public API changes are introduced.

Path exclusions still reject configured trees/files and canonical aliases. The
scanner additionally rejects pinned private metadata before traversal and checks
an opened directory again before creating its iterator. A path/handle identity
mismatch cannot supply a new subtree. Pinned objects are retained for the live
host; this is not a durable provenance catalog for arbitrarily relocated historical
storage from before host initialization.

Tests cover renamed private trees with ordinary files still visible, private-file
aliases, a private directory replacing public pathname metadata before open,
unlinked-slot reclamation, live-budget exhaustion and latched uncertain capture.
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
