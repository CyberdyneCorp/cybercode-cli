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

- [x] 5.1 Add validated per-prompt root format admission, bounded schema compilation and durable schema/attempt ownership without changing existing child contracts.
- [x] 5.2 Add `structured_output` finalization, two correction retries, budget/interrupt handling and durable successful/error result settlement.
- [x] 5.3 Add `--output-schema`, API/SDK result fields and text/JSON/stream rendering; preserve unstructured commands and validate option combinations before admission.
- [ ] 5.4 Test valid/invalid schemas, malformed results, retry exhaustion, missing finalizer, compaction, restart/retry identity, budget exits and independent queued prompts with a real server/scripted provider. Local functional cases pass: schema bounds, admission refusal, malformed/invalid submissions, retry exhaustion, missing finalizer, compaction during correction, independent queued schemas, shutdown/resume, interruption/budget outcomes, terminal writer cuts, post-commit acknowledgement loss, steering ownership, and CLI budget/timeout exit 4 and Unix SIGINT exit 130. Current-source platform acceptance remains required; this task is not accepted from local results alone.

## 6. Streaming JSONL input

- [x] 6.1 Define typed version-2 envelopes, correlation, resource bounds, aggregate exit rules and driver-owned interactive request lifetime; document compatibility and unsupported combinations.
- [ ] 6.2 Implement concurrent bounded parsing/admission and exact owned permission/question replies, retained request lookup, interrupt and end/EOF draining without stopping unrelated work. Basic parsing/admission merged in #117. The 2026-11-09 code review found that request collection and interrupt target the Session without checking the driver's admitted message ownership, and completed inputs retain their full result payloads indefinitely. Complete those ownership/resource bounds and their shared-Session/long-run regressions before accepting this task.
- [ ] 6.3 Emit per-input results, optional partial/hook events and atomic last-message output with bounded backpressure and secret-safe errors. `--include-hook-events` is not implemented: hook receipts are deliberately not on the Session stream, so the lines would have nothing reliable to carry.
- [ ] 6.4 Test multiple inputs, steering/queueing, malformed/oversized input, capacity, stale/cross-request replies, EOF while awaiting input, slow stdout, SIGINT, server restart and lost admission acknowledgement. Done: multiple inputs in order, per-input structured results, structured/plain steering while inference is gated, long-run slot retirement, malformed input (exit 2), unknown request replies, EOF while a request waits, unsupported combinations, with the real binary (`cyber-cli/tests/exec_stream_input.rs`). Open: shared-Session ownership, capacity, slow stdout, SIGINT, server restart and lost admission acknowledgement.

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

M6 prepared-cancellation continuation: bind the immutable prepared intent into
review and the durable cancellation activity, and admit version-3 target cleanup
only with persisted prepared inode identities, exact native cancellation and a
positive non-admitted phase. Retain all intent/cancellation/phase/request metadata;
unknown/incomplete construction and historical missing digest evidence cannot be
promoted into cleanup proof. Exercise an actual service kill with prepared images
while SQLite writer contention prevents source admission, then verify changed
inodes/digests, historical missing proof and durable partial cleanup. Raw partial
construction, managed/Git resources, metadata/CAS and full platform/cut acceptance
remain required; all parent tasks stay open.

User-index handoff continuation: native policy now binds the actual per-worktree
index, absence, routing files and bounded shared-index candidates, with native
read-only revalidation and a distinct fingerprint frame. Regression coverage
includes tracked-to-untracked, creation, identical-byte inode replacement, split
dependency changes and SHA-256 linked-worktree index isolation. A positive verifies
split-index metadata/bytes remain unchanged through preparation and native restore;
oversized observations retain canonical capture without native authority. Size-
decision generation, complete user config/filter compatibility, tracked large
files, runtime/checkout/recovery/cleanup and full crash/platform gates remain open.
Managed Git source admission and all M2–M8/parent task acceptance remain unchanged.

Generated size-exclusion continuation: retain size-excluded names from the same
verified recipe as generated output, include them in a distinct policy frame, and
recheck regular-file type and threshold through no-follow metadata at capture and
native planning revalidation. The reproduced shrink-after-preparation gap now
refuses before journal/source effects. Threshold/deletion/link/parent-link/FIFO
cases and still-oversized/fresh-capture historical-exclusion positives are covered.
Original classification provenance at generation, full configuration/filter/limit
compatibility and runtime/checkout/recovery/cleanup/platform acceptance remain
required; managed Git source admission and parent task acceptance stay disabled.

Classification-provenance continuation: version-2 generated recipes now bind
observed user index/routing, resolved user configuration and rule inputs. A private
classifier uses copied index/shared-index bytes and effective configuration whose
includes were resolved in the original branch/gitdir context; original metadata
stays strict. Private copied files have mode 0600 and post-read verification, with
only disposable shared-index timestamp refresh allowed. Fresh capture/preparation
requires matching provenance and skipped names; legacy/unproven recipes retain
canonical readability without native authority. GIT_CONFIG command redirection is
cleared to prevent internal configuration writes reaching an unrelated file.
Supported capture/preparation/restore and mutation-during-classification positives
and denials are covered. Complete configuration/ABA/filter/limit compatibility,
managed Git runtime/checkout/recovery/cleanup and source-bound platform acceptance
remain open; managed source admission and all parent acceptance stay disabled.

Runtime Git candidate continuation: add a non-default native-git-acceptance build
that takes the same owned restore/preview/changed-path ports and actual runtime
workers, without changing default admission. Git tracked-image reads use the raw
reader/total bound while configured max_file_bytes remains the untracked omission
threshold. Actual App cases cover root/scoped SHA-1 and scoped SHA-256, binary
tracked images above that threshold, links, Store receipts, native acknowledgement,
lookup/reconciliation and later owner edits. Root/scoped SHA-256 coordination now
shares a read-only ancestor-marker lock domain when libgit2 cannot discover it;
this is coordination, never source/permission authority. Add candidate jobs to the
native matrix, keeping their evidence distinct. Default admission, complete managed
resource/storage-root integration, keep/rollback/process-death/runtime cuts,
compatibility/reclamation and full platform acceptance remain open with M2–M8.

