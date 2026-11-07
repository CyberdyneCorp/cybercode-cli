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
pre-effect cancellation is available for new version-2 lifetime-bound file-copy
workers, including a published origin with no admitted source effects. Complete
acquired version-2 checkout sets now support separate receipt-bound native
acknowledgement. Partial/corrupt constructor cuts, post-effect/owned Git integration,
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
uncertainty. The following consuming path adds positive native phase proof and
paired cancellation; independent resource ownership, Git equivalents and complete
native/client acceptance remain required. Older reservations without this binding
receive no retrospective proof.


### Consuming an exited preparation before source effects

The owner path reviews a specific native activity and confirms a fingerprint
before cancelling its preparation. Eligibility requires the positive sequence-zero
snapshot reservation, unchanged exact version-2 lifetime binding and an unadmitted
native source phase. A published origin additionally requires explicit false
source-admission evidence in both reservation and launching records. Only the
original pending record or its exact unadmitted disposal may be consumed.
Acquiring the original lifetime file proves that the actual worker has exited;
missing journal files are not used as proof. Historical reservations without native
phase proof, generic/binary activities, admitted effects and live/replaced locks
refuse. Old cancellation receipts remain readable without granting new authority.

Before the first source callback, the worker durably appends source admission to
its native activity, then publishes admission and the exact origin in its private
native phase record, then rechecks authority. Neither marker can be reset to false.
A lost database commit reply prevents callback dispatch, while its durable true
record refuses cancellation. An older database backup cannot override native true
or missing phase evidence. The compatibility migration prevents older binaries
from mutating a Store containing this protocol. The native phase is flushed before
stable lifetime publication, counts toward quota and remains protected from GC.

Confirmation preserves all source files and transcript state. A checked paired
append acknowledges only that original snapshot activity and records the request
receipt in a Session-creation/request-derived aggregate. The writer rechecks native
proof, original record, Session revision and recovery authority. Retry looks up
that durable pair before native access and refuses changed request content or
cross-activity reuse. A cancellation with an origin then writes a bounded private
`preparation.json` receipt in its exact journal directory. It never synthesizes an
intent or marks an unapplied restore completed. Explicit request acknowledgement
reconciles a missing native receipt after durable Store settlement, retaining exact
worker proof and current authority. Contradictory/corrupt intents refuse; all
original bytes remain retained. Cancelling an unstarted recovery request leaves
its original failed restore fenced. Checkout/process uncertainty stays independent;
no Session lease is acknowledged by this operation.


### Independent checkout claim acquisition

Snapshot workers and recovery-resolution workers use a dedicated host admission
port carrying the already-reserved native activity ID. The production CLI/App
adapter claims each managed checkout under that activity, independently of the
Session, binary documents, LSP and MCP owners. Retained settlement continues to
hold every actual checkout descriptor and Location activity through worker teardown.
A managed host that has not implemented this port refuses rather than falling back
to ordinary Session admission. Unmanaged hosts preserve their existing Location
activity behavior through the default adapter.

An interrupted checkout acquisition may retain an unknown activity-owned lease.
The immutable lifetime reservation precedes that acquisition, but the activity ID
alone is not a native acknowledgement receipt. This continuation does not clear
such records, infer missing claims from an empty directory or acknowledge another
Session. Exact persisted checkout file/directory identity and creation/lifetime
binding, consuming proof through Store settlement, reconciliation and real kill
cuts across multi-checkout acquisition remain required before the complete resource
recovery path is accepted. Older Session-owned unknown leases remain independent.


### Native proof retained during acquired checkout work

Each new snapshot checkout claim now records a bounded versioned binding for the
claim file, common/activity directories and managed creation record. The core lease
pins the actual directories/creation descriptor alongside its locked claim and
verifies a fresh no-follow route against those identities, exact owner/worktree,
creation digest and retained record. Unix identity includes device/inode/birth and
owner/mode; Windows uses full volume/file identity, creation time and attributes,
with reparse and hard-link refusal. Positional reads use the original locked claim
handle and do not reopen its locked byte range for reading.

