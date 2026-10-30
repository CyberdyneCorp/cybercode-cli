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
runtime families require the hosted x64/arm64 workflow; those results remain
pending. No production launch path changes in this extension.

Outside-root relative contexts, supported aliases, datagram/seqpacket,
destination-bearing sends, ancillary and peer identity behavior, proxy/network
intersections and production ownership remain required. N2–N7 and the full
goal stay open.