Managed default-storage continuation: identify ready registered source subtrees
within application data for observation/reservation while retaining private
snapshot/other-data/explicit-root boundaries. Require the actual worker's complete
native checkout claims before constructing source intents; registration alone
cannot authorize an unowned restore. Release the Session projection mutex before
host checkout-policy admission, retain its captured revision and refuse stale
admission. The non-default App candidate now exercises actual default /worktrees
creation, native resource publication, restore, unchanged user Git metadata and
reopened metadata acknowledgement without source replay. This is an ordinary
managed restore positive, not complete managed keep/rollback/process-death,
construction/reclamation/platform acceptance. Default Git admission remains off;
M2–M8 and all parent tasks remain open.

Managed Git CLI process-death continuation: the explicit native-git-acceptance
binary creates a ready managed linked checkout in default application storage,
observes a genuinely partial namespace during a 1,280-file restore and kills the
registered service. Fresh services review/confirm keep and rollback, consume
non-empty original/resolution checkout bindings and reconcile metadata without
source replay. Verify all bulk file images, an early deletion, unrelated/later
owner files, escaped newline paths, stale/foreign requests, unchanged transcript
and user index/configuration, and no inference during recovery. Reuse verified
blob bytes only within one bounded image handoff while charging/verifying every
placement; SHA-1/SHA-256 tests retain mode, byte-limit and later-corruption refusal.
The four native workflow jobs now select this candidate, but workflow edits are
not execution evidence. These two source-kill cuts do not close the full cut,
resource construction, reclamation or platform matrix; default Git admission and
all parent acceptance remain unchanged.

Resume verification (2026-11-08, macOS ARM): the repeated-blob regression passes
for SHA-1/SHA-256, snapshot all-target Clippy with `-D warnings` passes, and
format/whitespace checks pass. The candidate CLI suite reports 19 passes and one
preparation timeout in
`fully_prepared_pre_effect_cancellation_reclaims_only_persisted_target_inodes`;
that case passes alone (60.54 seconds). Managed Git keep/rollback passes, now
including an explicit assertion for the newline-containing path. This is not a
green complete-suite result. Strict validation with CI-pinned OpenSpec 1.13.2
passes all 117 items; spec lint reports zero errors and 25 warnings. The default
CLI run was stopped before execution; native CI/platform runs remain unperformed.
Fetched main is now `8266db6c`, including merged structured-output PR #116 and
stream-input PR #117. Integrate and audit their remaining acceptance obligations
before continuing sections 5/6; both issues remain open. These observations do
not close parent tasks or establish the preparation timeout's origin.

Main integration audit (2026-11-09): main `8266db6c` is merged at `2fcd745d`,
retaining the recovery branch and all seven uncommitted files. Incoming #116/#117
are now included; task 6.2 is reopened for exact driver ownership and bounded
retained state. Unchanged main passes its six JSONL tests. The integrated branch
initially fails four because optional snapshot warnings are treated as failed
inputs. After matching ordinary exec's nonfatal warning handling and adding a
real-binary regression, all seven JSONL and three structured-output CLI tests
pass. Five structured-result server and six binary-worker ownership tests also
pass, as do formatting/whitespace checks and strict OpenSpec (100 items after
incoming archives; lint zero errors/25 warnings). The focused review and remaining
scope are recorded in docs/goal-delivery.md. Full workspace/SDK/plugin/native CI
were not rerun; the prior preparation timeout and all parent acceptance gates
remain open. No goal implementation PR is open and no issue was closed.

Managed regular-file Git payload reclamation continuation (2026-11-09): ordinary
restores and both recovery owners now reacquire their complete persisted checkout
sets, require the exact already published consuming acknowledgements, synchronize
those retained markers/directories and hold all native pins through private
deletion. Missing markers never create metadata or a reclamation plan. Live
claims, changed registrations/resource digests and foreign dispositions refuse.
The existing plan/receipt formats are retained; data-only validation no longer
mistakes a managed resource set for an unsupported shape. Native phase checks
still require exact admission/origin and resource-digest evidence. Git restore
default admission remains off; typed link payloads, incomplete construction,
CAS/metadata collection and the full native/cut matrix remain open with M2–M8.

The real managed CLI keep/rollback cases now remove an acknowledgement and hold a
live claim to prove precise pre-plan refusals, then interrupt after durable plan
publication and resume reclamation without source replay. The App verifies aged
ordinary managed cleanup, reopened receipt lookup, preserved later owner symlink,
no inference and unchanged user index/configuration. The core read-only marker
requirement and persisted source-phase/digest cases pass. The native workflow
selects the new core requirement on all four platforms; edits are not CI evidence.

Local checks: all 20 candidate CLI branch/recovery tests pass with four parallel
threads; both Git App acceptance tests pass. Snapshot library: 140 passed and six
ignored subprocess fixtures; focused final marker and source-phase tests pass.
All-target Clippy for core/snapshot/server/App/CLI passes with warnings denied,
as do formatting/whitespace and strict OpenSpec (100 items; lint zero errors and
25 warnings). The preparation kill helper now gives staging an independent
bounded deadline; the service-kill probe matches the recorded server instance
rather than any listener at a reused port. An unconstrained parallel run passed
19 cases but stalled before conversation-test prompt admission; that attempt
was terminated after preserving its Store/registration and samples under
/tmp/cyber-goal-first-prompt-stall-20261109. Its isolated rerun passed. That run
is not green and its startup-stall cause remains unassigned. SDK/plugin/full
workspace and external native CI have not been rerun for this checkpoint.

Typed acknowledged-payload continuation (2026-11-09): ordinary and paired cleanup
now admits immutable symlink images through the existing no-follow native inode
pins, retaining the same complete preflight, frozen plan and per-unlink identity,
generation, route and checkout checks. App acceptance passes both managed restore
and SHA-1/SHA-256 root/scoped cases, including dangling journal targets and refusal
of equal-target replacement inodes before a plan exists. No source replay or
receipt/schema change is introduced. Cancelled Git preparation, incomplete
construction, CAS/metadata collection and parent acceptance remain open. Focused
code/security review found no additional issue in this extension; it is not a
review or acceptance of the whole branch.

Native CI run 42684837811 on 3d3471f9 passed the snapshot adapter and exact core
marker test on Linux x64/ARM, then failed managed App worktree creation because
bubblewrap was not installed. The direct Git App worker case passed on both.
The workflow now installs bubblewrap and applies the same Ubuntu user-namespace
prerequisite as ci.yml. This workflow edit is not passing CI evidence; fresh
source-bound platform results are still required.

