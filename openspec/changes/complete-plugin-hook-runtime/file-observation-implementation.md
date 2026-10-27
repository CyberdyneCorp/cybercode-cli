# File observation implementation notes

Status: in progress. The earlier journal/dispatch revision passed focused Linux
and Windows checks; macOS is queued. Automatic host-owned watching is now wired
and has new application regressions awaiting CI. Local isolated model/native/root
checks pass, but do not compile the host integration. Task 5.2 remains open.
## Components under construction

- `cyber-tools/src/file_changes/native.rs`: native notify 8.2.0 hints, with a
  bounded synchronous queue, at most 128 relative paths per retained hint and
  4,096 bytes per path. Queue loss, oversized payloads and backend rescan flags
  request reconciliation. Native registrations are non-recursive and capped at
  256 directories supplied by the no-follow scanner. Incomplete scans preserve
  existing coverage; limit/registration failures report partial coverage through
  the rescan flag. Deleted registrations release their slots. Access-only events
  do not masquerade as writes.
- `scan.rs`: metadata-only traversal through pinned cap-std directories and
  `open_dir_nofollow`, with entry, depth, path and cancellation bounds. It opens
  no file contents and follows neither directory nor file symlinks. Exclusion
  is supplied before reading child metadata. Unreadable/limited scans are
  incomplete evidence, not proof of file deletion.
- `state.rs`: native hints plus metadata reconciliation detect replacements,
  create/remove and renames as changed source/destination paths without claiming
  a writer identity. Overflow preserves prior state; incomplete scans retain
  unseen entries under a memory bound. Protection revocation forgets obsolete
  stamps without reporting the newly protected paths as deleted.

