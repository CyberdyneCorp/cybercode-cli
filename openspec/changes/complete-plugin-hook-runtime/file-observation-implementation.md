# File observation implementation notes

Status: **in progress; task 5.2 remains open**. The focused suite at `1e31e2f6`
passed Linux, macOS and Windows. The newer pinned alias, physical input-scope
and traversal-descriptor integration, including nested-directory move guards,
has 54 passing local macOS component tests and awaits full application/platform
CI. Remaining acceptance, broader workspace verification and substantive review are still required.

## Ownership and native observation

`cyber-tools/src/file_changes/owner.rs` starts a host-owned observer from
`open_location`/`refresh_location`. One actor is shared by the canonical Location,
with at most 32 Locations and 256 member Sessions per Location. Registry locks
never span scans, journal writes, callbacks or task joins. Idle actors check for
eligible handlers before acquiring native observation resources. Shutdown cancels
and joins workers outside registry locks; already launched callbacks receive
cooperative cancellation and settle through the existing receipt machinery.

Metadata traversal now also receives owner cancellation through a shared atomic
flag. Its async adapter retains and joins the blocking worker, which stops at
the next traversal boundary after any synchronous operation already in flight.
A cancelled result is rejected rather than used for a baseline or delta. The
source regression failed with the previous non-cancellable adapter and passes
with this change; full owner/platform verification of this increment is pending.

`native.rs` uses notify 8.2.0 with a bounded queue of 32 hints. Each hint retains
at most 128 relative paths of at most 4,096 bytes; coalescing retains at most
1,024 names. Loss, invalid names, oversized payloads and backend rescan flags
request reconciliation. Native registrations are non-recursive, supplied by the
safe scanner and capped at 256 directories. Missing coverage is reported rather
than interpreted as complete evidence. Access-only events are ignored.

A persistent registration/traversal limit now requests immediate reconciliation
only when entering partial coverage. It then relies on periodic reconciliation;
real callback queue/backend loss independently requests a rescan. The previous
adapter reasserted loss on every limited scan and drove a scan at every actor
tick. A source regression reproduced that loop and passes with the transition
tracking; full platform verification of this correction is pending.

