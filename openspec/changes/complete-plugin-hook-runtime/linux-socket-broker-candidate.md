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

The next real-notification case creates a pthread that detaches its file table
using close_range with CLOSE_RANGE_UNSHARE. Its leader retains a UNIX socket at
the same descriptor number where that worker installs an IP server. The worker
must perform a full TCP token exchange through the broker; after joining, the
leader must still see its original UNIX object. This covers the broker's real
thread selection, beyond the earlier standalone prerequisite fixture. A copied
control removes PIDFD_THREAD and must fail this assertion while the ordinary
single-thread TCP exchange still succeeds. Descriptor/address mutation during
an already admitted operation and cancellation/saturation cases remain open.

The kernel's [seccomp notification documentation](https://docs.kernel.org/userspace-api/seccomp_filter.html)
requires copied tracee arguments to avoid policy decisions on mutable memory.
This experiment additionally pins the actual socket object and emulates the
operation instead of using CONTINUE. Native checks must establish its behavior;
source review and successful compilation are insufficient acceptance evidence.

Local verification on macOS: builder modes produce their expected
temporary source transformations without changing production native source
hashes; this check intercepts the compiler and does not compile Linux C. The
Rust diagnostic example passes Clippy with Rust warnings denied. Workflow YAML,
shell blocks and embedded Python parse; formatting/diff checks, all 107
strict OpenSpec items and spec lint (zero errors, 25 warnings) pass. The native C
SDK build emits two Seatbelt deprecation warnings. Linux candidate compilation,
kernel behavior for each revision requires corresponding hosted x64/arm64 jobs.
The new thread-specific case/control and corrected runtime report label still
require fresh hosted results; earlier receipts below cannot prove those changes.

Scoped source/security review rejected the nested-listener design, corrected a
potential second signal after guest reaping, drains worker replies before
reaping their identities, unblocks the receive-watchdog signal, and ensures the
child closes bootstrap/listener descriptors before executing target code.
These source corrections are not native evidence. In particular the unified
startup decision and adversarial lifetime cases remain explicit review gates.

## First native candidate receipts

[Run 41006793275](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/41006793275)
completed successfully on x64 (job 123903409605) and arm64 (job 123903409514).
The retained [x64](../../../scripts/probes/plugin-native-entry/evidence/linux-socket-broker-f84c3942/x64/linux-ipc-broker.json)
and [arm64](../../../scripts/probes/plugin-native-entry/evidence/linux-socket-broker-f84c3942/arm64/linux-ipc-broker.json)
reports each contain 32 socket cases: 16 direct ambient-endpoint denials and 16
pair cases requiring private stream-pair communication and an actual IP server/
client exchange across all four read/write combinations. Forbidden UNIX pair
bind/listen/reconnect and other pair types remain denied in this candidate.
Both missing-bind controls fail the named TCP assertion. These are fixture
messages only, with no ambient service or credential access.

Merge checkout `23eb5e490ffbc9733bac39d187e9d10c680ca05e` binds source SHA-256
`fdbae19e0c20cc4b0035f7f2399284260eb896f3be2da6b219f5174d74720f22`.
It was independently recomputed from that checkout before retaining the reports.
The recorded helper hashes are x64
`4ee0a18077a90e3dc2db1a990ba8bc52b7a3a380e89dec2ea550f1d7b9abe179`
and arm64 `c3f587c520b222abb506e026074fa980a8992eb20bcf039fa2d808adbd50624d`.
Each architecture's five adjacent runtime JSON reports contain nine cases per
runtime (including its unconfined control), with no unsupported cases. Their
input/executed-helper hashes match that architecture's socket report. The runtime
source manifest hash `07a4cdbfba834bb2c367745240a1cfd924a279954692ba796c48b59b629b011a`
was also independently recomputed. All report kernel `7.0.0-1017-azure`.

The retained runtime reports are unmodified originals. Their shared driver's
static mechanism label incorrectly says the listener is closed; that statement
describes the exec-only launcher and is not evidence about this broker. Here the
listener stays live for socket operations and startup authority is a consumed
parent budget. The next workflow passes an explicit socket-broker reporting flag
to label that mechanism accurately. Source/binary bindings and observed cases
support only the specific tested candidate, not that inaccurate label or the
unperformed adversarial cases. Production admission and support/design review
remain open.