Typed-payload local checks: snapshot library 140 passed, six ignored subprocess
fixtures; Git App acceptance two passed; exact managed Git real partial-kill
keep/rollback passed (315.50 seconds). The first expanded mixed-image CLI run
failed its 45-second first-exchange wait before reaching cleanup assertions.
The helper now permits 120 seconds for bounded preparation, reports journal stage
and intent state on timeout, and captures a prematurely exited command's output.
The same partial-window and post-kill source assertions remain required. This
retry does not establish the original timeout's precise cause or resolve the
separately preserved first-prompt startup stall. All-target Clippy for
core/snapshot/server/App/CLI passes with warnings denied. Formatting, whitespace,
strict OpenSpec (100 items) and lint (zero errors/25 warnings) pass. The complete
CLI suite was not repeated for this typed-payload extension.

JSONL last-message publication continuation (2026-11-09): exclusively created
same-directory temporary files replace the predictable `.name.tmp` path, with
synchronized bytes and cleanup after failed publication. Destination symlinks
are replaced without writing their targets. Concurrent writers publish one
complete answer. Publication failure preserves per-input results and produces
aggregate exit 1 plus an `end` error when the run would otherwise succeed;
interruption/protocol exit precedence is unchanged. Three unit and all eight
real JSONL regressions pass; CLI all-target Clippy passes with warnings denied.
Strict OpenSpec still passes all 100 items, lint zero errors/25 warnings.
The native workflow now selects these regressions and builds the Linux sandbox
helper after installing bubblewrap. External results for these edits are pending.
Task 6.3 remains open for hook events and bounded output ownership/backpressure;
6.2/6.4 and all parent acceptance obligations remain open.

Pending-input ledger continuation (2026-11-09): the JSONL driver now reserves an
exact message-ID slot before HTTP admission, retains at most 32 pending inputs,
and releases each terminal payload after emission. Promoted answers use stable
message IDs instead of indices into completed history. Refused admissions retire
their reserved slot; bounded aggregate counters retain input totals and failure
after payload retirement. Premature termination emits every pending result even
when an input already has an error. Two unit tests pass, including 4,096 payload
retirements and capacity/reused-slot identity checks. All nine real JSONL tests
pass, including 65 sequential correlated prompts and exact final input count.
Admission ordinals preserve pending terminal result order and the first failure
independently of completion order. The reporter also releases emitted completed
answer/tool/denial payloads while preserving billing and aggregate status. Owned
background tracking retains only its exact fork's handback-admitted bit rather
than every observed prompt ID; unrelated/foreign Session admissions cannot set
that bit. Thirty-one exec unit, 17 real delegation, five forked-command and nine
JSONL tests pass. CLI all-target Clippy passes with warnings denied; strict
OpenSpec passes 100 items and lint reports zero errors/25 warnings. Focused review
found the pending-output ordering regression and corrected it before acceptance
of this change. Request/interrupt ownership, bounded background Job lookup,
bounded output/backpressure, hook events and remaining 6.4 acceptance still remain
required. Tasks 6.2–6.4 stay open. Native CI run 42693044871 is on the earlier
9ac06c40 checkpoint; it is not native acceptance of this ledger extension.

Structured terminal-result continuation (2026-11-09): new runtime regressions
first reproduced absent input outcomes after interruption and budget exhaustion
during correction (five existing tests passed, two regressions failed). Drain
exit now records the matching open root input's outcome before releasing its
entry or publishing idle. It preserves a durable valid finalizer, records the
current submission count on error, and leaves shutdown attempts open for explicit
restart resume. Metadata settlement failures remain execution failures. Pass
execution/error classification is extracted to keep the existing cognitive
complexity limit. Four added cases prove interrupted/budget outcomes and lookup
after restart, schema/attempt preservation through actual overflow compaction,
and shutdown followed by explicit resume without resetting the result attempt.

Final local checks: 94 runtime regressions pass (13 budget, 19 context/compaction,
53 runtime and nine structured-root); three structured-output CLI and nine JSONL
tests pass. Server all-target Clippy passes with warnings denied, as do format,
whitespace and strict OpenSpec (100 items; lint zero errors/25 warnings). An
earlier Clippy run ran out of disk and the broader test command could not create
its log; neither is a passing attempt. Cargo package cleanup removed 23.2 GiB of
rebuildable server artifacts, and the checks above passed after cleanup with
incremental compilation disabled. Finalizer/terminal-outcome writer/restart cuts
and real CLI budget/SIGINT precedence remain open under 5.4. No new public receipt
or API/SDK field was added by this correction.

Native run 42693044871 passed all selected native, managed Git App/CLI and JSONL
steps on Linux x64 and ARM for 9ac06c40. macOS Intel/ARM jobs remain queued at this
audit; they have no passing result. The ledger and structured terminal-result
extensions need their own current-source native acceptance before delivery.

Structured terminal-write recovery continuation (2026-11-09): a new fault fixture
first reproduced two extra provider requests when a validated finalizer was
durable but its terminal append had failed. Start-of-pass recovery now settles
that valid result or the exhausted correction error before model dispatch; new
queued input remains eligible after reconciliation. Three runtime cases prove
valid/exhausted recovery without another request, preservation of later plain
prompt behavior, and old-result settlement before promoting a queued different
schema. The initial exhausted fixture repeated identical invalid calls and hit
the existing doom-loop approval (confirmed by its persisted permission event),
then timed out. Distinct invalid submissions now reach the intended boundary;
the repeated-tool safeguard is unchanged.

Real CLI cases now gate a correction at the provider and verify max-token and
wall-time termination retain exit 4, while Unix SIGINT retains 130 despite the
structured terminal error. The direct unsafe signal call was rejected by the
configured unsafe-code lint and replaced with the existing owned-child `kill
-INT` fixture pattern. Fixture helpers are split by preparation, reconciliation
and new-input checks to satisfy the cognitive complexity limit. These are
resolved intermediate failures, not passing attempts.

