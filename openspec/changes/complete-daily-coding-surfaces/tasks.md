## 1. Review and prerequisites

- [x] 1.1 Inspect rewind, non-git snapshot, rule/context and structured-output/JSONL contracts and existing reusable machinery at `097b6278`.
- [x] 1.2 Draft proposal/design/scenarios and ordered tasks; identify destructive-revert wording and retry-count inconsistencies.
- [x] 1.3 Review artifacts with the user, including platform limits, the additive instruction_limits setting, schema/JSONL bounds, multi-input exits and interrupt/EOF semantics. Approved on 2026-10-27: after PRs #53/#54 merged, the user explicitly requested implementation of both merged plans and then instructed goal resume. Native proof, baseline and acceptance gates remain required.
- [x] 1.4 Record the 2026-11-03 user-approved priority change, establish relevant unchanged-main baseline checks and current clause coverage, and retain unresolved Plugins/hooks dependencies without claiming acceptance. The scheduling prerequisite is superseded. Baseline and current clause coverage are recorded in implementation.md; incomplete native/plugin guarantees remain explicit.

## 2. Rewind and undo/redo

- [x] 2.1 Reconcile Three-phase revert, Rewind targets, Session branches and structured retry wording in canonical specs and overlapping deltas without deleting historical recovery evidence.
- [x] 2.2 Add typed CLI rewind and server preview/revalidation/confirmation using existing owned revert operations; regenerate command docs and API/SDK where needed.
- [x] 2.3 Implement TUI rewind menu, Esc-Esc, `/undo` and `/redo`, composer restoration, explicit busy interruption, conflict/exclusion/effect preview and stale-context fencing.
- [ ] 2.4 Test code/conversation/both, snapshots disabled, inactive branches/export, external edits, busy/interrupt races, restage/clear/restart and retained unknown-effect evidence through actual clients.

Task 2.4 evidence now includes real service kill/reload/restage/clear, disabled snapshots, no-Git conversation continuation/export, explicit interruption and reopened-terminal draft preservation. Native partial-replacement cuts and complete journal/recovery acceptance remain open; see implementation.md.

## 3. Non-git snapshots

- [x] 3.0 Review the concrete [managed-recovery ownership protocol](managed-recovery.md), including exact native acknowledgement, legacy journal limits and two-phase reclamation, before changing recovery admission. Approved on 2026-11-06: "Approve this protocol and implement."

- [x] 3.1 Add private content-addressed BLAKE3 objects and Location-bound manifests behind the snapshot interface, including ignore/size/metadata reporting and same-size/same-mtime verification.
- [ ] 3.2 Implement conflict-aware owned restore and recoverable backups/journal, preserving Git behavior, protected paths, symlink/hardlink safety and interruption evidence.
- [ ] 3.3 Integrate 200 MiB cap, visible unavailable/overflow state, reference-aware retention and staged/branch/recovery protections.
- [ ] 3.4 Test real non-git editing/restoration, limits, deletes/renames, unsupported files, concurrent Session/editor changes, crashes and native filesystem privacy.

### Managed recovery implementation and acceptance map

These steps expand 3.2–3.4 and the remaining native cuts in 2.4. They do not
select the protocol by themselves. The user approved 3.0 on 2026-11-06. The
source map was inspected on main `db9e171e`; this breakdown is not evidence of
recovery implementation.

- [x] M1 Record the user's 3.0 decision in managed-recovery.md and reconcile any requested revisions with the snapshot spec delta before implementation. Approved without protocol revisions on 2026-11-06.
- [ ] M2 Add immutable snapshot-worker origin and request ownership to the runtime activity and snapshot ports. Retain actual lifecycle/checkout/store/mutation owners inside the synchronous worker through its last filesystem call. Test request disposal, cancellation, shutdown and a foreign/live worker; never reuse binary-document or generic-process acknowledgement authority.
- [ ] M3 Add versioned origin-bound Git and file-copy intents, preserved displaced preimages and resolution-intent records. Keep legacy journals inspectable without inventing origin proof. Test missing/corrupt records, private namespace replacement, aliases, metadata and Location mismatch before admitting effects.
- [ ] M4 Implement bounded keep/rollback planning and fingerprint revalidation under exact native locks. Preserve unrelated edits, refuse the complete plan on conflicts, and retain any interrupted resolution as independent evidence. Test keep and rollback after actual partial namespace replacement, including staged undo and inactive branches.
- [ ] M5 Atomically validate and commit the exact origin acknowledgement and resolution receipt at the Store writer. Determine the bounded owner aggregates before extending the single-aggregate append API. Test late CAS/projector/validation failure rollback, absence of uncommitted notifications and committed-but-lost writer replies; repeat request lookup must never mutate files again.

