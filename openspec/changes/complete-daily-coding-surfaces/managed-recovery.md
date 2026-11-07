# Managed snapshot recovery: ownership and resolution design

Status: approved for implementation on 2026-11-06. The user reviewed the concrete
protocol below and replied, "Approve this protocol and implement." This includes
fingerprint-confirmed keep/rollback, exact worker-bound acknowledgement, retry
lookup without replay, legacy unknown fences and cleanup after durable
acknowledgement/reference checks. The earlier 2026-11-05 feature request alone
did not select this protocol. Approval does not establish implementation or
acceptance; the native/platform and complete recovery tests remain required.

Implementation checkpoint: the non-Git review/confirm/lookup API and SDK now use
durable request reservation and paired native settlement. Local real-service
partial-restore kills exercise keep/rollback and lookup without replay. Explicit
journal acknowledgement reconciliation is implemented for new file-copy receipts;
pre-origin resource cuts, Git journaling,
reclamation, complete clients and the platform matrix remain unfinished. Parent
tasks are open; the complete recovery delivery is not merge-ready.

## Owner-visible contract

The owner selects a retained restore by Session/Location and journal ID, requests a
read-only `keep` or `rollback` review, and confirms its fingerprint. Review includes
original operation identity, current source identities, intended and displaced
images, exclusions, conflicts, proposed byte/mode changes and acknowledgement limits.
CLI: `cyber sessions recovery <session> <journal> --action keep|rollback`; applying
requires `--confirm <fingerprint>` and a stable request ID. API/SDK expose the same
review/confirm/lookup flow. No inference starts. The TUI links recovery from rewind
unavailability and uses the same confirmation, never a second restore engine.

`keep` preserves the reviewed live files; it does not finish the old rewind or move
the conversation. `rollback` reverses only effects proven to belong to this restore,
preserving unrelated owner files. If a changed path does not match its reviewed
before/after/native evidence, refuse the whole plan before mutation. Neither action
replays the original restore or treats a missing `applied` entry as no effect.
Unsupported metadata, aliases, cross-filesystem staging, ambiguous/malformed records
or changed Location identity refuse rather than weakening the native contract.

Keep may review a later ordinary owner replacement as the current image; preserving
it does not assign that inode to the failed restore. Rollback still requires the
original source/private ownership pairing. Both actions bind exact current images
and parent observations into the review and refuse stale confirmation. A resolution
uses a distinct native intent, retaining its displaced live inodes and a reference
to the original immutable intent. Retry of an existing request is lookup-only even
when its outcome is partial. Original and resolution evidence remain fenced until
the consuming runtime transaction acknowledges the exact workers. Empty created
source directories remain protected until acknowledged, ownership-checked cleanup;
file rollback does not recursively remove owner directories.

## Bind evidence to a worker, not merely a Session

New managed restore intents record an immutable origin: Session creation identity,
canonical/native Location identity, native activity ID, checkout binding, journal
namespace identity and version. The runtime marks the activity as a snapshot
filesystem-only worker before it can launch. Snapshot callbacks cannot label a
generic shell/process activity as filesystem-only.
New origins also bind the exact private lock inode, including its creation
identity. Acquiring a replacement lock file cannot prove that an old worker has
released its original descriptor. Older origins without this binding stay
inspectable and fenced; no retrospective lock proof is invented.

The snapshot-worker compatibility migration records the new event semantics even
without new tables. Older migration catalogs must refuse mutation rather than
ignore typed worker fences. Existing native evidence remains unchanged.

A trusted synchronous worker owns the actual mutation lock, private cross-process
store lock and checkout/lifecycle resource through its last filesystem call,
including when its async requester disappears. Reuse the binary worker's retained
checkout-acknowledgement pattern: the retained native checkout lock stays owned by
the actual worker; disposal cannot release it while a detached worker runs. A
process death releases the lock, but leaves the separate intent and pending native
activity. Acquiring the exact bound store/checkout locks plus validating that origin
is proof about this worker only. It cannot acknowledge a shell, child process,
other document, another runtime's live worker or an unrelated unknown activity.

Runtime Location uncertainty distinguishes exact snapshot activity IDs from opaque
process/checkout outcomes. Reload preserves both kinds independently. Observing an
exact durable snapshot acknowledgement can remove only that snapshot reason; it
cannot clear the opaque reason or another snapshot owner. A committed runtime
acknowledgement followed by failed journal acknowledgement requires reconciliation,
without reclassifying the already-committed native effect as unknown.