Final checks: 111 regressions pass in one server/CLI feature-unified run (97
runtime cases and 14 real CLI cases); combined server/CLI all-target Clippy passes
with warnings denied, as do formatting, whitespace and strict OpenSpec (100
items; lint zero errors/25 warnings). The native workflow selects all exec units
plus the structured-root/CLI-stop cases on its four hosts and retains their log.
This edit is not current-source external acceptance: run 42693044871 remains on
9ac06c40 (Linux x64/ARM passed; macOS jobs queued at the preceding audit). No new
API/SDK field or CLI flag was added. SDK/plugin/full-workspace checks and Windows
execution were not rerun here; the SIGINT case is explicitly Unix-only.

Structured steering and committed-reply continuation (2026-11-09): two new
runtime regressions first failed because a steered promotion discarded the prior
input's open result. Promotion now freezes displaced outcomes and commits each
immediately before its replacement in the same append. A valid durable finalizer
wins; otherwise each displaced structured input gets an explicit superseded
error with its own attempt count, including zero-attempt intermediate schemas.
Child result behavior and unstructured input semantics are unchanged. A shared
pure result constructor also preserves the same terminal rules at Drain exit.

Runtime cases prove multiple steered schemas and replacement by plain input.
Two real JSONL cases gate inference, admit steering concurrently over stdin, and
verify exact per-input outcomes plus aggregate failure for a superseded schema.
Two SQLite WAL-hook cases lose the acknowledgement only after the exact target
terminal commit is visible through an independent read-only connection. They
prove the writer fences, cached terminal state does not advance, and reopening
the physical Store recovers the original result or committed new input without
replaying old inference/tools or duplicating promotion. The native workflow now
watches the fault helper. Manual proof dispatches queue behind an active matrix
rather than cancelling it; obsolete pull-request heads still cancel normally.

Checks: 115 broad runtime/CLI regressions pass, followed by both added WAL-ack
cases. The final targeted run passes all 16 structured-root, five output-schema
CLI and 11 JSONL tests; combined server/CLI all-target Clippy passes with warnings
denied. Formatting, whitespace and strict OpenSpec pass (100 items; lint zero
errors/25 warnings). Focused code/security review confirms state stays unchanged
until the atomic promotion append succeeds and no request/reply or interruption
authority is inferred from these result records. No new API/SDK fields were
added. Current-source native/platform acceptance and global SDK/plugin/full
workspace checks remain open. This does not complete tasks 6.2–6.4 or M2–M8.


Cancelled fully prepared Git payload continuation (2026-11-09): cancellation
cleanup now accepts fully persisted Git target images and typed symlinks through
the same version-3 intent-bound plan, positive non-admitted phase, complete checkout
set and exact already-published acknowledgement checks as other cleanup classes.
No source replay, receipt field, plan format or default Git admission is added.
Incomplete construction without persisted identities, CAS/metadata collection and
the remaining native/cut/client gates remain open with M6 and its parent tasks.

Both real CLI prepared-cancellation cases pass on local macOS ARM (235.48 seconds):
file-copy and managed Git. The managed case kills the real service after prepared
identities are published while SQLite writer contention prevents source admission.
It verifies non-empty persisted checkout digest evidence, missing-marker and live
claim refusal before a plan, equal-target symlink and regular-file replacement
refusal, dangling private link ownership, immutable Store intent binding, unknown
entry refusal, resumed cleanup after a durable plan and synthetic planned unlink,
retained cancellation lookup after restart, unchanged owner files/transcript/Git
index/configuration and no cleanup inference. The Git fixture uses smaller images
under the unchanged handoff limit; its first oversized attempt was rejected before
cleanup, and an empty-resource assertion inherited from the file-copy fixture was
corrected to verify the actual persisted bindings. Neither failed attempt counts
as acceptance. The final helper refactor and persisted-set checks are in the passing
binary. Snapshot library: 140 passed, six intentionally ignored subprocess fixtures.
All-target snapshot/CLI Clippy with warnings denied, formatting/whitespace and
strict OpenSpec pass (100 items; lint zero errors/25 warnings). The compiler still
reports macOS sandbox deprecation and linker unwind-table warnings. Focused code
and security review found no additional issue in this extension; this is not a
review of the full recovery branch. SDK/generated-artifact, default CLI, full
workspace and current-source external platform acceptance were not rerun here.
The native workflow now selects both managed Git process-death cases serially;
the workflow edit is not passing platform evidence.

Native run 42716226637 at 03b4526a completed successfully on Linux x64/ARM and
macOS ARM/Intel. It predates steering and this cancellation extension. Run
42728260672 at 1be03642 covers steering and post-commit acknowledgement loss;
its Linux x64 job has passed, with the remaining jobs pending at this checkpoint.
Neither run accepts the new cancelled-Git cleanup source. Queue its own native
proof and retain each run's actual tested source identity.


Fallback collection retention continuation (2026-11-09): six new regressions
initially fail because disabled capture still prunes manifests, zero retention
still prunes temporary/orphan bytes, orphan aging is fixed at seven days, corrupt
or missing Location evidence is discovered after deletion, and policy revocation
is not rechecked. Collection now preflights the complete namespace-bound manifest
inventory and known regular object/temporary entries before manifest unlinks,
retains disabled/zero-retention bytes, applies configured retention to orphan and
temporary aging, and refuses a changed configuration before each unlink. Foreign
Location manifests, temporary aliases and unknown object entries refuse before
any manifest deletion; already empty namespaces are no-ops. The native collector
is isolated in copy/collection.rs and still uses the existing serialized Store
reference callback. No native worker/journal fence is relaxed and no API, SDK or
receipt schema changes. Missing surviving Location evidence protects orphan bytes;
durable collection/retirement evidence, complete metadata/CAS reclamation and
partial construction remain required by M6 rather than being inferred from names.

Final local verification: 28 fallback integration tests and 16 App reclamation
cases pass, including a real Store-writer disabled/zero-policy/reenabled-collection
case and existing reference-writer exclusion, cap-pressure, unknown-owner, native
process-death, restart and receipt regressions. The additional empty-namespace
unit and quoted unknown-entry diagnostic unit pass. A focused App unknown-payload
rerun passes after the diagnostic change, and the new CLI panic-preservation probe
passes. Earlier in this turn, before the final preflight refinements, the complete
snapshot suite passed 188 tests with seven intentional subprocess/diagnostic skips;
that earlier run is not claimed as final-source full-suite acceptance. Final
all-target snapshot/App/CLI Clippy and formatting/whitespace pass. Strict OpenSpec
passes 100 items; spec lint reports zero errors and 25 warnings. Focused code and
security review found no additional issue in these changes after the preflight
fixes; the macOS construction failure and complete branch review remain open. Native platform, SDK/generated
artifact and full-workspace delivery acceptance remain open.