M5 implementation now uses `Store::append_pair_checked` for the exact original
native owner and the resolution request owner. A Session-creation/request-derived
reservation precedes native work, and the consuming writer checks captured
recovery authority, current Session revision, both immutable origins and the exact
request/receipt. A WAL-hook reply-loss regression reopens the actual Store and
finds both committed owners without notifications or replay. M5 remains open for
complete failure/reconciliation and native acceptance; the primitive alone did not
satisfy these obligations.

M4/M7 continuation: review, confirm and request lookup HTTP/SDK operations now
exist for the file-copy backend. Dedicated snapshot-only admission preserves
captured closed boundaries; ordinary native/hook admission is fenced at the writer.
Actual CLI/API service-kill cases resolve a partial namespace with keep and
rollback, preserve unrelated/later owner files and the transcript, and repeat only
lookup after restart. This is local non-Git evidence, not complete operator/client
or platform acceptance. Typed CLI/TUI recovery, pre-origin cuts and Git-native equivalents remain required. Explicit recovery-request
acknowledgement reconciliation now uses the exact committed pair and persisted
immutable resolution digest; it changes only journal metadata under retained native
locks and never rescans or reinstalls source files.
Ordinary rewind commit still refuses recovery receipts. Filesystem completion alone
never clears a native fence. Created source directories remain protected for safe
acknowledged cleanup.

Late original-scope disposal now checks durable identity and status within the
writer transaction after native store-lock release. It preserves an already
acknowledged activity and its complete receipt; changed ownership/sequence refuses
the append. The consuming recovery pair now removes only its exact snapshot
reason after observed durable commit, preserving opaque/generic Location
uncertainty. Snapshot uncertainty is tracked
by exact activity ID separately from the opaque flag; reload reconstructs generic
fences and snapshot owners independently. A late-scope no-op clears only its exact
snapshot reason after observing the matching durable acknowledgement at the writer.
Explicit recovery-request reconciliation distinguishes a durable writer
acknowledgement with missing journal acknowledgement from an uncommitted filesystem
outcome. Regression coverage refuses the latter before calling the native adapter,
and preserves later owner edits, including a source replaced with a symlink.
Older receipts without an immutable resolution digest cannot authorize new journal
writes. New ordinary file-copy restores now persist a separate immutable intent/Session-batch
receipt in the paired append. Its lookup and metadata-only acknowledgement path
checks the exact batch without replay. Historical settlements lacking that proof
cannot authorize new journal writes.
Unprepared reservation/origin/intent cuts still require exact resource ownership
proof; missing records alone do not license acknowledgement.

- [ ] M6 Publish journal acknowledgement only after observed durable Store settlement. Add reference/reader/lease-aware eligibility and durable two-phase reclamation with retained retry receipts. Test cap pressure, disabled/zero retention, incomplete reference views and crashes at writer/journal/reclaim boundaries without deleting unresolved evidence.
- [ ] M7 Add owner recovery review/confirm/lookup HTTP routes, typed CLI and SDK operations, and TUI recovery entry from rewind unavailability. Regenerate OpenAPI/SDK/command artifacts; exercise the real clients with no inference or transcript movement and with stale/foreign request refusal.
- [ ] M8 Run process-kill cuts before/after intent, stage preparation, every exchange, progress, filesystem completion, writer commit, journal acknowledgement and reclaim. Obtain Linux x64/ARM and macOS Intel/ARM positives/denials, explicit Windows results/limits and shared Git/binary regressions; update evidence and perform substantive code/security review before accepting parent tasks.

Lifetime continuation: new file-copy runtime reservations now retain an exact
worker-specific private lock acquired before reservation and checkout admission.
The actual worker keeps it through resource teardown, and source authorization
revalidates its native route. Lock metadata and orphan stages count toward the cap
and conservatively protect GC. Native tests reacquire only after actual process
death and preserve an interrupted pre-publication stage. This is a prerequisite,
not completion of all pre-origin recovery. The following continuation adds the
consuming path only for a positively bound reservation with no published origin.
Positive source-admission phases for origin-before-intent, independent managed-
checkout proof and Git equivalence remain open; no force-clear is enabled.

Implementation source map:

The non-Git reviewed-confirm path now uses a runtime-issued synchronous worker,
origin-bound intent and paired Session/native commit followed by journal
acknowledgement. The recovery path adds globally reserved request ownership and
paired original/resolution settlement. M2–M8 remain open: Git and legacy entry-point
integration, pre-origin resource proof, cleanup,
complete clients and native/platform acceptance are still required.