The production adapter retains a dedicated metadata verification port, including
Location activity. The worker checks it at authorization, completion and immediately
before consuming settlement. The Store writer verifies that same native proof and
returns it only after the paired commit/reply, so failure rolls back both owners
and suppresses journal acknowledgement. Verification cannot call the primary
runtime/Store or launch Git/processes. Unmanaged ownership keeps its prior retention
semantics without claiming managed proof.

This is proof of an already acquired claim. It does not yet bind the checkout
receipt to the immutable snapshot lifetime for interrupted-acquisition recovery,
reacquire an unknown claim or publish new acknowledgement authority. The cut before complete native binding publication remains open. Persisting the complete
resource binding before stable publication and consuming it with durable recovery
receipts are still required. No old Session or operation claim is reclassified.


The checkout-proof compatibility migration prevents earlier writer catalogs from
opening this Store for mutation and bypassing these native verification steps.
It performs no native evidence repair or deletion.


### Atomic initialized checkout publication and lifetime context

New lifetime-bound claims carry a version-2 native binding containing the activity,
Session creation, canonical source directory and a domain-separated digest of the
complete immutable snapshot lifetime binding. Runtime admission validates every
returned managed resource against that exact context, not just an activity label.
Legacy claims remain distinct and gain no new acknowledgement authority.

Construction prepares and flushes the complete bounded record under its actual
native lock before publishing the stable activity name without replacement. A cut
before publication retains private construction evidence; no partially initialized
stable record is a valid claim. Linux/macOS use no-replace native rename; the Windows
publication path must retain an interrupted link/unlink transition as evidence and
never treat a remaining alias as a completed acquisition. Proof still verifies
creation/namespace identities and exact record contents after publication. This
step does not enable interrupted-claim acknowledgement or reclaim construction
evidence; those require consuming recovery receipts and complete reference/lease
checks in the following implementation.


Publication inventories are bounded to 4096 entries, including the new stable claim;
exhaustion retains all evidence and requires acknowledged cleanup. Bound native
records are limited to 16 KiB; unbound/generic records retain their 4 KiB limit.
Construction prefixes include the activity length to avoid prefix collisions between
valid activity IDs. A retained preparation refuses reuse of that activity even when
its stable name was never published. Reconciliation/cleanup of those preparations
is not enabled by publication alone.

Lifetime-bound durable publication currently admits Linux/macOS only. Windows keeps
legacy unbound claim compatibility through initialized no-replace link/unlink
publication, while a retained extra link refuses verification. It does not gain
lifetime-bound durability or recovery authority; that path refuses before creating
claim evidence until native directory durability has been implemented and verified.
This remains part of the explicit Windows/platform acceptance limits.


### Reacquisition and durable acquired-resource publication

An exited claim may be reacquired only from its exact version-2 binding and matching
immutable lifetime context. Reacquisition opens existing native evidence without
creating a name, resetting settled state or acknowledging a Session/process claim.
It retains the original lock and metadata pins; live, missing, replaced, aliased,
legacy or foreign evidence refuses. The read-only proof is distinct from an ordinary
claim and cannot be passed to generic settlement.

Before a ready worker can publish an origin or dispatch source effects, its actual
complete acquired binding set is published under the private lifetime phase. Native
publication is bounded/quota-checked, cannot replace a different set, and cannot add
references after source admission. Empty complete sets remain distinct from absent
or incomplete publication. Managed source admission requires complete publication.
This resource set is retained alongside source admission/origin and remains protected
for later consuming receipt reconciliation. The current step does not acknowledge
unknown claims or infer completeness from missing files; pre-publication construction
cuts and complete reference/cleanup acceptance remain required.


### Consuming acquired-resource evidence