Source-bound native failure: run 42728260672 at 1be03642 passes Linux x64/ARM,
but macOS ARM fails the managed Git process-death recovery case at cleanup-plan
preparation with "Unknown recovery payload or construction entry". Its adapter,
core marker and App Git acceptance steps passed; JSONL and structured steps were
skipped after the failure. The log does not identify the entry, so its type and
cause are not established and the fence must not be bypassed. Paired cleanup now
quotes the first bounded unexpected leaf in its refusal, with a unit case proving
newlines are escaped and bytes remain intact. CLI fixtures now retain their own
private roots on panic after service stop, with a catch-unwind regression; native
CI uploads those failed scripted-fixture directories including hidden Git metadata.
Independent JSONL/structured checks run unless cancelled even after an earlier
recovery failure, without relaxing the overall job gate. The native workflow also
selects the complete App reclamation suite. These edits provide diagnostics and
coverage, not a fix or passing acceptance for the macOS construction failure.

Owned intent-update continuation (2026-11-10): runtime-owned prepared journals
now publish version 7 with an immutable binding to two preallocated native intent
inodes and the original copy/Git image format. Progress, completion and native
acknowledgement overwrite/sync the spare, exchange the two names and sync the
directory; they allocate no anonymous source-phase temporary intent. Reopen and
updates reject missing, replaced, aliased or invalid bindings. Ordinary, paired
and prepared-cancellation payload cleanup retain both slots, recognize the spare
only through its binding and recheck both metadata generations before unlinks.
Legacy digest framing and unknown construction fences remain unchanged.

Local evidence: five intent-slot cases pass, including an actual subprocess kill
during a torn spare write and physical Store reopen; its ignored child is invoked
by the parent proof. The complete snapshot candidate run passes 197 cases with
eight intentional subprocess/diagnostic skips, before the final malformed-version
writer guard; the slot cases pass again with that guard. Eighteen App reclamation
and two App Git runtime cases pass with both metadata pins. Three real managed Git
CLI cases pass before the extra spare-generation pin; the full final serial CLI
run is pending. Four non-Git CLI cases pass after correcting the old version-3
assertions. The broad four-thread CLI run records six failures: those three
version assertions, two pre-origin observation deadlines and one typed storage-busy
TUI acknowledgement. The three preparation/TUI cases pass serially in 124.39s;
the parallel failure causes remain unestablished, and no safety fence or deadline
was relaxed. Six structured CLI cases pass, including a separate timeout with a
verified in-flight correction; short deadlines may legitimately expire before
main inference. All-target snapshot/App/CLI Clippy passes after extracting the
copy-intent header assertions. Focused code/security review finds no additional
issue in this increment; full-branch review and delivery acceptance remain open.

Native run 42733865471 passes all four Linux/macOS jobs at e2b3ba18. It predates
these changes and accepts only that checkpoint. The workflow now selects the
preparation/TUI client cases serially in addition to managed Git recovery; fresh
source-bound platform evidence remains required. Partial construction, complete
reference/CAS/metadata retirement, default Git admission and M2–M8/parent task
acceptance remain open. No public API or SDK schema changes in this increment.

Final intent-slot CLI evidence: all 22 serial branch/recovery cases pass at
2ec1b1c5 in 837.69s, including both managed Git process-death cases and terminal
lost-reply reconciliation. SDK generation consistency, typecheck and 136 tests
pass at that checkpoint, with ten configured external skips. Native run
42753722365 passes both Linux jobs and macOS ARM; Intel remains running at this
checkpoint. These results do not accept later source changes.

Collection-provenance continuation (2026-11-10): before any unlink, fallback
collection now publishes/synchronizes an immutable bounded Location/root/native-
namespace record derived from complete valid manifests and pins its exact native
identity/generation through deletion. Restart after the last manifest disappears
can establish fresh retention without guessing from orphan names. Missing legacy
provenance, corrupt/foreign records, aliases, changed metadata, unavailable
configuration and disabled/zero retention protect remaining bytes. Every protected
copy reference must resolve to a valid manifest in the global private inventory
before collection; a dangling root cannot silently become an empty object set.
Valid other-Location roots and unrelated Git IDs retain compatibility. Namespace
metadata and unknown unpublished construction files count toward quota; none gain
cleanup authority from their names. Journal/worker fences remain unchanged.

Local verification: the snapshot candidate suite passes 205 cases with nine
intentional subprocess/diagnostic skips before the final unavailable-policy and
Git-reference compatibility refinements. Final targeted runs cover all 31 fallback
cases and seven collection unit cases, including the actual process-kill/physical-
reopen proof and its invoked ignored child. Eighteen App reclamation cases pass,
including serialized reference-writer exclusion. The final refinements are confined
to collection and covered by its targeted runs. Focused code/security review caught
the missing-reference and null-configuration cases; both now have regression
coverage. Full metadata/CAS retirement, partial construction and complete M2–M8
acceptance remain open. Public API/SDK schemas are unchanged; full workspace and
source-bound native/platform acceptance remain required.

Final collection-provenance checks: all-target snapshot/App/CLI Clippy with warnings
denied, formatting and whitespace pass. Strict OpenSpec passes 100 items; spec lint
reports zero errors and 25 warnings. Native macOS sandbox deprecation and linker
unwind-table diagnostics were observed during builds. No new public/generated
contract changed; the earlier SDK evidence remains scoped to its tested checkpoint.

Native scheduling correction: run 42753722365 at 2ec1b1c5 finishes both Linux jobs
and macOS ARM successfully. Intel passes snapshot/core/App, managed Git, preparation/
TUI and JSONL checks, then reaches the 45-minute job limit while compiling final
structured-result binaries; its matrix result is cancelled, not accepted. The job
allowance is now 60 minutes without changing any test deadline/assertion. Collection
source 124c11b9 has its own run 42759174937 in progress and is not accepted from the
prior source's results.