New version-3 file-copy intents retain native per-change source/staged file and
parent identities, including privately staged identities for new parents before
publication. Read-only inspection classifies actual source/private slots without
using progress as proof; five storage-level process-kill cuts verify this after
fresh ownership. Legacy version-1/2 intents have no historical per-file proof.
Do not infer rollback ownership from matching bytes or missing progress. Git
equivalence, the remaining runtime/crash boundaries and the complete native platform
matrix remain open with M3/M8. Two real non-Git CLI/API scenarios now kill the
service after a partial exchange, restart, keep/rollback and verify lookup without
replay; these do not cover every cut or reclamation.
Crash acceptance must also cover reservation before origin publication and origin
publication before intent publication. Those durable fences cannot be force-cleared
or resolved by inventing a missing journal; implement exact pre-effect ownership
proof/reconciliation for these cuts before claiming complete managed recovery.

- `cyber-server/src/runtime/activity.rs` owns native activity records; `binary_documents.rs` retains a real synchronous worker's resources and `binary_recovery.rs` demonstrates document-specific recovery proof. Snapshot workers require their own origin/proof rather than calling binary recovery.
- `cyber-server/src/runtime/host.rs::Snapshots` exposes restore, recovery inspection and owned resolution; `rewind_preview.rs` owns reviewed rewind confirmation. `snapshot_recovery.rs` owns durable recovery admission/lookup, with snapshot-only captured authority and the paired native consuming commit in `activity.rs`.
- `cyber-snapshot/src/copy/restore.rs` reads legacy version-1/2 intents and writes version-3 native ownership records. `copy/ownership.rs` prepares file/parent evidence and inspects actual slots; `copy/storage.rs` owns private descriptor-relative I/O. `restore.rs` and `git_history.rs` cover Git restore and historical exclusions. Keep historical evidence readable as the new protocol is added.
- `cyber-snapshot/src/copy/resolution.rs` freezes native observations, reads retained preimages, publishes independent intents and rejects request replay. Acknowledged child journals settle incomplete parents through validated links without inventing original completion. The retained worker guard pins immutable intent/receipt data. Origins bind the exact private lock inode; older origins without that proof remain inspectable and fenced.
- `cyber-store/src/store.rs::append_pair_checked` commits the bounded original/resolution owner pair. Independent single-owner appends cannot establish this atomic acknowledgement.

## 4. Rules and context inspection

- [ ] 4.1 Implement trusted path-scoped rule discovery/precedence, validated globs, overrides and bounded instruction assembly without changing `instructions` array compatibility.
- [ ] 4.2 Bind reminder delivery to successful authorized operations and Context Epoch identity, including edits/patches, partial settlements, compaction/restart and changed/revoked rules.
- [ ] 4.3 Add authenticated read-only context inspection and `/context`, source estimates/origins/status/pending updates and honest utilization; synchronize OpenAPI/SDK and doctor diagnostics.
- [ ] 4.4 Test no rule leakage across Location/Epoch, denied/failed calls, truncation, untrusted sources, stale response and token-count consistency with actual request assembly.

## 5. Root structured output

- [ ] 5.1 Add validated per-prompt root format admission, bounded schema compilation and durable schema/attempt ownership without changing existing child contracts.
- [ ] 5.2 Add `structured_output` finalization, two correction retries, budget/interrupt handling and durable successful/error result settlement.
- [ ] 5.3 Add `--output-schema`, API/SDK result fields and text/JSON/stream rendering; preserve unstructured commands and validate option combinations before admission.
- [ ] 5.4 Test valid/invalid schemas, malformed results, retry exhaustion, missing finalizer, compaction, restart/retry identity, budget exits and independent queued prompts with a real server/scripted provider.

## 6. Streaming JSONL input

- [ ] 6.1 Define typed version-2 envelopes, correlation, resource bounds, aggregate exit rules and driver-owned interactive request lifetime; document compatibility and unsupported combinations.
- [ ] 6.2 Implement concurrent bounded parsing/admission and exact owned permission/question replies, retained request lookup, interrupt and end/EOF draining without stopping unrelated work.
- [ ] 6.3 Emit per-input results, optional partial/hook events and atomic last-message output with bounded backpressure and secret-safe errors.
- [ ] 6.4 Test multiple inputs, steering/queueing, malformed/oversized input, capacity, stale/cross-request replies, EOF while awaiting input, slow stdout, SIGINT, server restart and lost admission acknowledgement.

