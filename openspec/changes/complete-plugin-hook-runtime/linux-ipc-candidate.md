# Linux socket-family candidate (experiment only)

The [native counterexample](linux-ipc-proof.md) proves that private IP namespaces
and ABI-3 filesystem rules leave host pathname UNIX sockets reachable. This
candidate evaluates a compatible seccomp restriction before any production or
support/design decision. It does not change production native sources or the
helper's ABI floor.

The workflow copies the current native entry and headers into an owned temporary
directory and adds a socket-family filter to that copy. The initial-exec listener
channel is created before the filter and closed before guest execution. A small
C wrapper invokes the copied entry with the same argument shape. This wrapper
is an experimental helper, not the admitted production Rust binary.

The candidate permits `socket` only for INET/INET6, still within a mandatory
private network namespace. It permits `socketpair` only for UNIX stream pairs,
including close-on-exec/nonblocking flags. It denies ambient UNIX sockets, other
families (including host vsock/netlink endpoints), and datagram/seqpacket pairs.
All existing initial-exec, architecture and escape denials remain in place.

The diagnostic tests all four read/write combinations with shell disabled. In
addition to denying the three exposed host socket routes and private socket,
it requires connected stream-pair token exchange to work, then attempts direct
connect, AF_UNSPEC disconnect/reconnect, bind/listen, addressed sendto/sendmsg,
and creation of datagram/seqpacket pairs. Host receipt remains the decisive
reachability observation. Project reads must still match their independent flag.
Node, Python, Bun, Deno and native runtime fixtures must separately retain their
existing nine-case behavior per Linux architecture with the candidate helper.

These checks are configured, not yet native verified. This experiment does not
certify all socket APIs, child IPC, real plugin-runtime compatibility, host
descriptor/stdio admission, network allowlist proxy transport or secret handling.
The permitted stream pair must not be assumed incapable of ambient connection
until the native reconnect/addressed-message controls run. Production integration
requires the native proof and explicit support/design review gate: this candidate
restricts plugin-created UNIX endpoints, which may affect legitimate runtime or
plugin IPC. No weaker feature contract is silently selected.

Local checks: example Clippy passes with Rust warnings denied and the repository's
cognitive-complexity ceiling of 15; option parsing and pair verification are
separate functions. The skill's analyzer does not support Rust, so this is the
actual Rust Clippy check. The portable C fixture passes warnings-denied compilation
and its direct stream-pair token exchange on macOS. YAML and four shell blocks
parse; the Python builder passes syntax plus an isolated-copy/injection/compiler
argument check without invoking a Linux compiler. Formatting/diff checks, all 107
strict OpenSpec items and spec lint (zero errors, 25 warnings) pass. Two deprecated
Seatbelt SDK warnings are observed without assigning their origin. Native Linux
candidate compilation, denial/positive tests and runtime execution remain pending.

The code/security review covers the BPF branches and offsets, family/type/flag
checks and restored syscall-number loads, temporary source ownership, unchanged
production sources, bounded fixed-fixture IO, native pair attempts, workflow and
report bindings. It finds no further source issue within the experiment. The
test setup is partial: local portable tests execute; Linux kernel behavior cannot
run on this macOS host. Review is not production security acceptance, and the
explicit support/design gate remains open.