Metadata-retirement writer continuation (2026-11-10): reclamation sequences 2/3
now retain a bounded semantic-role native identity/generation/content-hash plan
and its exact completion, separately from payload completion at sequence 1.
Owner/plan mismatch, duplicate roles or inode identities, supplied paths, incomplete
payloads and inconsistent bytes refuse. The App's additive maintenance port commits
the immutable plan before its guarded callback, serializes it against reference
writers, re-attests receipt/native fences/references, and returns zero without
calling an observed completed callback again. Legacy trait implementations default
to unavailable without invoking the callback. A migration marker makes older Store
catalogs read-only; physical Store reopen and historical reprojection retain exact
phase distinctions without conferring native I/O authority.

This is a writer prerequisite, not native retirement acceptance. No native adapter
calls the port yet; current cleanup still retains all journal/lifetime/phase
metadata. Implement native per-leaf planning/deletion/resumption and retired-receipt
lookup/reconciliation, then complete managed checkout/partial-construction and
crash/platform acceptance. M6 and all parent tasks remain open. HTTP/OpenAPI/SDK
schemas are unchanged; the Rust maintenance interface and internal event protocol
have the additive retirement state/port described above.

The real App unavailable-policy regression initially deletes 57,344 bytes under
null configuration. The common ordinary/paired/cancellation retention guard now
refuses unavailable policy, and an actual two-payload case loses configuration after
its first unlink, preserves the second and resumes only under restored policy.
Final local evidence: 27 App reclamation/writer cases and 14 migration cases pass,
with an additional exact-inventory re-attestation and retry-counter rerun after
review refinements. The metadata callback tests deliberately prove Store sequencing
without deleting native metadata. All-target Store/server/snapshot/App/CLI Clippy,
formatting/whitespace, OpenAPI artifact and SDK generation checks pass; strict
OpenSpec passes 100 items and spec lint reports zero errors/25 warnings. macOS
sandbox deprecation and linker diagnostics were observed. Full workspace, SDK
runtime suites, native metadata deletion and new-source platform checks remain open.

Review refinement: the metadata registry now bounds vector length and counts JSON
encoding into a 16 KiB sink before cloning/deserializing; scalar bounds also precede
typed encoding. Final oversized-owner/vector, duplicate-identity, missing/foreign
role and supplied-path cases pass, as does final server/App Clippy. This adds no
native deletion authority. Focused code/security review finds no further issue in
this writer increment; native producer/reconciliation and full branch review remain
required before delivery acceptance.

Collection source 124c11b9 completes native run 42759174937 successfully on Linux
x64/ARM and macOS ARM/Intel. This accepts that source, not the subsequent metadata
writer protocol. Run 42759434324 at 50e00372 retains the longer workflow allowance
and is queued at this checkpoint; it also predates these writer changes.

Native metadata-retirement continuation (2026-11-10): the Linux/macOS adapter now
captures semantic-role identities, generations, BLAKE3 hashes and exact byte totals
for acknowledged journals with empty complete checkout inventories. It retains the
Store namespace lock, Location mutation/reader lease and actual exited lifetime
locks, verifies current and retained routes, known-only journal inventory, pins and
policy before each unlink, and synchronizes exact leaf/directory removal. Existing
immutable plans resume missing leaves without reconstructing or replaying source.
Unknown entries, replacement inodes, aliases, live owners and changed policy refuse.
The Store writer remains held through the callback; completed receipts answer native
acknowledgement from retained Store history, while pending plans freeze rewrites.

Periodic maintenance discovers at most 32 payload-completed receipts from the Store
per stable cursor page, with 64 KiB per-receipt bounds and Location-scoped cap
fallback. Thus missing active intents do not hide interrupted cleanup, and deferred
managed candidates do not permanently starve later pages. Discovery is not native
authority. A successful metadata cap fallback permits one fresh capture attempt.

Local evidence: all 37 App reclamation cases pass, including real ordinary native
retirement, actual HTTP acknowledgement after removal, before/after callback cuts,
replacement/unknown/alias/live-lock refusal, policy loss after the first unlink,
stable discovery pagination, cap-triggered capture and actual subprocess SIGKILL
followed by physical Store reopen and production periodic resumption. Later source
symlinks/unreadable owner bytes, transcript and inference counts are preserved.
The child fixture is an inert top-level test unless explicitly invoked by its parent;
its process-death assertion actually executes in the parent case. Affected server,
snapshot and App all-target Clippy passes with warnings denied; native macOS C
sandbox deprecation diagnostics are observed without unchanged-main attribution.
Managed claim retirement, zero-payload/partial construction, old/deleted Location
maintenance, full receipt-kind cuts and current-source platform acceptance remain
required. M6 and parent tasks remain open; this is not issue or delivery completion.

Prior-source native run 42759434324 at 50e00372 finishes both Linux jobs and macOS
ARM successfully; Intel fails the managed Git prepared-cancellation test because
its preparation origin was not observed (branches_cli.rs:1912). The failed private
fixture is retained/uploaded at .tmphzzjxr; the fixture-preservation probe's earlier
caught panic is expected and passes. Independent preparation/TUI, JSONL and
structured checks pass on Intel. This is a real unresolved acceptance failure,
not a passing matrix or evidence that a longer job allowance corrected the case.
Run 42767559419 at 5be366b7 is queued at this audit and predates native retirement.

Final local checkpoint verification: native-git-acceptance snapshot package passes
153 library cases (8 subprocess fixtures intentionally ignored at top level), 31
file-copy cases and 22 Git cases (1 subprocess fixture ignored at top level); the
Windows-only and doc binaries select zero here. Server snapshot selection passes
53 cases. OpenAPI artifact, SDK generation consistency, formatting and whitespace
checks pass. CI-pinned OpenSpec 1.13.2 validates all 100 items; spec lint reports zero
errors/25 warnings. Code-review-skill review of the native descriptor/route checks,
semantic-only plan, writer-held callback, refusal/resumption and actual tests finds
no additional defect in the tested ordinary path. Full receipt-kind acceptance
has a test gap: native preparation/paired retirement needs direct cases, and managed
retirement remains unsupported. These are required before accepting M6/delivery.
Full workspace, SDK runtime, current-source CLI/TUI and cross-platform acceptance
were not rerun for this checkpoint. No PR, merge or parent completion is claimed.