## 7. Review and delivery

- [ ] 7.1 Update guides, roadmap/feature evidence, original task lists and generated API/SDK/CLI artifacts for actual delivered behavior.
- [ ] 7.2 Run affected Rust tests, format/Clippy, SDK typecheck/tests, TUI/real-binary acceptance, strict OpenSpec/lint and relevant native/platform CI. Record skips and unperformed external checks explicitly.
- [ ] 7.3 Use the code-review skill for substantive review, address findings and deliver reviewable PRs. Merge only when authorized; archive after accepted merge and overlap reconciliation.
- [ ] 7.4 Verify all approved first- and second-workstream outcomes against evidence before marking the combined goal complete.


Reserved-before-origin continuation: list/review/confirm/lookup operations now
consume the exact exited worker lifetime proof and atomically pair only that native
acknowledgement with a retained cancellation request. A real service kill before
origin publication preserves later symlink/owner edits and the transcript, supports
retry after restart and admits a subsequent prompt in an unmanaged non-Git Location.
Cancelling an unstarted resolution reports `cancelled_before_origin` without
acknowledging its original failed restore. Origin-before-intent, independent checkout
resources, complete clients, Git, reclamation and the remaining M8 cuts remain open.


Pre-effect origin continuation (2026-11-06): new file-copy workers publish version-2
native source-phase proof before their stable lifetime reservation. A durable
activity admission marker and native true phase precede the first source callback;
missing/legacy phase proof and database/native disagreement refuse cancellation.
The owner cancellation now accepts an exited published origin only with explicit
unadmitted evidence, commits its paired receipt, then publishes bounded journal
cancellation metadata. Explicit preparation acknowledgement reconciles that metadata
without source effects. Original intents and private preparations remain retained.
Local native missing-intent and real-service origin-before-source-admission cuts
are covered; the real-service cut does not assert that intent publication was absent.
Independent checkout resource ownership, Git journaling, cleanup, complete clients
and the remaining M8 cut/platform matrix still prevent acceptance of M2–M8 and
parent tasks 2.4/3.2–3.4. No additional parent task is marked complete.


Independent checkout acquisition continuation: ordinary snapshot and recovery
workers now call a dedicated host port with their already-reserved activity ID.
The production CLI/App adapters retain separate `op_` checkout locks alongside
Session/binary/MCP/LSP claims. Managed hosts without explicit support refuse.
Retained settlement still holds actual descriptors through teardown. Regression
coverage verifies coexistence, duplicate/foreign ID refusal, removal exclusion,
unknown claim retention and preservation of unrelated Session evidence. This is
acquisition ownership, not native reconciliation authority: immutable persisted
checkout route/creation/lifetime binding and interrupted-claim acknowledgement
remain incomplete, so M2–M8 and parent tasks remain open.


Acquired checkout proof continuation: snapshot claims retain native common/activity/
creation directory pins and record exact file/creation identity. Runtime source
callbacks and consuming commits verify the dedicated native proof, including inside
the Store writer through commit/reply. Changed routes/records refuse without source
callbacks or acknowledgement. This does not finish immutable lifetime/resource
publication or unknown-claim reacquisition across early cuts. Those and complete
platform/client acceptance keep M2–M8 and parent tasks open.


Atomic lifetime-bound checkout publication continuation: new managed claims stage
and flush complete ownership records under their native lock before no-replace
publication. Version-2 claims carry exact Session creation/directory and immutable
lifetime digest. Runtime checks the complete returned resource set, including
version, foreign/missing context and duplicates, before native settlement. Read-only
inspection precedes any acknowledgement. Real native pre-publication kill and
collision tests retain evidence and refuse activity reuse. Unbound legacy claims
and Windows durability limits remain distinct. Unknown-claim acknowledgement,
full publication/recovery cuts, Git journals and reclamation remain incomplete;
no parent M2–M8 or 2.4/3.2–3.4 task is accepted here.


Read-only reacquisition and acquired-resource publication continuation (2026-11-07):
core now reacquires an exact exited version-2 checkout claim under its native lock,
retaining original metadata pins without creating, settling or changing evidence.
A distinct proof type exposes no generic settlement. Runtime publishes the actual
complete acquired binding set into the private lifetime source phase before origin
or source dispatch. Publication is quota-checked and immutable; source admission
retains references and refuses missing managed sets. Explicit empty unmanaged sets
remain distinct from absent constructor evidence. Unknown-claim acknowledgement,
partial acquisition/publication cuts, Git journals, reclamation and full native/client
acceptance remain open; no parent task is marked complete.


