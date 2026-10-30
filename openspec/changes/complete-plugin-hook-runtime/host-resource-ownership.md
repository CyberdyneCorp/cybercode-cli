# Cancellation-safe ownership of started hosts

The real production launch now returns an opaque `Started` owner. It holds the
Process, scratch and optional proxy together before initialization receives any
stdio endpoints. Callers cannot independently reap the raw Process and forget
the resource-retention state. Its tracked wait uses the existing backend's
tree-aware wait and records success or an unconfirmed error.

Dropping the owner on an early return or cancelled initialization requests known
process-group termination and proxy shutdown immediately, then transfers ownership
to a cleanup task. `finish()` awaits that same owned cleanup pattern; dropping its
caller future does not cancel the cleanup task. Scratch is released only after
the backend wait succeeds and proxy disposal is acknowledged. The proxy's new
idempotent `request_shutdown()` separates prompt revocation from later disposal
acknowledgement; ordinary `shutdown()` retains its previous behavior.
Initialization also holds a protocol cancellation guard until successful handoff;
abandoning it cancels the connection even if an in-flight handler holds an Arc.
An unconfirmed supervised cleanup records a failed host and stops the immediate
crash-restart loop rather than reporting a confirmed crash.

If a wait returns an error, the owner cannot safely signal a cached numeric group.
It preserves the Process identity and scratch rather than running Process Drop's
terminator or deleting the namespace. Runtime loss, task abortion and unwinding
also retain unconfirmed scratch. A remaining proxy is aborted rather than retained
as active network authority. The plugin-named diagnostic reports unconfirmed
cleanup without exposing resource paths. Retention is intentionally conservative;
it does not certify that the host stopped or that remote effects were undone.

The process backend's containment assumptions still apply. Escaped process groups,
native namespace/platform admission, protected staging ancestors, generation-bound
launch/revocation, quarantine of unconfirmed generations and retry fencing remain
open. In particular this owner alone does not prevent a later host incarnation
from being launched by the registry. Those obligations must be resolved before
native activation or closing the parent lifecycle requirements. The App continues
to use its legacy sandbox.

## Regressions and evidence scope

Eight real Unix process tests exercise production supervision and this owner:

- Crash and acknowledged shutdown kill the same-group child and release scratch.
- Abandoned initialization retains scratch until its queued waiter actually runs.
- Cancelling a `finish` future still lets owned cleanup complete.
- An actual externally reaped fixture leader makes the backend wait unconfirmed.
  Scratch remains; the owner does not signal the cached group. A live-child token
  roundtrip proves the child still runs, then the fixture releases it deliberately.
  A real accepted proxy client closes independently despite that unconfirmed wait.
- Dropping a separate cleanup runtime before it polls its task retains scratch.
- Dropping the owner outside a runtime retains scratch instead of pretending that
  a reap was observed.
- An actual unconfirmed supervisor wait records failed cleanup, cancels the
  connection and returns the directive that stops the immediate restart loop.

The fixed fixture provides its own direct-child PID before fork. Its test-only
waitpid reaps only that child and sends no signal. All controls release stdin
and drain the owned child's stdout before asserting, avoiding daemon leaks or
killing a reused PID. A separate sandbox test requests proxy revocation while
retaining its owner handle and observes accepted-client disposal before joining.

Missing-tree-wait, premature-resource-drop and missing-unconfirmed-retention
mutations must fail their exact named native assertions, followed by restored
tests. The existing bootstrap workflow keeps these on Linux/macOS and runs the
proxy test on every configured platform. Windows Job Object behavior and all
hosted platform results still require CI evidence; local Unix positives cannot
certify them.

## Local verification and review

On macOS arm64 all eight lifetime tests and both sandbox proxy tests pass. Each
of the three missing-guard mutations fails its named native assertion; the exact
positive source is restored and the broader plugin-host suite passes 33 tests.
The two configured Bun/Deno receipts remain ignored locally because neither
runtime is installed. The hosted bootstrap workflow installs and executes them.
Sandbox/Tools library-and-test Clippy passes with Rust warnings denied; two
deprecated Seatbelt SDK warnings are observed without assigning their origin.
Formatting, workflow YAML/13 shell blocks and ten embedded Python blocks, exact
application of all three hosted mutation scripts, diff checks, 107 strict
OpenSpec items and spec lint (zero errors, 25 warnings) pass. Fresh native platform
execution is required for this ownership revision.

The scoped code/security review covers all changed production paths, tests,
workflow and SDK/spec documentation. It identified that retaining the entire
resource bundle could retain active proxy authority; the final implementation
requests shutdown synchronously and aborts any remaining proxy before retaining
scratch. The real accepted-client/unknown-wait regression verifies this distinction.
Process identity is not signalled after an unconfirmed wait. Cleanup state remains
owned across caller cancellation, and runtime abortion leaks unconfirmed scratch
rather than deleting it. The test-only external reap is limited to the fixture's
own direct child. Registry quarantine, retry fencing and native containment are
explicit remaining integration requirements, not acceptance supplied by this review.