Preparation review and its committed cancellation receipt bind a versioned digest
of the complete persisted checkout set, including the exact immutable lifetime.
Recovery reacquires those original claims read-only and retains their actual locks
and native metadata pins through the consuming Store commit and observed reply.
The writer revalidates both the retained set and native publication. Missing,
foreign, live or changed evidence refuses without a consuming receipt. Historical
receipts lacking the digest remain readable but cannot acquire new checkout
acknowledgement authority. Request lookup remains database-only. Explicit metadata
reconciliation must match the committed digest before accessing original claims.
This binding does not by itself acknowledge an unknown native checkout claim.


### Native acknowledgement of consumed checkout claims

After observed durable preparation cancellation, the retained original checkout
proof may publish a separate bounded acknowledgement record. It binds the exact
version-2 claim, the consumed complete-resource digest and a versioned digest of
the complete committed preparation receipt. Original claim bytes are never reset
or rewritten. Publication holds the original lock, stages and flushes a complete
private record, publishes without replacement and synchronizes the directory.
The writer also checks existing journal cancellation metadata and retains any
existing checkout marker against the proposed receipt
before commit; a database backup cannot replace a different native receipt.
The acknowledgement retains its own native file identity and construction name;
a replacement, alias, changed context or different receipt refuses.

Removal admission may recognize that exact acknowledged claim while holding its
original lock and native pins. It still checks every independent Session/process/
checkout record. A partial acknowledgement stage never supplies authority. A
retained stage with the exact new acknowledgement-construction name is metadata
only after a matching stable acknowledgement exists; removal pins it without
deleting it. Orphan/foreign stages, unacknowledged claim construction, legacy claims
and unknown records retain their fences. All payload and construction reclamation
remains subject to the separate reference/reader/lease-aware cleanup protocol.
Retry lookup never publishes metadata. Explicit acknowledgement reconciliation
reacquires the original lifetime/claims, attests the committed pair and resource
digest, then idempotently completes journal and checkout metadata. A crash during
one resource acknowledgement leaves completed markers and retained stages, and the
same receipt can resume without repeating source effects. Unsupported lifetime-bound
native directory durability refuses before metadata creation.

A later authorized managed-worktree removal retains the original creation record
under its creation ID when native checkout acknowledgements reference it. The
no-replace archival rename preserves its native file identity and bytes. Removal
retries may verify that archived record through the same retained creation-directory
identity; ordinary worker reacquisition still requires the live creation route.
This preserves acknowledgement proof across removal completion/recreation without
relaxing any independent unknown claim or reclaiming referenced creation evidence.

Complete resource publication precedes checkout admission settlement as well as
origin/source dispatch. A publication failure leaves every acquired claim
unsettled. A cut after complete publication and before checkout settlement has
positive exited-lifetime/resource evidence for the new metadata acknowledgement;
partial acquisition and corrupt/incomplete records remain fenced until their
separate exact recovery proof is implemented.


### Exact Git image materialization

Git checkpoint images used for planning or owned-journal materialization must resolve
one exact literal path and retain its validated object ID and mode. NUL-delimited
listings must match that requested path, reject ambiguous/malformed records and
unsupported entry kinds, and never substitute a pathspec match. Blob contents must
match the Git SHA-1/SHA-256 object ID before becoming restore or recovery authority.
Existing historical exclusion evidence remains mandatory. Corrupt images fail
before source replacement; this does not enable owned Git journaling by itself.

Git image preparation also verifies exclusion-tag identities and the complete eligible
scoped directory-tree graph before deriving changed paths. A verified in-memory
catalog binds literal paths, modes and leaf object IDs to that graph, so a corrupt
nested tree cannot make a restore appear to have no changes. Metadata reads are
bounded and over-limit subprocess output is killed and awaited. This catalog is
preparation input for the future Git journal bridge, not native origin authority;
source mutation must still run under the retained synchronous filesystem worker.


### Git journal handoff

