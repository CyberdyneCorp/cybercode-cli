# Same-group plugin child cleanup

The production host supervisor now uses the existing `Process::wait_tree()` for
leader exit and graceful shutdown instead of reaping through `wait()` first. On
Unix this observes leader exit with WNOWAIT, preserving its PID/process-group
identity, requests group termination, then reaps the leader. Reaping first lost
the identity used by `terminate()`, leaving a same-group child alive when its
host leader exited. The connection-close and timeout paths use the same tree-aware
wait after their termination request. Windows retains the backend's existing
Job Object wait behavior.

Two Unix regressions exercise the real production `supervise` and `shutdown`
functions with an actual Python leader and forked child. The leader exits while
the child holds stdout open, waiting for owner-controlled stdin EOF. Independent
JSON-RPC pipes keep connection closure from selecting a different cleanup branch;
the graceful case receives a shutdown response. The owner requires EOF on the
child-held output and verifies scratch removal after supervision. A missing-fix
control restores the four leader-only waits and must fail both named child-held
pipe assertions. Every test releases the child's input and drains its output
before asserting, so even the failing controls do not leave a daemon behind.

This fixes same-group cleanup through those host paths. It does not establish
complete native descendant containment or prove that all resources survive every
unknown settlement. A child that changes group/session, startup-error/cancelled
future cleanup, process-wait errors, native namespace/platform admission and
generation-bound resource retention still require their own implementation and
proof. Sending a termination signal is not an assertion that every possible
descendant is reaped. No parent native admission or lifetime task is closed here.

## Verification

On local macOS arm64 the two live-child regressions pass. Both fail their exact
child-held pipe assertion with leader-only waits restored, then the restored
broader plugin-host suite passes 27 tests. Two Bun/Deno startup receipts remain
ignored locally because those runtimes are absent; the focused workflow installs
and explicitly runs them. Linux/native backend and hosted platform execution
remain required. The same workflow now runs these Unix tests and controls and
retains their logs, followed by the restored suite. Tools library/test Clippy passes
with Rust warnings denied; two deprecated C Seatbelt SDK warnings are observed
without assigning their origin. Formatting, the host workflow's YAML/12 shell
blocks, main CI concurrency syntax, both corrected runtime mutation scripts,
diff checks, 107 strict OpenSpec items and spec lint (zero errors, 25 warnings)
pass locally.

The scoped code/security review checks all four waits, cancellation behavior of
the existing Process API, pipe ownership, guaranteed fixture-child release,
independent connection lifetime and the limits above. The change reuses process
ownership infrastructure rather than inventing PID discovery or killing a PID
whose leader was already reaped. This is a scoped fix, not full native activation.
