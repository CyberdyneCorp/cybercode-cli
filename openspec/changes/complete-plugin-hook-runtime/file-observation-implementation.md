# File observation implementation notes

Status: in progress; component tests only. The application does not yet emit
`FileChanged` from these components. Task 5.2 remains open, and this increment
cannot establish the plugin/hook workstream's acceptance.

## Components under construction

- `cyber-tools/src/file_changes/native.rs`: native notify 8.2.0 hints, with a
  bounded synchronous queue, at most 128 relative paths per retained hint and
  4,096 bytes per path. Queue loss, oversized payloads and backend rescan flags
  request reconciliation. Access-only events do not masquerade as writes.
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

## Remaining integration requirements

1. Bind one cancellable watcher generation to a canonical Location and actual
   runtime ownership. Native recursive registration can still span a large tree;
   production must bound watch registrations or fail visibly and retain bounded
   reconciliation. Verify the root incarnation and fresh Session scope before
   recording/dispatching. Multiple Sessions must receive only their current
   Location's observations; archived/moved Sessions cannot borrow old watches.
2. Persist bounded metadata state, observation identities and watcher generations
   through registered owner events. Advance in-memory state only after durable
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
release integration is deliberately still pending. A component regression first
caught a missing `cap_fs_ext::DirExt` import and was corrected. Sixteen component
tests now cover traversal, native events, reconciliation, overflow, policy
revocation, scope names and finite feedback allowance on the local macOS host.