Resolution is available through a dedicated owner recovery path while ordinary
admission is fenced. It does not use a blanket admission bypass. It requires idle
Session/Location scope, no active origin worker, exact captured authority and
independently owned lifecycle/checkout/store resources. Revocation and source/route
identities are rechecked before every filesystem effect and the final commit.

## Rollback and commit ordering

New file-copy records retain a prepared native ownership section before source
effects: each original/staged file's device, inode and creation identity, every
parent route's native identity, and private staged identities for missing parents.
Missing parents are prepared privately and published with no-replace only after
this section is durable. Inspection compares both source and private slots to the
prepared identities and expected bytes/metadata; progress alone never decides
whether a rename occurred. Same-byte foreign replacements, aliases and changed
parents refuse. Older records without this evidence remain readable but do not
gain rollback authority. A crash during private preparation retains an unprepared
intent; it does not authorize inferring any source replacement.
Original inodes, prepared stages and new parents are flushed before prepared
ownership is published. New-parent ownership includes its ordinary metadata;
changed mode/ownership, ACLs, attributes or flags refuse rather than licensing
directory removal. Existing owner-directory modes are preserved. Authority is
rechecked before each parent publication and file namespace operation, including
when cancellation occurs between parent creation and file publication.

1. Validate original intent/stage objects and complete all conflict checks under the
   owned locks. Freeze a bounded reviewed plan and digest with current file and
   parent identities. A changed fingerprint requires a fresh owner review.
2. Publish a distinct durable resolution intent with its request ID, action, parent
   journal and current/preimage evidence before any rollback mutation. Prepare all
   stages privately. Rollback uses native exchange/no-replace and retains the actual
   displaced live inodes. A failed resolution is itself retained recovery evidence.
3. Flush source parents, private stages and a filesystem outcome. A crash or lost
   reply never becomes an inferred success. Repeated request IDs look up their exact
   outcome; they never redispatch source changes.
4. While the exact native proof remains held, compare-and-append the bound native
   activity acknowledgement and resolution receipt at the single database writer.
   Do not manufacture the old rewind's transcript events. Other native/tool unknown
   outcomes and subtree stop boundaries remain unchanged.
5. Only after an observed durable writer acknowledgement mark the resolution's
   acknowledgement in the journal. If the writer committed but its reply was lost,
   retained request/origin lookup reconciles that original receipt under the same
   proof. If journal acknowledgement fails, preserve payloads and permit only
   acknowledgement reconciliation, not another rollback.

## Store transaction prerequisite

The native activity and owner resolution receipt are two separate durable
aggregates. The Store's paired checked append accepts exactly two distinct owner
IDs (1–256 bytes), each with an exact expected sequence (`-1` for creation),
and at most 64 events/16 MiB of encoded events across the pair. The writer runs
the decision, both event batches and all projectors in one IMMEDIATE transaction.
Any decision, validation, CAS, projection or rejected-commit error rolls back
both owners and decision writes. Each changed owner is notified only after
observed COMMIT success. Encoded-size validation counts into a bounded sink rather
than allocating a serialized copy. The callback cannot perform filesystem or
other external effects.

An error after SQLite committed must fence new writes and withhold notifications;
the owner reconciles the original durable request/receipt after recovery instead
of repeating filesystem changes. This Store primitive alone does not establish
native proof, admit a recovery operation or implement a resolution receipt.

## Safe reclamation

Only records with both the durable owner resolution receipt and matching journal
acknowledgement are eligible. Reference/reader accounting protects staged baselines,
inactive branches, leases, binary saved versions and any unresolved intent. A missing
or incomplete reference view refuses cleanup. Neither quota nor age overrides this.

Cleanup uses a durable reclaim phase/tombstone and descriptor-relative no-follow
removal of known immutable payloads. Record and sync eligibility before deleting;
retain a bounded receipt sufficient to answer repeated request IDs. Restart may
finish acknowledged payload reclamation but must not delete unacknowledged evidence
or replay filesystem resolution. Unknown files, aliases and identity changes refuse
cleanup visibly. No direct user removal of private journal directories is required.

## Compatibility and migration

