# Owner save integration work

This implements the already approved binary save/recovery, snapshot and FileChanged
requirements. Source installation remains gated until the integrations and their
regressions are present. This document records implementation decisions, not completed
acceptance or an expansion of the model/plugin scope.

## Execution and file outcome are separate

A filesystem-only worker retains the native Location, workspace/path, lifecycle and
capacity owners until the actual blocking worker finishes. No subprocess or hook may
escape that worker. The worker's tagged SQL activity records execution ownership;
the private document journal records acknowledged/conflict/unknown file outcomes.

Explicit document recovery obtains the real exclusive private document lock. Its
borrowed proof is checked against the runtime's configured, pinned private root;
a same-named document under another root is insufficient. The proof may settle
only matching filesystem activity records with matching original provenance and
no live local control. Generic native processes retain their own acknowledgement
requirements. Recovery never exchanges files or manufactures a file receipt.
A still-unknown native document intent remains unknown after execution recovery.
Missing locks beside recovery data are not recreated; live stores recheck the lock's
native identity. The actual lock descriptor stays owned while validation probes the
binding ([Rust locking contract](https://doc.rust-lang.org/std/fs/struct.File.html#method.try_lock)).

## Capacity before native effects

Atomic exchange moves the old source into the private stage name. A shrinking save
or restore can increase private logical usage even though it allocates no source
payload blocks. Preflight reserves both record publication peaks, the displaced
file delta and the outcome record before publishing native intent. Record sizes
are counted by streaming serialization, including the actual acknowledgement
shape, without cloning the draft command history. Available filesystem space and
the 2 GiB logical document ceiling remain distinct checks. Failure retains the
draft and source unchanged. External filesystem failure after intent still requires
honest unknown-outcome handling and retained evidence.

## Remaining save activation work

- Refresh current read/edit/Mode/sandbox and captured ancestor authority at the
  native commit boundary after expensive preparation. No permission hook or model
  classifier may run while holding mutation ownership; a stricter final decision
  refuses and requires explicit readmission.
- Retain protected preimage and byte-range evidence for every acknowledged save,
  including non-git and ordinary-snapshot-excluded files. Shared version discovery
  exposes only acknowledged source versions, never another owner's unsaved commands.
  Restore retains exact current-identity/content checks and separate byte review.
- Durably queue shared version/diff and FileChanged work by document/operation
  identity after native acknowledgement. A durable terminal disposition prevents
  duplicate publication after lost responses. Unknown/failed operations produce
  no fabricated success event, and expired hook provenance is never replayed as a
  new user input. Integrate with the existing native observation producer.
- Add owner save/restore/reconciliation routes, SDK helpers and terminal states
  only after those invariants are tested. Model patch uses invocation-scoped
  drafts and the same guarded save path, with plugin filesystem intersections.
- Complete explicit retention/cleanup, native race/process-death tests, real
  terminal captures, performance/accessibility evidence and final delivery review.