Consuming acquired-resource proof continuation: preparation review and cancellation
receipts now bind a versioned digest of the exact lifetime and complete acquired
checkout set. The production CLI/App host port reacquires original claims read-only;
recovery retains and revalidates those locks/pins and native reference publication
through the paired Store commit and observed reply. Missing/changed/live/foreign
proof refuses without a consuming receipt. Database-only lookup remains independent
of native evidence; explicit journal metadata acknowledgement verifies the committed
digest. Historical digest-less receipts retain only their existing journal authority.
Native unknown-claim acknowledgement is still required: this continuation does not
settle a checkout claim, delete construction evidence or accept any parent task.


Pre-effect native checkout acknowledgement continuation: observed paired receipts
with complete version-2 resource bindings now publish separate exact native markers
under retained original locks. Original claim bytes and independent unknown claims
are preserved. Read-only candidate checks at the writer reject conflicting existing
journal or checkout receipts; lookup never retries metadata. Explicit acknowledgement
reconciles interrupted journal/checkout publication without source replay. Verified
markers may admit later removal while pinning retained acknowledgement stages;
original creation records are archived without replacement when needed for removal
retry/recreation proof. Complete resource publication precedes checkout admission
settlement. Actual native pre-publication kill/namespace replacement and real Git
removal/recreation cases cover this step. M2–M8 and parent tasks remain open for
partial acquisition/corrupt construction, post-effect/owned Git integration, safe
reclamation, complete clients and full crash/platform acceptance.


Git image boundary continuation: shadow-Git blob lookup now resolves an exact literal
NUL-delimited path, validates mode/object identity and checks the typed SHA-1/SHA-256
blob hash. Malformed/ambiguous output, unsafe paths, unsupported entries and corrupt
blob bytes refuse before replacement. Real unusual-path and corrupted-loose-object
cases preserve the user's index and current source on failure. Historical exclusion
lookup remains mandatory. This is an image-materialization prerequisite for owned
Git journals, not their implementation; M2–M8 and all parent tasks remain open.


Verified Git catalog continuation: changed-path derivation now uses a bounded
Location-scoped catalog whose exclusion tag and every traversed directory tree
match their typed Git object IDs. Metadata subprocess overflow refuses after
killing/awaiting the child. Literal Location selectors preserve Unix backslashes
and glob-shaped names independently of inherited pathspec environment. Real
corrupt tag/root/nested-tree restores refuse without changing source or user index;
scoped restores preserve sibling files. This remains preparation for owned Git
journals; M2–M8 and parent tasks are unchanged and incomplete.


Git/native regular-file handoff continuation: the owned adapter now prepares a
bounded immutable Git object packet before the synchronous filesystem callback.
Native import rechecks leaf hashes and known expected paths, uses the shared CAS/
preimage/ownership engine and writes version-4 intents with original Git IDs and
exact worktree/Location context. Native fixture keep/rollback and request retry
retain that context; legacy copy encoding and fences remain distinct. Ordinary
Git worker reservation/admission is still disabled. Symbolic links, full expected
state scanning (including newly appearing paths), exact runtime Git lifetime/
receipt acknowledgement, retention and complete crash/client/platform acceptance
remain required; M2–M8 and parent tasks remain open.


Native link continuation: version-5 intents now bind backend and typed regular/link
images while preserving earlier file encodings and digest frames. Link observations
pin the link inode, reject aliases/unsupported metadata and read bounded target
bytes without following them. Shared native staging/exchange and exact-preimage
rollback support link/file transitions. Review and generated SDK expose image kind.
Local macOS native positives and five real link process-kill cuts cover inode
classification without progress/replay; these are backend fixtures, not complete
Git runtime acceptance. Ordinary Git admission remains disabled for full expected
state/Git compatibility, lifetime/acknowledgement, retention, clients and complete
crash/platform gates. M2–M8 and parent tasks remain open.


Native inventory continuation: the Git importer now compares eligible Location
leaf names, revalidates directory namespace generations and read-only Git rule
inputs, and rejects new/removed/type-changed state before intent publication.
Current expected images use their own exclusions; historical exclusions still
constrain target/recorded mutations. Native relative paths now preserve Unix
backslashes while rejecting empty/dot/traversal components. Snapshot-bound ignore/
raw-state proof, full Git compatibility, actual Git lifetime/acknowledgement, safe
reclamation and full client/platform acceptance remain open. Admission remains
disabled; no M2–M8 or parent task is accepted by these checks.