Historical bare Git trees remain readable with explicit unknown exclusion history.
New Git snapshots retain immutable exclusion evidence separately from current config;
excluded paths are unknown rather than deletion authority.

Existing #110 journals lack an activity-origin binding. Keep them readable and retain
their actual preimages. Never upgrade them into authority to acknowledge a generic
native activity by matching only Session, path, PID or timestamps. A legacy filesystem
review may be offered without clearing such native fences; any migration that clears
one requires a separately evidenced exact-origin proof. No force-clear switch is part
of this design. Git-native journal support must meet these same ownership guarantees;
legacy backup directories alone are not crash-recovery proof.

Linux/macOS writes remain conditional on admitted native storage and metadata.
Windows unsupported file-copy cases stay explicit until their native implementation
and positive/negative proofs meet the same contract. Platform refusal tests do not
count as positive native implementation acceptance.

## Required acceptance

- Positive keep and rollback after real partial replacement; preserve unrelated edits.
- Reject stale review, foreign Session/Location/namespace, live origin worker,
  symlink/hardlink/parent replacement, unsupported metadata and missing preimages.
- Prove a snapshot receipt cannot settle a generic/native-child activity or another
  worker's checkout receipt; verify durable origin and writer CAS after restart.
- Process-kill cuts before/after intent, stage preparation, each exchange, progress,
  filesystem completion, writer commit, journal acknowledgement and reclaim phase.
- Lost writer/client replies reconcile by request ID without a second mutation;
  repeated shutdown/interrupt retains ownership until the actual worker acknowledges.
- Exercise keep/rollback through actual CLI/TUI/API/SDK, including staged undo and
  inactive branches, reader protection and cap pressure.
- Linux x64/ARM and macOS Intel/ARM native positives and denials; explicit Windows
  results/limits; existing Git compatibility and shared binary recovery regressions.


Implementation note (2026-11-06): committed file-copy recovery requests now expose
an explicit metadata-only acknowledgement operation. New durable receipts bind an
immutable resolution digest; older receipts lacking it cannot authorize new journal
writes. The worker observes the exact committed owner pair, retains native locks,
and checks current authority before publishing child and parent acknowledgements.
Lookup remains read-only. Ordinary file-copy restore reconciliation now uses a receipt binding the immutable
intent and exact paired Session event batch. Pre-origin cuts, Git integration and
reference-aware reclamation remain pending implementation.


Ordinary restore acknowledgements use a distinct `SnapshotRestoreReceipt`, stored
on the exact native activity in the same checked append as the Session changes.
The receipt binds the original native origin, immutable intent digest, bounded
Session sequence range and a digest of that range's event types, payloads and
causation IDs. Reconciliation observes this pair in a single database snapshot,
then reacquires the original native namespace and lock. It may publish only the
acknowledgement flag after current authority checks; it never inspects/replaces
later owner files. Historical native settlements lacking this receipt remain
inspectable but cannot acquire new acknowledgement-write authority. A stale
in-memory Session after a committed-but-lost writer reply must be reloaded before
this operation can proceed. This does not manufacture a second transcript change.


### Lifetime proof before origin publication

New file-copy workers acquire a distinct private native lifetime lock before their
runtime reservation is published or checkout acquisition begins. Its immutable
binding names the activity, Session creation, checkout, canonical/native Location,
private namespace and exact lock file/directory identities. The reservation keeps
this binding unchanged through origin publication, settlement and disposal. The
actual worker retains the lock until its earlier resource owners are dropped;
source authorization verifies the current lifetime route as well as its existing
authority. Reacquisition of the exact native lock proves only this worker's exit.
It cannot settle a Session-owned checkout lease or another native/process owner.

Construction writes and flushes a privately staged record before publishing its
stable owner name with no-replace. A cut before publication retains the unreferenced
stage; a later reservation can create its own complete lock without deleting that
evidence. Metadata is counted in the per-Location cap. GC retains all preparation
references conservatively while worker records exist; reference-aware reclamation
must eventually account for both complete records and orphan stages.

This supplies lifetime evidence for the reserved-before-origin cut. It does not
by itself admit cancellation/acknowledgement of that reservation, prove that a
missing journal had no source effects, or acknowledge managed checkout/process
uncertainty. The explicit pre-effect consuming protocol, origin-before-intent cut,
Git equivalent and complete native/client acceptance remain required. Older
reservations without this binding receive no retrospective proof.