Git object preparation may read only immutable private snapshot objects before the
synchronous native source callback. The bounded handoff retains original target,
recorded and expected Git identities, exact worktree/Location prefix, historical
exclusions and verified images; it must not manufacture copy snapshot identities.
Native import reads/revalidates source only inside the existing retained worker and
uses the same durable change/preimage/ownership journal. A distinct journal context
version binds Git identity and Location. Legacy copy contexts keep their existing
encoded identity. Git admission remains disabled until symbolic links, full expected
state revalidation and all native recovery/acknowledgement paths are integrated;
regular-file import tests alone do not authorize that switch.


### Native symbolic-link images

Native Git journals distinguish regular file bytes from symbolic-link target bytes.
Link observation pins the link inode itself without following its target, binds
identity/generation, owner/mode and bounded target bytes, and refuses hardlinked or
unsupported metadata. Source and private slots use the same typed observations;
regular-file legacy encodings retain their original identity. New link-capable
intents are versioned separately and cannot be read as legacy file-only proof.
Staging and exchange operate on the link namespace entry, never the target file.
Recovery review identifies the image kind. Rollback reads exact retained target
bytes under the original native observation before independent intent preparation.
New Git-desired links must stay inside the worktree and outside protected/private
roots; rollback may reinstate an exact owned preimage without following it. This
does not authorize broad Git admission before full state/lifetime/acceptance gates.


### Native pre-effect Git namespace inventory

The synchronous Git importer inventories the eligible Location namespace rather
than only names present in prepared trees. Current Git rule inputs cover the global
exclude file, shadow info/exclude, ancestor .gitignore files and nested .gitignore
files; configuration locators come from the trusted adapter, never a recovery
journal. These read-only configuration inputs reproduce existing Git reads and
grant no source mutation or recovery authority. Source rule files are traversed
without following directory/file links; Git-ignored .gitignore symlinks contribute
no rules. Inputs and traversal are bounded, and input hashes are checked again
before intent publication. Protected/private source namespaces remain excluded.

Historical exclusions constrain target/recorded mutation separately from the
expected snapshot's own exclusions and images. Formerly excluded files that are
currently eligible therefore still participate in stale-state detection without
becoming mutation targets. Known expected exclusions are checked for disappearance
and file/directory changes. Current eligible leaf names are compared with expected
names; empty directories do not invent Git images. Directory-generation marks are
revalidated after content planning and before intent publication. None of these
read-only marks grants native mutation, acknowledgement or reclamation authority.

Version-1 tags do not bind all rule inputs or raw working-tree representations to
the capture epoch. Git's directory-compacted exclusion list is not a complete
ignore oracle, and a changed ignored inventory can change a tag without changing
its eligible graph. Snapshot-bound policy/raw-state proof, complete exclusion
identity parity, attribute/filter and remaining Git compatibility, lifetime and
acceptance gates are still required before admitting ordinary Git workers.


### Capture-bound Git policy fingerprints

New supported native captures bind a versioned fingerprint of the effective Git
ignore settings, read-only rule input hashes and canonical ignored namespace names
into an immutable version-2 checkpoint tag. Policy is collected before and after
Git capture; a changed successful collection refuses publication. Unsupported
collections retain a readable legacy checkpoint with an explicit unavailable
diagnostic, never fabricated policy proof. Version-1 tags and bare trees remain
readable without acquiring that evidence. Native inventory compares the current
fingerprint with the expected tag before journaling. This is a policy/state-input
precondition, not worker, mutation, acknowledgement or reclamation authority. Raw
working-tree representation and the remaining admission/compatibility gates remain
required; a policy fingerprint alone does not activate ordinary Git recovery.

### Exact captured Git working-tree representations