Capture-policy continuation: version-2 Git checkpoint tags now retain bounded
capture-time policy fingerprints. Native inventory compares effective ignore
settings, rule hashes and canonical ignored names before intent publication;
legacy records remain readable without native proof. Captures bracket Git tree
creation with policy collections and refuse a changed successful collection.
Application ignore/size settings and private roots are included. Raw working-tree
representation, configuration-resolution races, Git compatibility, actual worker
lifetime/acknowledgement, reclamation and full clients/platform acceptance remain
open. Git admission stays disabled; M2–M8 and parent tasks remain incomplete.


Raw-state continuation: supported captures now bracket Git tree creation with
bounded exact raw file/link reads. Version-3 tags retain canonical and raw graphs
through a verified envelope tree; native preparation uses raw graphs, preserving
original checkpoint identities and exclusion rules. Public Git diff/blob behavior
resolves the canonical graph. Legacy bare/v1/v2 checkpoints remain readable but
cannot supply missing raw native proof. Git runtime ownership/acknowledgement,
preview/path-selection parity for normalized-only changes, configuration races,
remaining Git compatibility, retention and full client/platform acceptance still
block ordinary admission. M2–M8 and parent tasks remain incomplete.


Native preview continuation: managed Git preview is connected to the same raw
planner as restore behind the unchanged owned-Git availability gate. Raw changed-
path selection is connected at the same gate, preserving normalization-only edits.
Rendering uses verified expected images rather than reopening current source, and
scoped Git paths/patch headers stay worktree-relative. Backend fixtures verify
preview leaves no journal/source changes and revalidates outside selected paths.
Complete summaries/client behavior, worker lifetime/acknowledgement, reclamation,
remaining Git compatibility and platform acceptance remain required. Ordinary Git
admission and all M2–M8/parent task acceptance remain unchanged and incomplete.


Git lifetime-port continuation: trusted callers may now reserve backend-independent
version-2 native lifetime metadata in Git Locations. Exact live locks, owner/Session
creation/Location checks and managed-checkout completeness use the shared native
engine. This lifts only the metadata reservation refusal; ordinary owned Git
source availability remains false. Actual Git worker/checkout consuming receipts,
post-effect acknowledgement/reconciliation, reclamation and complete crash/client/
platform acceptance remain required; M2–M8 and parent tasks remain open.


Ordinary consuming-receipt continuation: new restore receipts now bind exact
version-2 lifetime and complete checkout-set digest when native lifetime proof
exists. The paired writer rechecks the retained set and lookup verifies original
native lifetime history. Legacy receipt encoding remains unchanged without the
optional field and cannot generate new checkout acknowledgement evidence. This
adds receipt evidence only; exact post-effect native checkout publication and
reconciliation, actual Git runtime acceptance, reclamation and remaining M2–M8 /
parent obligations remain open. Git source availability stays disabled.


Ordinary checkout acknowledgement continuation: version-2 host acquisition now
retains original unsettled claim bytes instead of applying generic settlement
before source dispatch. The consuming writer verifies candidate markers read-only;
observed durable ordinary-restore receipts publish exact native markers. Explicit
reconciliation reacquires original lifetime/resources and never dispatches source
work. Legacy version-1 acquisition and old receipt behavior remain distinct. Missing
managed journal receipts refuse with retained recovery evidence; successful managed
no-effect completion is not implemented. Recovery-resolution receipt/checkout
integration, actual Git source/backend/runtime and complete crash/platform/client
acceptance, reference-aware cleanup and other M2–M8/parent obligations remain open.
Ordinary Git source admission remains disabled.


Owned no-op and native namespace continuation: both journal backends now retain a
real empty intent for an owned empty plan, using ordinary origin/completion and
receipt acknowledgement without source admission. Unowned no-op behavior stays
unchanged. Receipt validation no longer equates the lifetime Location namespace
with its distinct journal collection. Real CLI regressions cover owned file-copy
no-op completion, retained owner files, receipt lookup and repeated acknowledgement
without source replay/inference. The full managed Git backend/source/checkout path,
recovery-resolution resource acknowledgement, complete native cuts/platforms and
reference-aware cleanup still require acceptance; parent tasks and Git admission
remain unchanged and open.


Original-owner retention continuation: recovery now reads attested original lifetime
metadata, reacquires exact version-2 lifetime and published native checkout resources
before reserving a resolution owner, and retains both owners through work/commit.
Authorization revalidates the original set. Legacy v1/missing lifetimes gain no new
claim proof; unknown versions refuse. Completion drops native result guards before
worker lifetime locks. This is retention, not original/resolution checkout marker
authority: binding both sets into consuming recovery receipts and publishing/
reconciling their exact acknowledgements remain required. M2–M8 and parent tasks
stay open; ordinary Git source admission is unchanged and disabled.


