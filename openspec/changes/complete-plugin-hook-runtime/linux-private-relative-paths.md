# Caller-bound relative private UNIX paths

The private-stream candidate now resolves relative names using the actual
notifying thread's working directory. This extends the approved broker scope;
it is not production admission or complete pathname/IPC acceptance.

The broker opens the thread's procfs CWD as an owned directory handle, reads that
handle's kernel pathname and matches it to a beneath-root directory open in the
protected scratch tree. It compares directory type, inode, device and mount ID
from handle-based [statx](https://man7.org/linux/man-pages/man2/statx.2.html).
A missing identity field, mismatch, replacement or outside-scratch context is
refused. A caller-supplied string or the broker's own CWD cannot supply authority.

The relative name is combined with that verified scratch-relative context and
opened with RESOLVE_BENEATH, NO_MAGICLINKS and NO_SYMLINKS from the pinned scratch
root. Parent components that remain inside scratch work; traversal above the
scratch root does not. The socket endpoint is pinned and type-checked before
the final notification check and worker operation. Temporary context handles
close before worker startup; the owned endpoint handle remains through connect.
Existing fresh-root, separate-mount, protected-ancestry and proxy-origin
obligations are unchanged.

The native fixture changes its CWD independently of the broker, then requires
real token/acknowledgement exchange from the scratch root and from a nested CWD
using `../parent.sock`. It also attempts `../host.sock`, a real fixture-owned host
endpoint outside scratch. The owner records actual listener delivery before
asserting denial, instead of relying only on the target's reported errno.
The fixture restores its original directory handle before subsequent checks.

Two precise temporary controls use the broker's CWD or omit only the final
beneath-root lookup constraint. The wrong-CWD control must fail relative exchange
while absolute scratch exchange still works. The missing-beneath control must
actually connect and deliver the fixture token to the owned host listener before
failing the named escape assertion. Startup refusal or an unrelated failure does
not satisfy either counterexample. Existing mutation and compatibility controls
remain in the workflow.

The diagnostic verifier groups private-stream assertions in a separate helper;
all assertions remain. The Rust example passes the repository's Clippy complexity
limit after this extraction. The cognitive-complexity skill has no Rust analyzer,
so the project-native Clippy gate supplies the Rust measurement.

Local checks cover Rust example Clippy, portable C fixture syntax excluding Linux
branches, Python/workflow syntax, 20 compiler-intercepted builder transformations
and formatting/diff. Actual Linux C compilation, mount/CWD semantics and both
controls require hosted x64/arm64 execution. The result below supplies that
evidence for the exact tested source.

## Native result for b227c833

[Run 41084231069](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/41084231069)
passed on Linux x64 and arm64. Each architecture retains 32 cases: 16 host
endpoint denials and 16 pair cases. Each pair exchanges a token and acknowledgement
using both scratch-root relative and nested parent-relative names. Escape
attempts neither connect nor send; the fixture-owned host listener receives no
token. Independent read/write expectations remain enforced.

The wrong-CWD control fails the named relative-root exchange while absolute
scratch exchange succeeds. The missing-beneath control actually delivers to the
owned host listener before failing the named escape assertion. Original reports
and both logs are retained under
`scripts/probes/plugin-native-entry/evidence/linux-private-relative-b227c833/{x64,arm64}`.
They bind tested merge `b719d619418f36f66f553c1582b09a99195c7ce5`, whose tree
equals head `b227c83329acf58b50a1239b937e25fc68475a5f`, and source manifest SHA-256
`f53cfe901242e30b8fdcd3bab1a90ac62ff2f451722339f95052d7b378239cad`.
The manifest and tree equality were independently verified. These reports do
not cover the subsequent runtime API exchange extension.

Relative names beginning from outside scratch, supported symlink/alias forms,
datagram/seqpacket and destination-bearing sends, ancillary/peer-credential
behavior, proxy/network intersections and production ownership remain required.
The full native proof/activation gate and task N2 remain open.
