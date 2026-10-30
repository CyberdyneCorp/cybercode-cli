# Private UNIX endpoint identity prerequisite

The approved [broker direction](native-host-integration.md) retains private IPC
compatibility as a requirement. The current broker experiment permits private
stream socketpairs but does not yet implement general scratch pathname IPC.
This fixture tests a necessary object-identity primitive before implementing that
mediation; it does not establish a broker policy or production confinement.

`linux_unix_endpoint_pin.c` creates only its own mode-0700 temporary fixture
directory and sockets. For stream, datagram and seqpacket sockets it pins the
private endpoint with `openat2` using `O_PATH` and beneath/no-symlink/no-magic-link
resolution, then verifies the selected inode is a socket. It unlinks the original
name and replaces it with a symlink to a second fixture-owned endpoint outside
the scratch subdirectory. The retained descriptor must still identify the now
unlinked original inode.

The pinned case connects through the trusted process's `/proc/self/fd/<fd>`
reference and must exchange a token and acknowledgement with the original private
server. The precise missing-pin control uses the replaced pathname instead and
must exchange them with the second endpoint. Each case checks that the unselected
server has no pending input/connection. These are real socket effects; a successful
open or inode comparison alone cannot satisfy the test.

Additional actual resolution requests must refuse the replacement symlink, a
parent-directory escape, an absolute path outside the pinned directory and a
directory selected as an endpoint. Every case must complete, retain the original
inode identity and prove the expected two-way exchange. Refusal of every socket
cannot count as prerequisite success.

The TypeScript driver requires all six unique cases in both the host and private
user/mount/PID/network/IPC namespace contexts. The latter has its own temporary
filesystem and procfs, drops capabilities and disables nested user namespaces.
The driver bounds process time/output and retains source/binary digests, actual
checkout, architecture and kernel alongside observations. Its self-checks reject
missing/duplicate cases, missing replacement/identity guards and a control that
does not actually reach the second fixture-owned endpoint.

Production obligations remain: exact-thread socket pinning in the real broker,
notification identity and cancellation, scratch-directory provenance, bind and
destination-bearing send mediation, safe concurrent object replacement, private
abstract IPC, exact proxy endpoint authority, network ceilings, resource lifetime
and positive runtime compatibility. Do not expose the trusted endpoint descriptor
or procfs reference to plugin code. The primitive's result cannot stand in for
these unimplemented properties.

Local macOS verification covers driver self-checks and workflow syntax only;
the Linux C branch and actual kernel operations require hosted x64/arm64 execution.
No native result is claimed yet. No production policy or #76 head changes.