Supported native captures SHALL bracket canonical Git capture with bounded,
no-follow reads of eligible regular files and link target bytes, refusing changed
successful captures. A version-3 tag SHALL retain a typed envelope tree linking
both the canonical Git tree and a raw representation tree. Both graphs remain
reachable from the checkpoint ref. Legacy consumers SHALL resolve the canonical
tree; native materialization SHALL use verified raw images without running clean
or smudge filters inside the filesystem worker. Historical IDs/exclusions remain
intact. Unsupported capture SHALL retain explicit unavailable proof and SHALL NOT
silently promote canonical blobs into raw-state evidence. This does not admit
ordinary Git workers or resolve their lifetime, acknowledgement, configuration
race, reclamation or complete compatibility/acceptance gates.

Managed Git preview SHALL use the same native raw-image planner as restore,
including expected-state/policy revalidation, historical exclusions, allowed paths
and all-plan conflict refusal. Preview SHALL publish no journal or source effects.
Its client path SHALL remain behind the ordinary owned-Git availability gate;
connecting preview alone SHALL NOT enable native source admission. Raw changed-path
selection SHALL preserve normalization-only changes when managed Git is admitted.

Git Locations MAY reserve the same exact version-2 native lifetime metadata as
file-copy Locations before resource acquisition. Reservation SHALL validate the
canonical Location, protect private roots, retain the live inode lock and refuse
foreign owner/creation context. Reacquisition SHALL read the exact original record;
a requested managed worktree SHALL NOT accept an empty checkout set. Reservation
alone SHALL NOT grant source admission, journal acknowledgement, checkout settlement
or reclamation authority. Ordinary owned Git availability remains separately gated.

New ordinary restore consuming receipts SHALL bind the exact version-2 lifetime
and a versioned digest of the complete retained checkout set when native lifetime
proof exists. The paired writer SHALL revalidate that complete set before commit;
lookup SHALL verify the receipt's lifetime against the original native history.
Historical receipts without this field remain readable without gaining checkout
acknowledgement authority. The receipt field is evidence for subsequent exact
native acknowledgement, not publication or reclamation authority by itself.

Version-2 checkout acquisition SHALL retain the owned original claim without
rewriting its unsettled record before source dispatch. An exited source worker
without a consuming acknowledgement SHALL therefore continue fencing native
removal. The consuming writer SHALL check a proposed ordinary-restore receipt
against retained claims and any existing disposition markers using read-only
verification. Only after observing the durable pair may the native host publish
idempotent markers for that receipt. Partial publication retains original bytes
and remaining fences; a failure SHALL report committed runtime state and require
explicit metadata reconciliation, never source replay. Legacy acquisition retains
its original behavior without gaining version-2 acknowledgement authority.

Explicit reconciliation for a new ordinary restore receipt SHALL reacquire the
receipt's original lifetime and complete checkout set, revalidate durable owner
history and current authority, and publish only journal/checkout acknowledgement
metadata. Missing publication, live locks, changed resources or foreign markers
SHALL refuse. Legacy receipts retain their original metadata path without gaining
claim adoption. Managed completions lacking a journal receipt SHALL retain their
pre-effect/unknown evidence instead of committing an unacknowledgeable claim; a
positive no-effect consuming protocol remains required before ordinary admission.

An owned no-op restore SHALL publish a real bounded empty intent after complete
planning and revalidation. Its native origin, empty ownership section and durable
completion SHALL use the same consuming-receipt/checkout acknowledgement path as
other owned restores, without calling source admission or installing files. An
empty plan does not justify a synthetic journal ID or invented historical proof.
Legacy unowned no-op calls retain their former no-journal behavior. Lifetime and
journal namespace identities refer to different native directories; each is
verified by its native guard, and the receipt binds both rather than equating them.

Before dispatching a resolution, a version-2 interrupted owner SHALL have its
exact original lifetime and complete published checkout set reacquired read-only
and retained alongside the distinct resolution owner's resources. Verification
SHALL continue through source operations and consuming commit. Live, missing,
foreign or changed proof refuses before resolution dispatch. Legacy owners SHALL
NOT acquire retrospective checkout proof. Completion disposal SHALL release native
result guards before the worker's lifetime locks so guard cleanup cannot outlive
the native ownership proof. Retention alone grants no checkout acknowledgement.