Intel cancellation investigation (2026-11-10): downloaded the retained failed
fixture from native run 42759434324, artifact 13741826409. Its original archive
remains untouched. A separate read-only SQLite inspection finds no published
snapshot preparation origin; server logs show the preceding rewind preview took
40,253 ms. This narrows the failure to the pre-origin request path but does not
prove the final cause or acceptance. The confirmation client previously discarded
its output when the observation assertion failed; its stdout/stderr now remain
inside the retained fixture, with exit status and observed-origin timing in logs.
The existing 45-second origin, 120-second staging and native pre-effect assertions
are unchanged.

Raw graph publication was starting one hash-object process per path even for
identical immutable captured bytes. The new isolated real-Git regression initially
fails with 65 calls for two contents. An exact-byte borrowed map now writes each
distinct blob once per capture while preserving every path/mode/tree record and
Git hash verification. The cache never crosses captures or source observations.
The regression passes for SHA-1 and SHA-256, binary bytes, literal newline paths,
executable/regular/link entries, equal bytes across different modes, repeated
capture and changed content (2/4/7 cumulative writes). It does not replace source
identity/policy capture checks or per-placement handoff quota accounting.

Final local raw-publication verification: the real managed Git prepared-cancellation
case passes in 130.51 seconds with origin observation at 14.5746 seconds. All original
45/120-second deadlines and actual source-admission/cancellation/payload assertions
remain. The feature snapshot package passes 154 library, 31 file-copy and 22 Git
cases; nine library subprocess fixtures and one Git subprocess fixture are ignored
at top level. The new process-accounting child actually runs both formats through
its parent. Windows-only/doc binaries select zero on this host. Snapshot/CLI all-target Clippy passes with warnings denied
following a test-helper split (the initial helper exceeded 15); no lint allowance
was added. Formatting, whitespace, strict OpenSpec (100 items) and spec lint
(0 errors/25 warnings) pass. Native C sandbox deprecations and a linker unwind-size
warning were observed without assigning their origin. Code/security review finds
no additional issue in this exact-byte, operation-local optimization: no source
observation, path/mode placement, hash check or quota proof is removed. Native CI
path selection now also covers App snapshot support fixtures and nested Store
reclamation protocol files. Full branch review, workspace/SDK runtime, other
CLI/TUI scenarios and fresh source-bound platform acceptance remain required;
this does not complete the Intel acceptance failure, M6 or issue #93.

Native receipt-kind retirement continuation (2026-11-10): existing actual CLI
preparation and partial-source-kill keep/rollback fixtures now invoke metadata
retirement only after observed payload completion. Prepared cancellation retains
its immutable five-file plan; paired recovery retains eight semantic leaves for
two exact owners. Tests require journal removal, no-follow absence of each lifetime
and source-phase path, observed completion, zero-effect repeat retirement, and the
original receipt through restarted real CLI acknowledgement. Existing later-owner
symlink/bytes, transcript, model-request and source-result assertions remain.
The native matrix now selects these two full cases in independent steps even after
another step fails, retains separate logs and preserves failed fixture directories.

The new managed-preparation refusal assertion initially fails on the error text:
empty-set digest validation returned a generic incomplete-resource error before
the adapter's intended unsupported-managed diagnostic. Its local fixture remains
at .tmpskmwRM. The adapter now explicitly refuses managed ownership before that
empty-set calculation; complete receipt verification and every native/Store fence
remain mandatory. The regression requires the specific unavailable result, no
metadata plan, and retained intent/cancellation files. This grants no managed
retirement authority. Managed claims, zero-payload and partial construction,
old/deleted Location maintenance and complete crash/platform acceptance remain
required before M6 or its parent tasks can close.

Final receipt-kind checks pass: all three exact CLI cases run serially and pass
with final no-follow worker-file absence assertions and the explicit managed
refusal. The cases cover prepared cancellation, managed Git cancellation (retained
unsupported metadata) and both actual partial-kill keep/rollback actions. The
corrected run reports 220.47 seconds, including an early fixture pause to let the
previous run finish; that elapsed time is not a performance benchmark. Its native
origin observations occur after resumption under the original deadline. The prior
run's two positives and one diagnostic mismatch are retained as failures rather
than counted as a green suite. Snapshot/CLI all-target Clippy with warnings denied,
formatting/whitespace, strict OpenSpec (100 items), spec lint (0 errors/25 warnings)
and Ruby YAML parsing pass. Code-review-skill review finds no additional issue in
the typed receipt-based assertions or the refusal-only guard correction. No new
HTTP/generated contract changes. Full workspace, SDK runtime, other CLI/TUI cases,
all receipt-kind crash cuts and new-source platform acceptance remain unperformed
here and required by their parent tasks. No M6, issue, PR or delivery completion is
claimed. The preserved original pre-main-integration stash remains intact.

Managed retirement implementation continuation: extend the approved M6 protocol
with a frozen complete checkout inventory, exact native claim/acknowledgement
observations and persistent repository-lock identity. Retain all native locks
through the final metadata call; remove only verified acknowledged leaves under
the Store writer's durable plan. Resumption may observe missing exact leaves only
after that plan, and same-owner construction/foreign/aliased/live evidence refuses.
Shared creation and unrelated activity records remain retained. Implement the core
native guard, extended writer frame/catalog fence, actual snapshot adapter and
ordinary/preparation/paired managed positives plus refusal and process-cut cases.
This is ongoing M6 implementation, not acceptance or a new parent completion.

Managed checkout metadata retirement (2026-11-10): core now exposes a native guard
separate from its serializable observations. It derives fixed claim/acknowledgement
names, retains unique no-follow file locks and persistent repository locks, checks
native routes/creation bytes and every surviving generation before each unlink,
removes acknowledgement before original claim and preserves unrelated activity and
shared creation records. Same-owner unknown constructors refuse. Missing leaves
are progress only for the trusted adapter's already committed exact plan; ordinary
acquisition/acknowledgement is never synthesized during cleanup. Metadata opens
are nonblocking before regular-file validation, including a bounded FIFO regression.

The internal metadata event's version-2 frame retains complete ordered checkout
bindings with per-owner consumed resource/receipt digests and exact leaf metadata.
It bounds resources to 8,192, each native leaf to one MiB, aggregate checkout bytes
to 64 MiB and encoding to 32 MiB. Legacy version-1 frames stay empty-inventory and
16 KiB bounded. Shape/uniqueness and exact consumption are validated at the Store
writer; no data frame itself grants native effects. Migration marker
20261110010000_snapshot_checkout_retirement makes older catalogs read-only and
preserves existing evidence across physical reopen. Historical absolute-path
shape validation is portable and data-only; native acquisition separately requires
host-native absolute routes.

