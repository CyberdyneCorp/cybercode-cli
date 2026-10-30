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
No production policy or #76 head changes.

## Native result

The endpoint identity steps in
[run 41053828357](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/41053828357)
passed on x64 (job 124056240517) and arm64 (job 124056240704). Each original
report contains six unique cases in each of the host and private namespace
contexts: 12 cases per architecture, 24 total. All require actual token/ack
exchange, including the missing-pin controls reaching the second fixture endpoint.
Every guard and pinned inode-retention assertion passed. The rest of that broader
workflow remains separately pending; these step results do not claim its completion.

Reports are retained unchanged at
`scripts/probes/plugin-native-entry/evidence/linux-unix-endpoint-e968c44e/{x64,arm64}/linux-unix-endpoint.json`.
Both bind checkout `e968c44e0c0d4d54d578630350a4ed9b65f0f02e`, kernel
`7.0.0-1017-azure` and source manifest SHA-256
`fccc9972e359ce3ecbfc5c240d8b2a6327a152b1ff40c53999773da234ca6f34`.
That manifest was independently recomputed from the exact Git tree before
retaining the reports. Binary digests remain in each original report; later
revisions do not inherit these observations automatically.

The scoped review covers owned fixture paths, exclusive temporary creation,
bounded nonblocking socket I/O, cleanup, actual endpoint selection and reverse
acknowledgement, exact missing-pin effects, strict case uniqueness and negative
driver self-checks, private namespaces and retained source bindings. The positive
result permits trying this primitive in the real broker. It does not complete
general private IPC, notification ownership, bind/send mediation, network/proxy
intersections or production activation acceptance.