New consuming recovery receipts SHALL bind the resolution origin and its complete
version-2 checkout evidence, plus original evidence whenever the interrupted owner
has version-2 proof. Both native claim sets SHALL be verified at the paired writer.
Only an observed durable pair may publish matching resolution markers followed by
original markers. The checksum SHALL cover both roles and the immutable native
resolution digest. Native journal verification remains responsible for its own
immutable intent; runtime checkout evidence is additionally attested by the writer.
Legacy receipts omit the section and gain no marker authority. Missing original
version-2 proof cannot be silently downgraded to legacy evidence.


### Git repository format routing

Read-only presence of an ancestor .git marker SHALL prevent an unsupported or
unreadable Git repository from being admitted as an ordinary non-Git Location.
This guard SHALL NOT parse untrusted markers into mutation authority or create
project identity/cache files. Shadow initialization SHALL query the exact user's
common repository object format with bounded read-only Git metadata output,
initialize a matching SHA-1 or SHA-256 private repository, and refuse an existing
shadow with a different format before capture/index mutation. Public capture of a
SHA-256 user repository SHALL retain its Git identities rather than silently
returning copy identities. Source admission, native acknowledgement and reclamation
retain their independent gates. Unsupported formats SHALL fail explicitly rather
than selecting a different backend.


Retained historical copy checkpoints SHALL remain readable by their typed IDs for
changed-path, diff and exclusion inspection after Git discovery is corrected.
These reads SHALL require an existing Location-bound store and SHALL create no
replacement namespace. Comparing a copy checkpoint to a Git checkpoint SHALL
refuse explicitly rather than return an empty difference or reinterpret IDs.
This read-only compatibility grants no cross-backend restore/migration authority;
that acceptance remains required before complete managed Git delivery.


### Capture-bound Git configuration locators

The policy handoff SHALL bind the configuration roots, active origin files and
include targets that selected ignore locators, case behavior and includes. Roots
include Git-reported system/global candidates even when absent, the private shadow
config/config.worktree and HEAD, and explicit environment config paths. Included
paths SHALL be expanded by Git, interpreted relative to their origin file and
bounded together with origin metadata. Relevant inherited environment SHALL be
bound by a digest; persistent handoff evidence SHALL NOT retain full configuration
file contents or cleartext environment values. Resolved policy paths remain part
of the private handoff.

Resolution SHALL bracket selected-value queries with the same bounded origin/include
inventory and stable file observations. Native inventory SHALL revalidate exact
configuration file state and environment before reading rules and again before
intent publication, without spawning Git from the synchronous filesystem worker.
Changed locators, missing-file creation, include changes and HEAD-dependent policy
changes SHALL refuse before source effects. Configuration origins grant read-only
policy evidence, never source mutation or checkout acknowledgement authority.
Unsupported metadata/capabilities retain explicit unavailable native proof and
legacy-readable capture behavior. A new policy fingerprint frame SHALL distinguish
this proof; old checkpoints SHALL NOT be retroactively upgraded.


Physical worktree/private-Git-directory identities and canonical routes SHALL be
bound because conditional Git-directory includes depend on that context. Selected
values SHALL come from the bound origin stream; Git path/boolean expansion SHALL
use explicit supplied overrides rather than reselect mutable settings in separate
queries. Include path expansion SHALL be batched and bounded. Named-user home
expansion without native account-database proof SHALL remain unavailable. Metadata
subprocesses SHALL have a bounded read/wait deadline and SHALL be killed and awaited
on timeout or output failure; this does not claim that every legacy capture/filter
subprocess has the same deadline or native source ownership.