The actual snapshot adapter now prepares those guards from retained lifetimes,
complete resource publication and existing exact consuming acknowledgements,
commits the immutable Store plan, performs guarded claim and private metadata
retirement under the writer, and resumes from frozen bindings after phase/intent
removal. Repository locks remain through its last filesystem call. Shared
namespace/record entries are synchronized before planning, and exact parent
directories are also synchronized when resumption merely observes missing names.
Review tightened capture ordering: native identities/hashes/generations are frozen
before typed original context is re-attested, then fresh pins reject later changes.
Changed intent/admission and original-claim context regressions preserve bytes and
refuse before planning. The default owned-Git admission gate remains off.

Actual local positives cover managed ordinary restore in the default data layout,
managed prepared cancellation, and both keep/rollback after real partial source
replacement kills. They verify claim/acknowledgement disappearance, exact completion,
no-op repeats, protected index/config/creation bytes and receipt-only lookup/
acknowledgement after restarting. Later source edits/links, transcript and inference
counts remain unchanged. Native guard tests cover both per-leaf cuts through reopen,
multiple consumed owners sharing one repository lock, live/busy exclusion,
replacement/changed/aliased/context/constructor refusal, FIFO nonblocking and data
bounds. These primitive reopen cuts are not managed Store/process-death acceptance.
The prior unrelated probe panic is caught/expected; two test-helper failures were
retained as failures (.tmpvlw4ci and .tmptdEG3s), then corrected to avoid recreating
retired journals or killing an already stopped service. App/core failed fixture
paths are now retained for native CI alongside the existing CLI evidence.

Final local managed-retirement verification: 22 selected core worktree tests pass
(two existing subprocess fixtures ignored at top level), including nine new
retirement/portable-path cases; all 14 migration cases pass. Both real App Git
runtime cases and all 38 App reclamation cases pass. The final three managed CLI
cases pass in 292.72 seconds, including the caught fixture-preservation probe and
both keep/rollback source-kill actions. A later sample attempt finds the CLI process
already terminal; the verified filesystem wait is not treated as a timeout or
restarted. The snapshot feature package passes 154 library, 31 file-copy and 22 Git
cases (nine library/one Git subprocess fixtures intentionally ignored at top level;
Windows-only/doc binaries select zero here). All-target core/Store/server/snapshot/
App/CLI Clippy passes with warnings denied, as do format/whitespace, SDK generation
consistency, strict OpenSpec (100 items), spec lint (0 errors/25 warnings), and Ruby
workflow YAML parsing/guard-step selection. macOS C sandbox deprecations and linker
unwind-size diagnostics are observed without assigning their origin. Build-only
CLI artifacts were cleaned with cargo clean after verifying no CLI test/service
was live; original source/evidence and the pre-main-integration stash are preserved.

Code-review-skill summary for this checkpoint: architecture follows the existing
native core / runtime receipt / App Store-writer / snapshot adapter boundaries.
Native plans are data only; publication uses existing consumption markers and no
source replay or capability activation. Review identified and corrected capture
ordering and missing-parent-sync gaps; context/bounds/refusal tests pass. Critical:
0. Major: 1 required acceptance gap (managed metadata-deletion process/crash matrix
and current-source platforms). Minor/nits: 0. Verdict: REQUEST CHANGES before full
recovery delivery, while this scoped checkpoint is reviewable. Next: actual managed
metadata process cuts/retention-loss/reader cases, zero-payload and partial
construction, old/deleted Location maintenance and complete native acceptance.
Full workspace, SDK runtime and other CLI/TUI scenarios were not rerun here.
M6, parent tasks and issue #93 remain open; no PR, merge or delivery is claimed.

The final OpenAPI artifact consistency test also passes (one exact HTTP case).
No generated HTTP/SDK schema changes are required by this internal protocol.

Managed metadata process-death continuation (2026-11-10): an actual App creates a
managed checkout in default application data, captures/restores through the real
owned worker, observes payload completion and starts metadata retirement. A
configuration callback observes the real acknowledgement unlink, or the following
original-claim unlink, and parks before the next native deletion. The parent
verifies readiness, sends SIGKILL and reaps the child. No native source/deletion
hook or Store event is replaced. Failed control/child fixtures remain retained.
The original 30-second restore confirmation bound is unchanged; a separate new
90-second parent deadline covers the complete fixture setup and selected cut.

After each kill, a physically reopened Store shows version-2 metadata planning
without completion, the exact missing checkout leaves and the still-retained
intent. Production periodic maintenance resumes the frozen complete bindings,
finishes the same plan and returns zero on repeat. Protected Git index/config and
shared creation bytes, Session event sequence and a later source symlink to an
unreadable owner file stay unchanged. A fresh App returns the same receipt through
explicit acknowledgement after metadata is gone. These are ordinary managed
restore cuts at acknowledgement/claim removal, not the full managed preparation/
paired/constructor/removed-Location crash matrix. The child entry point is inert
in ordinary test discovery and actually executes twice through its parent.

The existing managed App fixture setup and tracked restore are shared with the
new subprocess case without changing its source/ownership assertions. Native CI
already runs the complete Git App binary; its path filter now explicitly includes
the new support file. This advances M6/M8 acceptance without closing either task.

Final local cut verification passes all four Git App entries in 48.73 seconds,
including the inert child and parent exercising two actual SIGKILLs. The focused
new parent also passes independently in 28.83 seconds. App all-target Clippy with
warnings denied passes after splitting cut-state assertions from resumption (the
initial combined helper measured 18/15; no allowance was added). Formatting,
whitespace, strict OpenSpec (100 items) and spec lint (0 errors/25 warnings) pass.
Code-review-skill review checks the exact observed cut, unconditional child
kill/reap, retained failure roots, physical Store reopen, immutable plan equality,
production maintenance resumption and data-only acknowledgement assertions; no
additional issue is found in this test continuation. Native C sandbox and linker
unwind diagnostics were observed without assigning their origin. No production
behavior, wire/SDK contract, default Git admission or parent completion changes.
Full workspace/SDK runtime and fresh-source cross-platform proof were not performed
locally; the native matrix must execute this source before accepting M8.
