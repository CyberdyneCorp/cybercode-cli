# Runtime private pathname IPC acceptance

The private-stream candidate's startup matrix previously checked file, thread
and execution behavior without using each runtime's own pathname socket API.
This extension requires real token/acknowledgement exchanges over both absolute
and caller-CWD-relative scratch sockets. It implements part of task N2 under the
approved [native host direction](native-host-integration.md).

Node, Bun and Deno use the existing untrusted CommonJS bootstrap and a shared
`node:net` fixture. Python uses its socket API; the native fixture uses direct
socket calls. Each binds a listener, connects a separate client, sends `T` and
requires `A` in response. The relative case changes and restores the runtime's
CWD, independently of the broker's CWD. Sockets are closed and owned pathname
listeners removed before restoring that context. Operations have time limits;
the outer harness retains its process deadline.

Only `--broker-private-stream` enables these expectations. All nine rows per
runtime, including the first unconfined control and all eight read/write/shell
combinations, require `private_unix: [true, true]`. Original capability
assertions remain. The source manifest includes the new shared fixture.

A temporary missing-support helper denies new UNIX socket creation while
retaining stream socketpairs and the rest of the candidate policy. The Python
matrix must pass its unconfined row, then fail specifically at
`no-capabilities: private UNIX runtime exchange failed: [false,false]`.
Startup refusal or an unrelated assertion cannot satisfy this control.

Local unconfined component checks pass for Node, Python and the native exchange
functions, including CWD restoration and pathname cleanup. Portable C syntax,
TypeScript/JavaScript/Python syntax and workflow checks are available on macOS.
Bun and Deno are unavailable locally. Real Linux confinement and all five
runtime families require the hosted x64/arm64 workflow. No production launch
path changes in this extension.

## Native result for 077fb62e

[Run 41089801122](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/41089801122)
passes both Linux jobs. Each architecture's Node, Python, Bun, Deno and native
reports contains all nine rows with `private_unix: [true, true]` and no unsupported
rows: 45 runtime cases per architecture. The missing-create control passes the
unconfined row and fails at the exact named no-capabilities assertion with both
exchange results false.

Original runtime reports, the socket report binding their helper digest, and the
control log are retained under
`scripts/probes/plugin-native-entry/evidence/linux-private-runtime-077fb62e/{x64,arm64}`.
They bind tested merge `e161ed17510ed29120fad191610d9fc5e900edee`, whose tree equals
source head `077fb62e04ac6bbdeaa66cbdf465dab12a7647cc`, kernel
`7.0.0-1017-azure`, runtime manifest SHA-256
`8a8fa6f23f4e209611af317fd7bb2babdbe4f0ffde7b26e4c95822fef0679288`
and socket manifest SHA-256
`7fc859f57cf92e4a0c072b9a3d6d814360fa5e774a226f8c1743f544de6ce35a`.
Both manifests and tree equality were independently verified. Every runtime's
input and executed helper digest matches its architecture's socket report.
These receipts do not cover the subsequent alias/mount-identity extension.

Outside-root relative contexts, supported aliases, datagram/seqpacket,
destination-bearing sends, ancillary and peer identity behavior, proxy/network
intersections and production ownership remain required. N2–N7 and the full
goal stay open.
