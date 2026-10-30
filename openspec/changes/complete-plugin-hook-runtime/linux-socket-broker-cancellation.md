# Socket-worker cancellation proof

This extends the isolated [socket broker candidate](linux-socket-broker-candidate.md),
not production hosting or an approved support floor. The current candidate's
real thread-local table case has separate native receipts; it does not prove
cancellation settlement.

The controlled C target fills the accept queue of its own private loopback
listener. A second thread calls blocking connect. The fixture waits until
TCP_INFO reports SYN_SENT on that socket, establishing a real in-kernel attempt,
then sends SIGUSR1 specifically to that requesting thread with no restart flag.
It must observe EINTR and report both facts before waiting on its owned stdin.
The pending socket selects ten SYN retries rather than depending on a machine's
possibly short default. The listener, queued connection and pending socket stay
open during that wait.
No ambient network endpoint or credential is contacted.

An audit-only copied helper prints bounded worker-start, cancellation-request and
normal-waitpid receipts. The Rust driver releases the target only after seeing
the exact cancelled worker's successful normal reaping, matched to its start.
The C target then requires a fresh private TCP token exchange before exiting.
This establishes settlement while the target is still alive, not just teardown
when the namespace exits. The driver uses bounded line/count readers and a
30-second deadline. It terminates/waits the owned bwrap process and drains its
readers before asserting, including on a deliberately failing control. Only its
single owner waits that unreaped child; reader threads do no process signalling.

The audit markers are test instrumentation in a temporary copied header. The
base candidate and production helper sources are unchanged. The controlled target
does not forge markers on the shared stderr channel; these markers are not an
authenticated protocol for arbitrary plugins and must never grant authority.
Kernel notification identities and owned worker identities remain the actual
broker authority. The receipts make their exercised path observable to this
fixed source-bound fixture only.

The missing-cancel control removes only the owned-worker SIGKILL on notification
invalidation, retaining the audit markers and cleanup at guest exit. It must fail
the named reaping assertion after a genuine SYN_SENT/EINTR readiness receipt.
It cannot pass by delaying cleanup until target exit: the target is waiting for
the driver's acknowledgement. Neither a fixture startup failure nor a missing
notification may count as the expected missing-cancel counterexample.

Cancellation does not roll back socket effects. The SYN has already reached the
fixture network and a guest-held socket can retain network state after a worker
dies. This test establishes worker-process settlement and continued broker
service, not absence of prior effects, retry safety or network-policy acceptance.
Descriptor/address mutation, worker saturation, signal races beyond this case,
descendant/namespace lifecycle, runtime admission and the full support/design
review remain open.

Local macOS checks cover Rust example Clippy, formatting/diff, portable C syntax,
eight temporary builder transformations with compiler interception, workflow
syntax and OpenSpec validation. The Linux-specific C branch and actual signal/
seccomp/TCP behavior require the hosted x64/arm64 jobs; they are not verified here.

## Native receipts

[Run 41009708899](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/41009708899)
passed on x64 (job 123912578832) and arm64 (job 123912578660). Retained
[x64](../../../scripts/probes/plugin-native-entry/evidence/linux-socket-broker-cancel-7a8ed4e6/x64/linux-ipc-broker-cancel.json)
and [arm64](../../../scripts/probes/plugin-native-entry/evidence/linux-socket-broker-cancel-7a8ed4e6/arm64/linux-ipc-broker-cancel.json)
reports each contain four actual SYN_SENT/EINTR, exact-worker normal-reap and
subsequent TCP exchange cases across all read/write combinations, plus the 32
ordinary socket cases. Adjacent missing-cancel logs fail the named reaping
assertion after genuine interruption while the target remains waiting.

Merge checkout `6a9ac84504defd5ebe7de06e45b80da6b51ed7e6` binds source SHA-256
`5c2327b86c3ca1edad606213b88d977b734a5f5da2ce7c531a5454f985c83357`,
independently recomputed from that checkout. Audit helper hashes are x64
`ca1c090ee7a3eb939f9591bfa247bc89e9cc4f09efa1a07f93d703f5818fe5c1`
and arm64 `9a87557fa36eab0e716e5a2a67a2ae51979f97aff66cc437528dd03fdf56d139`.
The native runtime reports identify kernel `7.0.0-1017-azure`.
These receipts establish the exercised cancellation settlement, not rollback,
arbitrary-plugin marker authenticity or unperformed mutation/saturation cases.
Subsequent shared-driver changes require their own fresh hosted execution.