Native notifications may be coalesced or lost and have editor-dependent shapes.
A three-second reconciliation supplements notifications and successful mutation
hints. Neither notification nor metadata establishes an exact write order or
writer identity. See the primary API documentation for
[notify](https://docs.rs/notify/8.2.0/notify/) and its
[rescan flag](https://docs.rs/notify/8.2.0/notify/struct.Event.html#method.need_rescan).

## Pinned roots, traversal and private state

`root.rs` retains a pinned directory and filesystem identity, checking that its
path still names that incarnation before committing or delivering observations.
A replacement claims a fresh generation and baseline. Loaded Sessions at the
recorded directory have their project-envelope caches invalidated; an epoch
prevents an earlier resolution from restoring a stale cache. On Windows the
read-only directory handle permits read/write/delete sharing so observation does
not prevent ordinary root renames.

`scan.rs` traverses metadata through pinned cap-std directories and
`open_dir_nofollow`. It opens no file contents, follows neither directory nor file
symlinks and bounds entries to 10,000, depth to 64 and paths to 4,096 bytes.
Unreadable or limited traversal is incomplete evidence. Nested traversal retains
the actual child basename separately from the normalized observation name.

Exclusions are checked before child metadata is read. They combine existing
protected-path rules, `.cyber`, database/WAL/SHM files, tool output, temporary
storage and global configuration. The real App explicitly supplies its owner
data/configuration/state/cache/temporary roots before opening Locations;
canonical aliases of those roots are also excluded. Inferring a storage root
from a tool-output parent could incorrectly exclude an embedding checkout.

`links.rs` groups permitted regular entries by filesystem identity and reported
link count. Fully accounted in-Location hard links remain observable. Entries
with unaccounted, excluded or inconsistent aliases are suppressed; the observer
does not enumerate outside or private trees to manufacture a proof. Every
bounded traversal finalizes link checks. Suppressed names retain old stamps
without synthetic deletion while complete evidence for other names can still
report real deletion. Coverage notices expose this uncertainty. Windows stamps
also retain the identity from cap-fs-ext metadata.

## Durable state and dispatch

`cyber-core::hooks::file_observations` defines bounded checkpoints and deltas:
10,000 retained files, 256 updates per batch, 4 MiB per record and at most 128
deltas between checkpoints. Metadata, not file contents, enters these records.
`Runtime::claim_file_watch` claims an exclusive generation with exact-sequence
CAS; a stale actor cannot advance the acknowledged baseline. The single writer
validates Location, generation, checkpoint preservation and allowance accounting.
An append failure leaves the actor's acknowledged state unchanged.

`state.rs` combines hints and metadata reconciliation. Incomplete scans preserve
unseen entries under bounds; deletes precede creates in replacement batches.
Protection revocation forgets old stamps silently instead of disclosing newly
protected names as deletions. Replay reconstructs only metadata and credit,
never pending hook effects. Incremental replay checks byte bounds at each fold
and poisons a failed candidate rather than returning partial state.

`FileWatch::dispatch_to_cancellable` constructs observational events for a real,
non-archived Session at the recorded Location, outside Session/lifecycle locks.
The private hook-admission binding is checked by the single writer against the
actual source observation, path, eligibility and current generation. Public
receipts omit the binding. Receipt IDs are stable for the source observation,
Session, path and effective handler identity; source entropy distinguishes
observations created in the same millisecond. Duplicate and Unknown receipts
remain consumed across restart. Post-effect hook decisions cannot veto a write
that already occurred, and observations grant no permission or writer identity.

## Feedback and explicit mutation hints

Each Location has a durable allowance of 64 changed paths before handler
execution pauses visibly. Quiet time, background writes and restart do not
restore credit. Reconciliation and bounded recording continue while paused.

A renewal is a separate empty delta. Only a newly committed, hook-checked user
prompt is forwarded to the host, after releasing the Session lock. Failed or
denied admission, exact retries and non-user input do not forward it. AppHost
forwards the notification to BuiltinHost. The actor retains only the newest
acknowledgement per admitted Session, under its existing membership bound. The
single writer verifies the real user source, current and historical Location,
non-archived ownership and single-use event ID before restoring credit. Notices
explain pause and resumption; hook/plugin input cannot create renewal authority.

Successful built-in write/edit/notebook/patch feedback contributes bounded hints
**after formatter settlement**, including when language services are disabled.
Native observation supplies shell, background and external effects. These hints
request metadata reconciliation; they make no authenticated tool-writer claim.

## Verification evidence

The [focused workflow](../../../.github/workflows/file-observation-regression.yml)
at [`f0c5a743`](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40606857160)
passed on Linux, macOS and Windows. It executes:

- Ten core observation-record tests and the native/scanner/reconciliation
  component tests (32 on Unix, 27 on Windows).
- Nine actual Store/Runtime regressions for generation fencing, failed append,
  checkpoint/replay bounds, forged scope/credit/metadata, user renewal and
  source/admission provenance.
- Seven HTTP/scripted-runtime scenarios covering real scoped dispatch,
  deduplication and ignored observational deny; automatic nested external edits,
  rename/removal and shared Sessions; quiet-time feedback and user renewal;
  root replacement with distinct real Git project identities; built-in
  write/edit effects without language services; scoped hard links; and held
  callback shutdown followed by explicit Session resume without old-effect replay.
- Both real AppHost forwarding checks, including nested owner-storage privacy
  and durable renewal through an actually committed held user prompt.

The broader workspace workflow at that revision is separate and was still
queued when last checked. An earlier broad run caught a missing AppHost delegate
for the new notification, which was corrected before this focused success;
lower-level observer tests alone did not verify the App facade.

At `413d10ad`, three new application tests exercise an actual formatter process,
foreground and background shell tools, and a hook that writes follow-up files
until durable feedback exhaustion. The background command waits for a fixture
gate until the model Turn is idle, then writes. Delayed changes after quiet time
must still be recorded without restarting the hook loop. The workflow builds
the production native process-ownership helper first. Windows uses PowerShell;
Unix uses its shell. These tests explicitly select full access to verify
observation/process lifecycle and are **not plugin-confinement evidence**.
At `da891bc2`, Linux passed the complete focused job, and macOS passed all
ten HTTP/scripted-runtime scenarios while its AppHost step was still running
in the [expanded workflow](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40611521520).
Windows passed nine of ten scenarios, including the real formatter and
self-mutating hook, but the shell fixture attempted member calls/control flow
non-interactively. Those expressions deliberately require manual confirmation
and are refused outright in bypass mode. The fixture enables interaction and default Mode on Windows and replies
Once only after matching each exact command through the real permission API;
it also uses absolute fixture paths and reports tool output on missing effects.
That correction and the remaining macOS application-facade step remain unverified. These results precede
the scanner cancellation and 32-Location acceptance additions described here.

A temporary local Cargo harness loads the actual core/native/root/scanner/state
sources by path and passes 46 tests plus Clippy with warnings denied and the
project complexity threshold of 15. It does not compile the complete owner,
Store/Runtime, AppHost or application tests. Formatting and whitespace checks
pass; strict OpenSpec validation passes all 106 items, and spec lint reports
zero errors and 25 warnings. Those validation scopes do not establish full
workspace or feature completion.

A debug-only model probe with 5,000 files and 128 deltas measured 2.065 seconds
for clone-and-validate-per-delta replay and 77.821 milliseconds for incremental
replay. This measures the model algorithm, not SQLite/application latency.

## Remaining acceptance and implementation

1. **Alias-aware Session acceptance:** the native lease, canonical journal key,
   Session scope and user-renewal integration below is implemented but awaits
   application/platform acceptance. Verify shared aliases, retargeting refusal,
   moved/archived Session eligibility and project-cache invalidation before
   closing this item.
2. **Resource/overflow/lifecycle acceptance:** exercise global Location/Session
   limits and visible incomplete coverage through the actual application, native
   queue loss/registration limits and bounded recovery, inaccessible/large trees,
   membership transitions and shutdown during reconciliation. Component tests
   cover several bounds but do not prove their full owner integration.
   A new real-runtime regression opens 33 distinct Location Sessions, requires
   visible refusal at the 32-Location watch limit, verifies another Location
   still delivers its edit, and joins all admitted owners at shutdown. Its
   application/platform results are pending. A second real-runtime regression
   exceeds the 256-directory native registration cap, requires a visible partial
   coverage notice and observes a later edit in the registration tail through
   ongoing reconciliation. Its platform results are also pending.
3. **Native-effect verification:** obtain Linux/macOS/Windows results for the
   three tests introduced at `413d10ad` and correct any failures.
4. **Delivery checks:** complete relevant broader workspace checks and a
   substantive code/security review, update evidence and keep task 5.2 open until
   all approved requirements are verified. This producer work does not close the
   separate plugin-enforcement/protocol support gates or the overall goal.

## Session-scope provenance increment

`file.watch.scope.1` now records one current scope per Session in a separate
aggregate, with expected-sequence append and writer checks for live Session
directory, watch generation/root and aggregate identity. Exact-directory dispatch
prepares this provenance idempotently before starting callbacks. Scope records
advance neither metadata CAS nor feedback credit, and do not grant filesystem
access or alias authority. Three new actual Store/runtime regressions cover
idempotence/fencing, malformed scope bindings and absence of a cross-directory
grant. Their application/platform execution is pending; the new bounded core
record test passes in the 46-test local harness.

[The scope integration design](file-observation-scope-design.md) specifies the
remaining pinned native lease, canonical journal key, per-handler/renewal checks
and project-cache work. The exact-directory increment described in this
historical section preceded the native alias integration below; that integration still awaits acceptance.

At `04deb581`, the full focused Linux job passed, including both new resource-limit
application scenarios. Windows passed eleven of twelve HTTP scenarios; its
approval fixture timed out because enabling interaction while retaining bypass
Mode still refuses uncertain PowerShell expressions. The fixture now uses default
Mode on Windows and includes tool outputs if a request is missing. That correction
and the scope increment require a new platform run.

## Pinned native Session integration under verification

At `1e31e2f6`, the entire focused workflow passed on Linux, macOS and Windows,
including all twelve Store/runtime and twelve HTTP/scripted-runtime scenarios
and both real AppHost checks. That includes the corrected default-Mode Windows
shell fixture and the durable exact-directory scope ledger. The newer native
alias integration described below has not yet passed application/platform CI.

Production observers now claim the canonical journal with a clone of the pinned
root descriptor. The runtime keeps bounded weak generation registrations; the
FileWatch owns the descriptor and admitted handlers retain it through settlement.
Dropping the watch revokes new admission even while a handler retains its pin.
Native checkpoints explicitly require a live lease, including for canonical
paths; they cannot fall back to the legacy metadata-only port after owner loss.

Session aliases prepare durable scope records only after physical identity
checks. Handler admission and launch recheck native ownership; configuration,
path matching and command cwd use the canonical root without changing public
Session envelopes. Membership refresh and root generation startup prepare
bounded scope updates. A changed physical scope invalidates cached project
envelopes with the existing epoch fence. Native user renewal also checks the
private physical-root snapshot captured in its actual user admission, current
and historical raw Session scope, and global single-use consumption. A new
partial expression index supports that global check. Projectors do no FS IO.

The local actual-source harness passes 52 tests and Clippy at threshold 15. It
now includes native lease-registry tests but still excludes the complete runtime,
owner and application integration. New Store/runtime tests cover alias handler
admission, generation/spelling changes, exhausted-credit restart and real input,
retargeting an unused acknowledgement, dropped owners and retained-handler pins.
New real application tests cover alias-first sharing, raw envelopes, symlink
retarget refusal, fresh project identity and user-input renewal. These await CI;
Unix symlink tests are explicitly gated, while spelling tests run on Windows too.
The private event execution hint also has a core regression in platform CI.

A separate real-source regression reproduced 519 open descriptors when scanning
512 sibling directories. The scanner now retains only its open ancestor chain,
so descriptor use follows the depth bound rather than tree width. The isolated
child-process FD regression now passes with full tail coverage. This supplements
the production application registration-tail test; it does not close remaining
nested-move, overflow/resource/lifecycle acceptance or final review obligations.

The invalid UTF-8 root fixture is Linux-only: the local macOS filesystem rejects
the byte sequence before native observation can inspect it. Unix symlink/FD tests
run on macOS/Linux; Windows runs spelling and real application coverage without
those Unix-only fixtures. No skipped platform fixture is counted as executed.
The renewal-index migration and exact consumption query were checked against
5,000 in-memory SQL events; SQLite selected the new expression index. This is
SQL-plan evidence, not full Store/runtime migration acceptance.

## Directory-move guard and current CI correction

A regression reproduced a pinned child descriptor following a directory moved
outside the Location during traversal, including a scan stopped at its entry
limit. `scan/scopes.rs` now records bounded directory identities and revalidates
them through no-follow parent opens before publishing results. It retains only
the open ancestor chain. Missing or changed directory incarnations discard their
subtree metadata and mark the scan incomplete, preserving prior stamps rather
than inventing deletions. Cancellation discards the candidate scan.

A second regression verifies that a moved directory's files cannot count as
permitted hard-link aliases of files remaining inside the Location. Scope
filtering precedes link-proof finalization. Both regressions are Unix-only;
Windows execution remains dependent on the platform suite's existing fixtures.
The actual-source macOS harness now passes 54 tests and Clippy with threshold 15
and warnings denied, excluding the complete owner, runtime and application.

The focused run at `5af61f59` failed. Linux and macOS logs identify a borrowed
Session ID captured by a Store closure requiring a static lifetime in the new
application fixture. The fixture now moves an owned ID into that closure; this
is a compilation correction, not evidence that the application scenario passed.
The directory-move changes and this correction require fresh platform CI.

Broader CI at `5af61f59` also reported `items_after_test_module` in the new
core envelope test placement on Linux/macOS/Windows. The production impl now
precedes that test module; current-head workspace Clippy still requires CI.

## Session-capacity recovery acceptance pending

An additional actual-runtime scenario creates 257 Sessions at one Location,
checks the visible refusal at the 256-member bound and admits ordinary held
user input while coverage is unavailable. It releases covered memberships,
checks that the refused Session receives no effect before readmission, then
requires committed input to prepare its durable scope and verifies shared
observation delivery and joined shutdown. The test uses the production owner
and HTTP hook, not a substitute registry. Execution is pending platform CI;
formatting alone does not establish that this acceptance passes.
