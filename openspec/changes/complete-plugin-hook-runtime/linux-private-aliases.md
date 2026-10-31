# Private UNIX aliases and outside-scratch caller contexts

This extends the approved broker's private-stream candidate under task N2. The
earlier [relative-path proof](linux-private-relative-paths.md) accepted only a
CWD already inside scratch and rejected symlinks. The current candidate instead
pins the resulting socket and checks its mount identity against the pinned,
separately mounted scratch root. It still requires protected scratch provenance
and does not activate production native hosts.

For a relative name, open `/proc/<notification-tid>/cwd` as a directory handle
and use that exact object for lookup. The broker's CWD and a reconstructed string
cannot supply context. Absolute names resolve in the shared protected mount
namespace. `openat2` uses O_PATH and RESOLVE_NO_MAGICLINKS. Ordinary symlinks may
resolve, including absolute links and directory components; no content is read.
The final handle must describe a socket on the exact scratch mount, with matching
device identity and returned statx mount/type fields. Both mount-bearing handles
stay pinned through validation; the endpoint remains owned through worker connect.

This uses the kernel's [handle-based statx mount identity](https://man7.org/linux/man-pages/man2/statx.2.html)
and [openat2 resolution controls](https://man7.org/linux/man-pages/man2/openat2.2.html).
The candidate no longer treats lexical beneath-root lookup as the authorization
boundary. Endpoint mount membership is the boundary, including when a path
temporarily traverses outside scratch. Missing metadata, a different mount,
non-socket endpoints and magic links are refused. Final notification validity
and the pinned socket/endpoint worker operation are unchanged.

The owner must establish a fresh scratch tree, bind it separately, protect its
source and all writable aliases from other hosts, exclude foreign sockets at
startup and prevent guest mount manipulation. A mount ID is not an ownership
proof without those preconditions. Cross-root hard links still fail EXDEV;
descendant mounts do not inherit scratch endpoint authority.

Each native private-stream pair case now requires token/acknowledgement exchange
from scratch root, from a nested parent-relative name, from a CWD outside scratch,
through relative and absolute socket symlinks, and through a directory symlink.
Direct parent escape and a scratch symlink to the owned host listener must both
fail. The owner checks actual host delivery in addition to target observations.

Three temporary controls isolate context, security and compatibility. Wrong-CWD
lookup must fail the named root-relative exchange after absolute exchange works.
Omitting only the endpoint mount guard must actually deliver to the owned host
listener and fail `socket denial: outside`. Adding NO_SYMLINKS must preserve root,
outside-CWD and absolute scratch exchanges, then fail the named relative-alias
exchange. Generic startup failures do not satisfy these controls. The prior
missing-beneath proof is retained as evidence of the earlier algorithm, while
the current workflow uses the missing-mount control.

Local checks cover portable C syntax, Rust example Clippy, intercepted candidate
transformations, Python/workflow syntax and spec validation. An unconfined macOS
component check exercises all six positive fixture exchanges and both reachable
owned host controls, adapting O_PATH/CLOEXEC flags for that platform. It does not
test Linux flags, mount identity or confinement. The hosted result below supplies
Linux evidence for the exact tested revision.

## Native result for 2f1fee99

[Run 41091051579](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/41091051579)
passes both Linux jobs. Each architecture has 32 socket rows: 16 direct host
denials and 16 pair cases. All six pathname forms exchange successfully in every
pair case, while direct and symlink escapes neither connect nor send and the
owned host listeners receive no token. Independent read/write expectations and
the original broker/runtime regression matrices pass.

Wrong-CWD lookup fails its named relative-root exchange. Removing only the mount
guard actually delivers to the owned outside listener and fails the named
socket-denial assertion. Adding NO_SYMLINKS preserves absolute scratch,
relative-root and outside-CWD exchanges, then fails the named relative-alias
assertion. Original socket reports and the three logs are retained under
`scripts/probes/plugin-native-entry/evidence/linux-private-aliases-2f1fee99/{x64,arm64}`.
They bind tested merge `4d8c4576b941f32fdcad6dc97c0c6e8396e28108`, whose tree
equals source `2f1fee99b730db5d3d37876b603e5c059399088d`, and source manifest
SHA-256 `ca4ffbe66815bc3a696f0296c9ffbe978244b0d511fc09c65c3d802409a4da52`.
Tree equality and the manifest were independently verified. These receipts do
not cover the subsequent peer-identity diagnostic.

Datagram/seqpacket, destination-bearing sends, ancillary and peer identity,
proxy/network intersections and production admission/settlement remain required.
N2–N7 and the parent tasks remain open.