The native API documents editor-dependent event shapes and possible notification
loss. Reconciliation is required even when the watcher starts successfully; a
native hint is not proof of exact write ordering or actor identity.
Sources: [notify](https://docs.rs/notify/8.2.0/notify/),
[rescan flag](https://docs.rs/notify/8.2.0/notify/struct.Event.html#method.need_rescan).

## Durable journal increment

`cyber-core::hooks::file_observations` defines bounded checkpoints and deltas.
`Runtime::claim_file_watch` claims an exclusive generation through exact-sequence
append; a stale worker cannot advance acknowledged state. The registered single
writer projector checks Location identity, checkpoint preservation, monotonic
allowance accounting and bounded replay. An append failure leaves the caller's
metadata and allowance unchanged. Checkpoints occur at most every 128 deltas;
replay rebuilds metadata and never returns hook effects to execute again.

A renewal is a separate empty delta. The writer requires a committed user input
from a live Session at that Location, and rejects a repeated acknowledgement ID
across generations. Hook-originated input cannot satisfy that proof. The actual
user-action forwarding and alias-aware owner integration remain pending; this
port does not create human presence or authorize a callback itself.

The initial six actual Store/Runtime regressions cover restart fencing, append failure,
periodic checkpoints, forged scope/credit/metadata checkpoints and single-use
user renewal and historical Location binding. These initial six and the native components passed the focused Linux job at
`c7f73ff3`. The later `84eb1d3d` dispatch revision passed Linux/Windows; macOS and the newer automatic-owner revision remain pending. The focused
workflow runs pure models, native components and Store/Runtime regressions on
Linux, macOS and Windows. No platform result is implied by adding that workflow.

## Remaining integration requirements

1. Bind one cancellable watcher generation to a canonical Location and actual
   runtime ownership. The native registration component now has a per-Location bound;
   the new owner adds global Location/Session bounds and partial-coverage notices,
   which still need application acceptance. Verify the root incarnation and fresh Session scope before
   recording/dispatching. Multiple Sessions must receive only their current
   Location's observations; archived/moved Sessions cannot borrow old watches.
2. The checkpoint/delta journal is implemented with registered owner events;
   retain the passing Store regressions and verify the new watcher connection. Advance in-memory state only after durable
   acknowledgement. A restarted generation must fence the old writer and reconcile
   recorded state, while uncertain hook effects retain Unknown and are not replayed.
3. Compose exclusions from the existing protected-path rules and owner storage,
   including state nested below a Location and roots reached through aliases.
   File contents and provider credentials must not enter observation records.
4. Deliver bounded/coalesced events through the existing scoped hook executor.
   Preserve path matchers and stable observation IDs, ignore post-effect decisions
   and leave durable completed/failed/unknown receipts. No observation grants
   permission or asserts that an identified tool/user wrote the path.
5. Persist a finite feedback allowance: quiet periods and delayed/background
   writes cannot replenish it. Exhaustion must pause handler dispatch visibly
   while retaining bounded observations/reconciliation. Renewal requires a real
   owner-authorized user action; hook/plugin prompts cannot manufacture renewal.
   The owner now forwards newly committed user input and shows paused/resumed
   notices; application acceptance of this path is pending.
6. Reconcile after successful built-in and formatter changes and observe native
   shell/background/external changes. Handle error/unknown tool results without
   fabricating a successful writer receipt. Overflow needs a visible reconciliation
   state and a bounded recovery path, including large/inaccessible trees.
7. Join watcher/worker tasks during shutdown and settle launched hooks. Obtain
   native macOS/Linux and explicit Windows results, real-server regressions for
   multiple Sessions, feedback, restart, rename/delete and external edits, plus
   full relevant Rust checks before a reviewable production PR.

## Current checks

The source components were loaded by path into a small temporary Cargo harness
so local tests fit available disk. Its isolated tests and Clippy do not compile
or verify the full Cyber application. Workspace test registration is present;
the newer host producer requires full application verification. The journal is
registered in production and now has a host observer caller. A component regression first
caught a missing `cap_fs_ext::DirExt` import and was corrected. Sixteen component
tests now cover traversal, native events, reconciliation, overflow, policy
revocation, scope names and finite feedback allowance on the local macOS host.

The expanded isolated harness passes 31 tests: the 20 native/reconciliation
component tests, ten pure observation-record tests and the existing ID helper
regression loaded with them. Isolated Clippy with warnings denied and the
project cognitive-complexity threshold also passes. These do not establish a
full application build or acceptance of the actual Store/Runtime regressions.

A local debug-only model probe with 5,000 files and 128 deltas measured 2.065 s
for clone-and-validate-per-delta replay. The incremental private replay builder
measured 77.821 ms for that same workload. It enforces byte bounds at each fold
and poisons a failed candidate so partial state cannot escape. This sample
measures the model algorithm, not end-to-end SQLite or application latency.

Full workspace CI at `90a57da8` caught a direct `ulid` reference in the server
without that package dependency. Generation creation now lives in the existing
core module that owns ULID validation (`11d5d517`); the actual Store/Runtime and
full platform checks are rerunning. Isolated harness success is not evidence of
a whole-workspace compile.

## Committed observation dispatch port

`FileWatch::dispatch_to` builds observational FileChanged events for a real,
non-archived Session at the recorded Location and invokes the existing lifecycle
executor outside Session/lifecycle locks. Receipt admission carries a private
observation binding; the single writer verifies the actual source record, path,
eligibility and current generation before any new handler starts. The binding
is not part of public receipts or telemetry. Synthetic CLI hook tests keep their
existing isolated behavior.

Receipt identities are stable for the observation, Session, path and effective
handler identity. A duplicate delivery remains consumed even when its earlier
receipt is Unknown. Separate source IDs created in one millisecond remain
distinct. This adds three actual Store/admission regressions and one real
BuiltinHost/HTTP-hook flow for two Sessions, path matching, duplicate delivery,
ignored observational deny and receipt privacy. These tests await CI.

The new automatic owner described below awaits application checks. Alias-aware
Session admission, acceptance of user-input feedback renewal, explicit tool/formatter
reconciliation and full shutdown/restart/overflow acceptance remain required. No task is closed
on the strength of a manually dispatched observation.


## Host observer integration in progress

The host now registers the observer module in production and starts one shared
native observer per canonical Location on `open_location`/`refresh_location`.
The registry admits at most 32 Locations and 256 Sessions per Location; native
directory registrations remain capped at 256 each. Idle registrations inspect
whether file handlers exist before starting native observation. The existing
scoped hook executor remains responsible for resolving actual handler authority.

The owner scans through a pinned directory handle, checks that the path still
names that root before recording/delivery, and claims a new fenced baseline on
root replacement. Native hints are coalesced under a fixed queue-drain bound.
Periodic reconciliation supplements native hints; incomplete evidence cannot
prove deletion. Database/WAL/SHM paths, tool output, temporary storage, global
configuration and protected project paths are excluded. Symlink aliases of
configured private roots are excluded at their physical locations. Hard-link
aliases and full alias-aware Session admission still need acceptance and any
required follow-up implementation.

Metadata advances only through acknowledged journal deltas. Deletes precede
creates in large replacements so batches stay within the retained-file bound.
Dispatch uses the durable feedback allowance and shows a notice when paused;
quiet periods do not renew it. The newer committed-input forwarding path is described below and awaits application acceptance.
Host shutdown cancels and joins observers outside registry locks. File-change
dispatch now forwards owner cancellation into the existing runner while retaining
the launched future through cooperative receipt settlement.

Three additional application regressions exercise automatic external edits,
rename/removal and shared Sessions, shutdown, feedback exhaustion across a quiet
period, and replacing the root directory. The manual dispatch regression now
isolates itself from the automatic owner and checks pre-cancelled admission.
These application tests have been added but have not yet run for this revision.
The isolated source harness passes 35 tests and Clippy with warnings denied at
the project complexity threshold; it does not load the new host owner or compile
the full application. Formatting and whitespace checks pass.

At `84eb1d3d`, the earlier focused suite passed Linux and Windows, including all
nine Store/Runtime regressions and the manual HTTP-hook dispatch test. macOS
was still queued when these notes were updated. Those results do not verify
the newer host integration. Task 5.2 remains open pending the remaining behavior,
resource/lifecycle regressions, platform checks and substantive review.


## Committed user-input renewal

The runtime now forwards a newly committed, hook-checked user prompt to a narrow
synchronous host notification after releasing the Session lock. A failed/denied
admission, an exact retry and non-user input do not notify it. The host retains
only the latest acknowledgement per admitted member Session, under the existing
256-Session bound. The observer drains that bounded map and submits a separate
renewal delta; the single writer still verifies the real source event, current
and historical Location, source=user and single-use identity before restoring
credit. Hook callbacks cannot refresh the budget by sleeping or sending their
own source=hook prompts. Paused/resumed notices explain that new user input
resumes handlers. The automatic feedback test now exercises hook input, real
user input and exact retries as well as quiet time; that expanded application
test awaits CI.

The first automatic-owner workspace build at `7e479fed` failed on Linux and
Windows because an async delivery method retained `&Observation` across an
await; its native receiver is Send but not Sync. Delivery now retains exclusive
actor ownership (`&mut self`) rather than making the receiver shared or adding
an unsafe Sync implementation. SDK/dashboard generation also stopped at that
same Rust compile error. The corrected application build and tests must rerun;
no result from the isolated harness establishes this correction's acceptance.


## Nested scanner regression

A new actual-source regression reproduced failed nested traversal: validating a
normalized relative name had shadowed the entry's basename, so metadata lookup
inside a pinned child directory mistakenly repeated its parent components.
Validation now keeps a separate normalized name, and metadata/open-directory
operations retain the original basename. The regression failed before the fix;
the expanded isolated source harness now passes 36 tests and Clippy at threshold
15. The automatic external-edit application test now uses newly created nested
directories, so CI must also verify that this works through native observation
and HTTP-hook delivery. Application verification is still pending.


## Application compilation follow-up

At `0a6461a1`, Linux compiled the automatic owner and passed all 25 observer
component tests and all nine actual Store/Runtime regressions. The HTTP
application test binary then failed to compile because three count queries
requested `usize`, which rusqlite does not decode from SQLite. Those test
queries now use SQLite's supported `i64` count representation. The application
scenarios still require execution; compilation of the owner and narrow suite
success do not establish their behavior.