Recovery checkout consuming-receipt continuation: new recovery receipts bind the
exact resolution origin, resolution lifetime/resource digest, and the original
version-2 lifetime/resource digest when available. Both retained claim sets are
checked read-only at the consuming paired writer, which checks both attested
lifetime histories and refuses a missing version-2 proof. Only an observed durable
pair admits exact resolution-then-original native checkout markers. Original
claim bytes stay immutable. Lookup verifies both committed owners, and explicit
metadata reconciliation reacquires both original resource sets rather than
substituting a fresh Session lease or replaying source work. Legacy receipts gain
no new claim authority. This continuation does not close M2–M8 or parent tasks:
partial acquisition, actual managed Git source integration, reference-aware
reclamation and full native/platform/client acceptance remain required.


Git format routing continuation: public capture now declines non-Git fallback when
an ancestor .git marker exists even if libgit2 cannot discover the repository.
Private shadows initialize with the user's exact supported object format and
mismatched existing shadows refuse before index mutation. Actual SHA-256 repository,
encoded-image and linked-worktree/subdirectory coverage replaces the former
manually initialized shadow-only graph fixture. Retained copy history stays readable
by typed IDs through existing namespaces; mixed-backend comparison refuses. This
does not admit managed Git source work or implement cross-backend restore/migration.
These compatibility/acceptance requirements, configuration races, partial
construction and reference-aware cleanup remain open with M2–M8 and parent tasks.


Git configuration locator continuation: supported native policy collection now
binds Git-reported root candidates (including absent files), active origins,
include targets, environment digest, exact file observations and physical directory
context. Selected values come from the bound origin inventory; batched explicit
path/boolean expansion cannot reselect a different value. Native planning rechecks
these inputs without Git subprocesses, using a new fingerprint frame that does not
upgrade old checkpoint proof. Metadata reads/waits have output/deadline limits and
kill/await cleanup. The application-created private reference-layout profile is
revalidated; unsupported topology, remote-URL conditions and named-user expansion
retain unavailable proof. This fixes the reproduced post-preparation locator change
but does not finish complete configuration/ABA/dependency, filter, actual Git
runtime/checkout, migration, crash/platform/client or reclamation acceptance.
Managed Git source admission and all parent tasks remain open and disabled.


Mixed-image journal continuation: native Git preparation can retain copy target or
recorded IDs as typed deferred manifests with a fresh raw Git expected checkpoint.
The synchronous native store loads only its original Location-bound manifests,
verifies BLAKE3 objects, enforces combined image budgets, and applies both backends'
historical exclusions without hiding expected-state checks. A version-6 context
and digest bind per-image source kinds; old versions reject those annotations.
Native preview/restore and keep/rollback, including interrupted replacement after
reopening the namespace, have backend coverage. Runtime-owned Git callbacks,
client planning/routing, complete migration/limit combinations, config/generated
rule-source and filter compatibility, native kills/platforms and reference-aware
cleanup remain required. Managed Git source admission and M2–M8/parent tasks remain
open; backend fixture origins do not constitute runtime/checkout proof.

Generated-rule source continuation: supported native policy now retains a bounded
version-1 generation recipe, original user common-repository exclude observations,
application settings, skipped literal names and exact output digest. Collection
regenerates and compares output, and native planning revalidates retained source,
recipe, output and common/info directory identity. Unchanged generation preserves
checkpoint identity; descriptor-relative output replacement does not follow file
or parent links. A new policy fingerprint frame leaves old evidence unpromoted.
This closes the reproduced post-preparation user-exclude change, including linked
worktrees, but does not prove user-index/large-untracked eligibility, complete Git
configuration/filter compatibility or actual runtime ownership. Managed Git source
admission, M2–M8 and parent tasks remain open. The next client milestone is typed
recovery review/confirm/lookup/acknowledgement on the implemented non-Git API, with
real restart/retry acceptance; it does not waive remaining Git/GC/platform gates.

Typed recovery CLI continuation: `sessions recovery` lists retained journals,
reviews an exact keep/rollback selection, and requires both fingerprint and stable
request ID for confirmation. `sessions recovery-request` performs lookup only;
explicit `--acknowledge` invokes the existing metadata-only reconciliation route.
Responses are typed and checked against the selected Session/journal/request;
missing legacy evidence and unobserved native acknowledgement remain explicit.
CLI reference and operator documentation are updated. Real-service kill/restart,
stale/foreign request, repeat lookup and later owner-edit coverage now exercise
these typed commands. This advances the restore-journal client portion of M7;
preparation/TUI clients, actual managed Git integration, safe cleanup and full
native/platform/client acceptance remain open. No parent task is closed.

