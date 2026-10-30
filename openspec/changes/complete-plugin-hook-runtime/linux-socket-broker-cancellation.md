# Socket-worker cancellation proof (pending native execution)

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