The current prerequisite supports unconditional includes and Git-directory/branch
conditions in the application-created standalone private shadow with an unborn
HEAD. Its native checks decline common-dir/worktree/reftable substitutions, linked
HEADs and loose/packed branch references rather than inferring an unknown reference
store. Ordinary packed snapshot refs remain readable and may change through Git GC.
Remote-URL conditional dependencies and named-user home expansion lack complete
native dependency proof and SHALL retain legacy-readable captures with unavailable
native policy evidence. Full configuration/dependency/race and Git compatibility
acceptance remain required before managed source admission; these observations do
not claim an atomic snapshot of every external configuration input or independent
permission authority.


### Mixed historical image provenance

Native Git planning SHALL be able to use retained Location-bound copy target or
recorded images with a fresh raw Git expected checkpoint. It SHALL retain all
original checkpoint IDs and verify copy manifests/objects using their BLAKE3
identity; no copy image SHALL be reinterpreted as a Git object or relabeled as a
Git checkpoint. The expected checkpoint SHALL retain complete current Git policy,
raw-image and namespace revalidation. Both backends' historical exclusions SHALL
constrain mutation while current expected images remain part of stale detection.

A new version-6 Git journal context SHALL bind explicit target/recorded/expected
image-source kinds to the original IDs and common Git worktree/Location scope.
Version-4/5 Git contexts SHALL keep their original all-Git interpretation and byte
encoding; source annotations cannot grant mixed-image authority to an old record.
Version-6 immutable digest framing SHALL be distinct. Complete existing copy
manifest validation, byte/path/count budgets, scoped selection, native preimages,
conflict refusal, worker ownership, acknowledgement and recovery remain mandatory.
This image bridge alone SHALL NOT enable managed Git source admission or substitute
for runtime/client/migration and crash/retention acceptance.

### Generated Git exclude source binding

Native policy proof SHALL bind the user common-repository `info/exclude` source,
including absence and physical common-directory identity, to the exact generated
shadow exclude bytes. A bounded versioned recipe SHALL retain a source observation
digest, capture settings, skipped literal paths and generated-byte digest. Source
locators SHALL derive from the trusted Shadow context, never decoded recipe paths.
Collection SHALL regenerate the bytes from fresh source observations and compare
both recipe and shadow output before granting proof. The synchronous native planner
SHALL revalidate these source, recipe and output observations before effects.
Unchanged generation SHALL preserve file observations and checkpoint identity;
publication SHALL use descriptor-relative replacement without following output
links. This does not prove user-index or large-untracked eligibility changes;
those and full configuration/filter compatibility still gate managed Git admission.

### Typed recovery CLI dispatch

`cyber sessions recovery <session>` SHALL list retained journals without inference.
Providing `<journal> --action keep|rollback` SHALL review only; `--confirm` SHALL
require `--request-id` and the exact reviewed fingerprint. The parser SHALL refuse
incomplete/invalid confirmation combinations before service startup. The separate
`cyber sessions recovery-request <session> <request-id>` SHALL perform lookup only;
`--acknowledge` SHALL explicitly reconcile that committed request's native metadata
through the existing route. Errors after confirmation SHALL retain a lookup hint
for the exact request. Text and JSON SHALL distinguish filesystem completion,
durable runtime state and observed native acknowledgement, including unavailable
historical evidence, conflicts and retained created parents. These clients SHALL
NOT manufacture origin proof or bypass unavailable backend/runtime admission.

### Recovery request discovery and terminal panel

A bounded authenticated read-only request-discovery operation SHALL expose retained
confirmation identities for an exact Session creation and journal. It SHALL verify
native request ownership and SHALL NOT inspect source files, acknowledge work or
replay a request. A fresh terminal SHALL use discovery to find previously reserved
requests after reply loss; client memory SHALL NOT be the sole retained identity.
The rewind panel SHALL link to recovery with `C`, list journals and their retained
requests, review explicit keep/rollback selections, and require a second Enter for
the reviewed fingerprint. Request lookup and native acknowledgement reconciliation
SHALL remain separate explicit keys. Session/Location/UI-generation changes SHALL
invalidate stale replies without changing the composer or clearing native fences.
