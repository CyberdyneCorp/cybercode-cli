# Descriptor-pinned socket broker candidate

This is an isolated early-proof experiment under the existing confinement gate.
It does not activate native hosting or select a production support revision.
The [prerequisite receipts](linux-pidfd-proof.md) support trying exact-thread
descriptor duplication and owned sockaddr copies on the tested Linux hosts.

The copied C launcher becomes a trusted single-threaded parent plus a guest.
The guest enters the existing filesystem policy, installs a long-lived
notification filter and transfers its listener before the trusted initial exec.
An initial proposal to keep two separate listeners was rejected during source
review: the [seccomp API](https://man7.org/linux/man-pages/man2/seccomp.2.html)
can reject a second NEW_LISTENER with EBUSY. The candidate therefore uses one
listener, with a separately bounded one-use startup decision in its parent.
Only the exact guest PID's first execve may continue, while no untrusted code or
threads have run. That budget is consumed before replying; later execve requests
receive ENOSYS when shell is false. Existing spawn/execveat denials remain in the
copied filter. With shell true, the existing inherited exec/spawn policy remains.
This is a candidate-specific change to the initial-exec mechanism, requiring real
runtime/constructor and adversarial tests plus explicit review. The production
one-shot supervisor is unchanged. The guest sees sanitized standard descriptors.
The parent is nondumpable and closes stdin/stdout. Admission still requires the
existing private mount/PID/network/IPC namespaces and dropped capabilities.

The socket filter intercepts bind, listen and connect. The parent validates the
notification, opens a PIDFD_THREAD handle for its exact requesting TID, duplicates
the socket, and copies the bounded sockaddr into parent-owned storage. It checks
the socket domain/type and copied address, then validates the notification again.
Only INET/INET6 stream/datagram objects are admitted; listen requires a stream.
An AF_UNSPEC connect may disconnect an IP object. UNIX stream pairs remain usable
but cannot bind, listen or reconnect. Other UNIX pair types remain unavailable in
this candidate. These restrictions and the PIDFD_THREAD dependency require review;
they are not silently substituted for the approved production contract.

The broker performs the operation on the pinned object and copied arguments. It
never continues a socket syscall after inspecting mutable pointers or descriptor
numbers. Dedicated forked workers keep blocking connect calls out of the event
loop. A fixed slot limit bounds workers, pipes and retained descriptors. Workers
retain only their pinned socket and result pipe, close inherited listeners and
standard descriptors, and set parent-death termination. Notification cancellation
kills and reaps the corresponding owned worker; it cannot undo an operation that
already happened. Cancellation therefore cannot establish absence of an effect.
Each worker identity remains owned until reaped, including after its reply.

The parent polls worker results, validates notification identities before replies,
and checks guest termination. A timer interrupts a receive whose pending request
disappears after poll, so cancellation cannot strand the event loop there. Guest
exit tears down all outstanding owned workers. Parent-death termination plus the
mandatory private PID namespace remain essential; escaped descendant settlement
and namespace teardown need their own native proof before production admission.

Required evidence starts with actual private TCP server/client token exchange,
UNIX stream-pair communication, denied ambient fixture sockets and forbidden pair
operations, across read/write combinations on x64 and arm64. The existing runtime
matrix must also execute this helper. Follow-up adversarial cases must cover
thread-local tables, argument/FD mutation during real notifications, cancellation,
worker saturation, signals, descendants and listener lifetime. The prerequisite
fixture alone cannot establish those broker properties. Network/proxy/secret
ceilings, immutable runtime admission and the full native support/design gate
remain open even if these initial candidate cases pass.

The initial workflow also builds a precise control that retains blanket bind
denial in the otherwise identical candidate. It must fail the actual TCP token
exchange assertion. A missing server capability therefore cannot count as this
candidate's successful confinement/compatibility result.

The kernel's [seccomp notification documentation](https://docs.kernel.org/userspace-api/seccomp_filter.html)
requires copied tracee arguments to avoid policy decisions on mutable memory.
This experiment additionally pins the actual socket object and emulates the
operation instead of using CONTINUE. Native checks must establish its behavior;
source review and successful compilation are insufficient acceptance evidence.

Local verification on macOS: all five builder modes produce their expected
temporary source transformations without changing production native source
hashes; this check intercepts the compiler and does not compile Linux C. The
Rust diagnostic example passes Clippy with Rust warnings denied. Workflow YAML,
ten shell blocks and embedded Python parse; formatting/diff checks, all 107
strict OpenSpec items and spec lint (zero errors, 25 warnings) pass. The native C
SDK build emits two Seatbelt deprecation warnings. Linux candidate compilation,
kernel behavior, the positive TCP cases, missing-bind control and runtime matrix
remain unverified until the corresponding hosted x64/arm64 jobs complete.

Scoped source/security review rejected the nested-listener design, corrected a
potential second signal after guest reaping, drains worker replies before
reaping their identities, unblocks the receive-watchdog signal, and ensures the
child closes bootstrap/listener descriptors before executing target code.
These source corrections are not native evidence. In particular the unified
startup decision and adversarial lifetime cases remain explicit review gates.
