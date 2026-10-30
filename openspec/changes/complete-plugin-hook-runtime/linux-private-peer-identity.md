# Kernel peer identity diagnostic

Status: diagnostic implementation; hosted execution pending. Full private IPC
acceptance and production native activation remain open.

The approved broker direction preserves full private IPC compatibility as a
requirement. Its byte-exchange controls do not establish the accepted socket's
kernel peer identity. The candidate currently emulates connect in an owned worker
using a pinned copy of the guest socket. That needs a separate SO_PEERCRED check.

In the [Linux 6.9 UNIX socket implementation](https://github.com/torvalds/linux/blob/v6.9/net/unix/af_unix.c#L1507-L1541),
the accepted-side socket receives credentials initialized from the task doing
connect. The client receives the listener's credentials. This suggests that
worker emulation can preserve the client's view of the listener while changing
the listener's view of the caller. This is a source-based inference until the
native diagnostic supplies actual observations for the tested kernels.

The fixture creates its listener and client in the same process and exchanges
a real token and acknowledgement. It reads SO_PEERCRED from both sockets and
compares PID, UID and GID with its own identity. Its output records the actual
PID values, syscall-check count and combined identity matches.

For each read/write capability combination, the owner first runs an unconfined
control and requires both identities to match. It then runs the same fixture
under the private-stream broker, requiring exchange and listener identity to
work, but recording accepted-side identity separately. The report marks
`compatibility_accepted` from the actual match, without equating exchange with
compatibility. The workflow currently expects the source-predicted accepted PID
mismatch as a counterexample; a successful diagnostic is not acceptance of that
behavior. Startup refusal or failed exchange cannot satisfy the counterexample.

If native execution confirms this mismatch, it is a concrete unresolved
compatibility issue for the approved strategy. Return that evidence for the
required design/support review before activation. Do not silently accept a
worker identity, weaken the requirement or advertise the green diagnostic as
completed IPC support. Kernel-enforced endpoint policy that permits the guest's
own connect, or a complete protected endpoint view, would require separate
proof of both isolation and identity semantics.

Local checks cover Rust example Clippy, portable C syntax excluding Linux-only
bodies, and workflow shell/embedded Python syntax. Linux SO_PEERCRED behavior
requires hosted x64/arm64 execution. Datagram/seqpacket, ancillary behavior,
proxy/network intersections and production resource ownership also remain open.