Terminal restore-recovery continuation: rewind now exposes C recovery with retained
journal/request selection, explicit keep/rollback review and second-Enter
confirmation, read-only request lookup and separate A acknowledgement metadata
reconciliation. Bounded server-side discovery uses the exact Session creation,
journal and native request-owner identity, so a fresh App can find a reserved
request after losing its reply. Failed discovery cannot authorize a fresh attempt;
legacy proof, pending/unknown outcomes and unrelated fences remain protected.
Actual production-App/native tests interrupt after a proven source replacement,
exercise both keep/rollback, preserve unrelated/later owner edits and the composer,
discard a successful reply, create a fresh TUI App and discover/lookup/reconcile
without inference or transcript movement. This is native cancellation plus a fresh
App, not an OS kill of the TUI process. Updated CLI service-kill acceptance also
checks discovery after server restart and foreign-Session filtering. Preparation
clients, ordinary managed Git source integration, partial construction, reclamation
and complete cut/platform acceptance remain open; M2–M8 and parent tasks are not
closed by this client continuation.

Typed preparation CLI continuation: `sessions recovery-preparation` lists and
reviews exact candidates, then requires a fingerprint and stable request ID for
pre-effect cancellation. `sessions recovery-preparation-request` provides receipt
lookup and explicit native metadata reconciliation. Replies bind selected
Session/activity/lifetime and published-origin context; unsupported review proof
or admitted effects refuse, while historical receipts retain their separate read
semantics. Real CLI pre-origin and published-origin-before-source kill cases now
use these typed commands for cancellation, restart lookup and reconciliation,
including stale/foreign refusal and preservation of later owner symlinks/files.
This advances the preparation CLI portion of M7; preparation TUI, partial
construction, managed Git integration, safe reclamation and complete acceptance
remain open. No parent task is closed.

Preparation terminal continuation: rewind recovery now offers P for preparation
candidates and committed cancellation history, with J returning to journals.
Candidate selection discovers a committed receipt before native-proof review;
confirmation retains a fresh stable request before sending, consumes the review,
and permits lookup rather than Enter replay after reply loss. A is explicit
metadata reconciliation. Session/Location/generation guards preserve composer,
attachments and conversation. Opt-in history keeps default pending listing intact;
exact receipt discovery delegates to the validated original committed pair.
Controller and actual HTTP transport coverage accompanies native CLI service-kill
cuts before/after origin publication, including restarted history/discovery and
foreign-Session refusal. The subsequent real-binary PTY test now covers native
pre-origin and published-origin cancellation with the committed confirmation
response withheld, SIGKILL of the TUI, and fresh-process discovery/lookup/explicit
acknowledgement without another cancellation. It preserves saved draft attachment
payloads and later source symlinks/bytes, messages and inference counts. These are
local macOS arm64 positives; other platform/cut combinations remain unproven.
M7/M8 and their parent tasks stay open alongside partial constructors, managed Git
admission, safe reclamation and complete compatibility/platform evidence.

M6 ordinary-payload continuation: implement receipt-bound Store plan/completion
records, writer-serialized reference checks, retained native namespace/reader/
lifetime proof, and no-follow removal of positively owned ordinary file-copy
stages. Keep intent/lifetime metadata and CAS roots; recheck retention and refuse
unknown entries, aliases, live workers and independent uncertainty. Wire periodic
maintenance and a bounded cap-pressure capture retry. Validate real-App positives,
interrupted plan resume, missing/changed proof, disabled/zero retention and cap
pressure before accepting this path. Managed Git, recovery-resolution/preparation
construction payload cleanup and complete reclamation/platform acceptance remain
required; M6 and all parent tasks stay open.

M6 paired-payload continuation: reclaim original and resolution regular private
slots together after exact keep/rollback Store and native acknowledgement. Bind
both journals, preserve version-1 ordinary plan IDs, retain both lifetimes/phases,
and preflight complete immutable outcome/displaced inventories before deleting.
Exercise the real partial-restore kill fixture for keep and rollback, both live
worker locks, young retention, unknown entries and planned partial progress.
Construction/preparation payloads, managed resources/Git, CAS/metadata reclamation
and the complete native cut/platform matrix remain required; M6 stays open.
