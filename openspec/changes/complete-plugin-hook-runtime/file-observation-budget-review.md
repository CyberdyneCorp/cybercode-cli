# File observation budget increment review

Scope: budget increment based on `c0cc40e3`, reviewed with the
code-review skill. This is not approval of the full 5.2 feature or parent goal.

## Resolved major finding: encoded bounds can stop valid observation

The owner passed a count/path-bounded scan directly to a byte-bounded journal.
A real-source 4,800-file fixture produced 4,545,809 encoded bytes against the
4,194,304-byte record limit and failed initial checkpoint validation. This left
all observation unavailable at that root, including ordinary existing files.
The new publication budget preserves bounded partial coverage and old unknown
stamps instead. The native scope/alias finalization still runs before publication.

Review also checked retained-state merges and intermediate batches: reserving
unknown preimages avoids accidental deletion/overflow; full enumeration frees
confirmed absence; shrinking updates precede growth in admission and journal
order; encoded update batches retain the 256-update ceiling while fitting the
record byte limit. Protected retirement remains non-dispatching. Core validation
is unchanged, with its pure entry-size helper shared by replay and the adapter.
No filesystem I/O or host permission rule moves into the domain model.

Files reviewed: core entry helper; new budget module and all its regressions;
module wiring; owner startup/reconciliation/retirement/recording; Scan omission
semantics; actual-runtime long-inventory test; associated spec/tasks/evidence.
No additional actionable finding was identified in this increment review.

## Verification and limits

The isolated source harness passes 63 library tests and scoped Clippy with
threshold 15 and warnings denied, suppressing unused-code lint only for omitted
production consumers. The oversized fixture failed before its budget stage and
validates after it. Unit cases use actual core checkpoint/delta/replay validation
in addition to counter assertions. The real-runtime regression is configured for
Linux/macOS/Windows CI and remains unexecuted at this new revision.

Full local workspace tests/Clippy were not run with about 1.9 GiB free disk; the
harness excludes complete owner/runtime/AppHost integration. Current-head
application/platform and broader CI are required before readiness. Earlier
Linux/Windows acceptance is recorded at its exact code revision and does not
verify this increment. Parent task 6.3 and final feature acceptance remain open.
