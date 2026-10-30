# Kernel peer identity diagnostic

Status: accepted-side PID mismatch confirmed on Linux x64 and arm64. Full private
IPC acceptance and production native activation remain open. The
[kernel UNIX revision](linux-kernel-unix-revision.md) is proposed for review.

The approved broker direction preserves full private IPC compatibility as a
requirement. Its byte-exchange controls do not establish the accepted socket's
kernel peer identity. The candidate currently emulates connect in an owned worker
using a pinned copy of the guest socket. That needs a separate SO_PEERCRED check.

In the [Linux 6.9 UNIX socket implementation](https://github.com/torvalds/linux/blob/v6.9/net/unix/af_unix.c#L1507-L1541),
the accepted-side socket receives credentials initialized from the task doing
connect. The client receives the listener's credentials. This suggests that
worker emulation can preserve the client's view of the listener while changing
the listener's view of the caller. The native diagnostic below confirms the
mismatch on the tested kernels; it does not prove every possible broker design
has the same limitation.

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

Native execution confirms this mismatch, a concrete unresolved compatibility
issue for the current implementation of the approved strategy. Return the evidence for the
required design/support review before activation. Do not silently accept a
worker identity, weaken the requirement or advertise the green diagnostic as
completed IPC support. Kernel-enforced endpoint policy that permits the guest's
own connect, or a complete protected endpoint view, would require separate
proof of both isolation and identity semantics.

## Native result for 4d7b13ec

[Run 41092065028](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/41092065028)
passes both Linux jobs by recording the expected counterexample. Each
architecture's report has 36 rows: the existing 32 socket rows and four
peer-identity diagnostic rows, one for each read/write combination. All four
unconfined controls exchange successfully and report the fixture's actual PID
at both ends. All four confined rows exchange successfully and correctly report
the listener as PID 3, but the accepted side reports PID 4 instead of caller PID 3.
Every diagnostic row explicitly records `compatibility_accepted: false`.

Original socket reports, ABI availability reports and exact ABI-9 refusal logs
are retained under
`scripts/probes/plugin-native-entry/evidence/linux-private-peer-4d7b13ec/{x64,arm64}`.
They bind tested merge `1f27f418bf3654e99e01e44f9d38760ff7f473c1`, whose tree
equals source `4d7b13ec7177680ccb6dcbf73d4bf06c94f25c11`, kernel
`7.0.0-1017-azure`, and source manifest SHA-256
`97e1ca54276e939ba4abe7bb0e648ac9a74fec98da1010a85df64e821924dad7`.
The manifest and tree equality were independently verified. Both hosts report
Landlock ABI 8 and explicitly refuse the existing ABI-9 alternative. That
alternative's positive execution remains unverified.

Local checks cover Rust example Clippy, portable C syntax excluding Linux-only
bodies, 21 workflow shell steps/14 embedded Python blocks, formatting/diff,
107 strict OpenSpec items and spec lint (zero errors, 25 warnings). The C build
emits two deprecated Seatbelt SDK warnings. Datagram/seqpacket, ancillary behavior,
proxy/network intersections and production resource ownership also remain open.
