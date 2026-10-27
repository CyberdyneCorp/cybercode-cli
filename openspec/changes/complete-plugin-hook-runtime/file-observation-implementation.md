# File observation implementation notes

Status: in progress; component and record-model checks passed locally; actual
store/runtime regressions await CI. The application does not yet emit
`FileChanged` from these components. Task 5.2 remains open, and this increment
cannot establish the plugin/hook workstream's acceptance.

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

Six actual Store/Runtime regressions cover restart fencing, append failure,
periodic checkpoints, forged scope/credit/metadata checkpoints and single-use
user renewal and historical Location binding. Their compilation and results must be confirmed in CI. The focused
workflow runs pure models, native components and Store/Runtime regressions on
Linux, macOS and Windows. No platform result is implied by adding that workflow.

## Remaining integration requirements

1. Bind one cancellable watcher generation to a canonical Location and actual
   runtime ownership. The native registration component now has a per-Location bound;
   the owner still needs a global Location/task bound and visible partial-coverage
   diagnostics with bounded reconciliation. Verify the root incarnation and fresh Session scope before
   recording/dispatching. Multiple Sessions must receive only their current
   Location's observations; archived/moved Sessions cannot borrow old watches.
2. The checkpoint/delta journal is implemented with registered owner events;
   confirm the actual Store regressions and connect it to watcher ownership. Advance in-memory state only after durable
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
   The component allowance has no automatic reset; the owner/UX path is pending.
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
the native producer integration remains pending. The journal is registered in
the production runtime, but has no application watcher caller yet. A component regression first
caught a missing `cap_fs_ext::DirExt` import and was corrected. Sixteen component
tests now cover traversal, native events, reconciliation, overflow, policy
revocation, scope names and finite feedback allowance on the local macOS host.

The expanded isolated harness passes 30 tests: the 20 native/reconciliation
component tests, nine pure observation-record tests and the existing ID helper
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
