# Generation-bound final launch commit

Task 2.2 requires the final process start to serialize with generation revocation.
A path read earlier through `Package::path()` cannot establish that ordering.
The preparation authority now supplies a synchronous final-effect gate, without
claiming native OS admission or complete production host integration.

`Generation::commit(&lease, start)` checks that the lease belongs to this exact
owner, acquires the shared generation gate, checks revocation again, and performs
the synchronous callback before releasing the gate. `Generation::revoke()` uses
the same gate: revocation that wins prevents the callback; a committed start that
wins completes before revocation returns. An unrelated fresh owner cannot use an
old lease, even if both remain live or the package content is identical.

`Package::commit(&owner, start)` supplies the owned destination path inside that
same commit, instead of asking the caller to copy a path and check later. This
only establishes process-local generation authority. The callback must perform
the actual synchronous spawn, not return a deferred future. The owner is borrowed
for the complete call; a compile-fail example verifies it cannot be moved/dropped
inside the callback. The gate is non-reentrant: callbacks must not call revoke or
commit on the same authority.

A startup panic permanently revokes the generation and wakes `Lease::revoked()`
before releasing the poisoned gate. Later commits fail closed. Explicit revoke
and owner drop recover the poisoned mutex only to publish permanent revocation;
they cannot renew the authority. Leases retain the private notification machinery
but never keep owner authority live after `Generation` drops. No plugin-supplied
identifier can manufacture a lease or owner binding.

The production host must still own one generation per plugin/digest/Location
incarnation, bind all admission work to it, observe revocation, and stop/reap a
running process before releasing its image, profile, scratch and proxy resources.
A successful synchronous spawn can precede revocation; this gate does not itself
terminate that process. Immutable package/runtime/helper admission, namespace and
protected-root policy, native runtime support and actual host launch integration
remain required. No native proof/design gate, platform support requirement or
parent task is closed by this increment.

## Verification and review

Regression coverage includes a same-owner revoked commit, owner drop, foreign
live-owner refusal, a concurrent admitted start versus revocation, startup panic
and notification/poison recovery, and a real Unix process positive start followed
by refusal of a second process. The existing actual-helper package replacement
regression now verifies path binding inside commit, refusal through a different
live owner, stale-package refusal and refusal after owner drop. It retains its
identical-content replacement and cleanup checks.

The concurrency regression settles both worker threads before asserting a
failure, and verifies the gate remains held independently of scheduling delays.
Missing-check and missing-gate behavioral controls, local test results and native
platform CI must be recorded before acceptance. The existing five-platform
package-copy workflow selects these regressions through `plugin_image` and the
actual `plugin_copy` integration target.

The scoped code/security review covers ownership and binding, watch lifetime,
mutex poisoning and panic ordering, callback reentrancy and asynchronous misuse,
process/resource lifetime, and unchanged preparation cancellation/SQLite lock
behavior. The first draft exposed commit through a lease alone; review tightened
it to require a borrowed matching owner, preventing accidental owner capture/drop
inside the final callback and explicitly rejecting cross-generation authority.
This is an implementation review, not an independent security audit.

On local macOS arm64, all 14 owner/authority tests and five actual-helper tests
pass; the helper suite's two ignored fixture entrypoints are explicitly invoked
by its SQLite/digest regression. The compile-fail ownership example passes.
Removing the commit gate fails the named serialization test; removing the final
lease check fails the named revoked-start test. The exact positive source is
restored and all 14 owner/authority tests pass again. Scoped sandbox all-target
Clippy passes with Rust warnings denied; its native build reports the two
explicitly permitted deprecated Seatbelt APIs. Formatting/diff checks, workflow
YAML and all Bash blocks pass. Strict OpenSpec passes 107 items, while spec lint
reports 0 errors and 25 warnings. Hosted current-source platform checks remain
pending; local tests do not establish Windows or Linux execution acceptance.
