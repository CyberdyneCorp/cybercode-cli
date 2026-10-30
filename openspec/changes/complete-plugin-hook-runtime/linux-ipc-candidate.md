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
The revised experiment also denies `bind` and `listen` for every family. This
is an explicit candidate limitation: IP server sockets and explicit client
binding are unavailable. It is not a selected production network contract.

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

## First native rejection

At `ccde59d4`, both Linux jobs reject the candidate's pair assertion. The x64
[job log](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40957529698/job/123741379932)
shows internal roundtrip succeeds, direct/disconnect/reconnect attempts fail,
datagram/seqpacket creation fails, but both `pair_bind` and `pair_listen` succeed
with project reads disabled. This refutes the assumption that a connected stream
pair must reject bind/listen. Host reachability of that newly listening pair was
not tested; no actual incoming host connection or descriptor escalation is
claimed. No failed candidate row is counted as isolation acceptance.

The revised copy adds unconditional bind/listen denials and requires pair bind
to fail too. Both native denial and runtime suites must run again. This candidate
still needs explicit design/support review because those syscall denials also
restrict legitimate IP servers and explicit client binds. A narrower production
policy, stronger primitive or reviewed support revision remains necessary; green
fixture tests alone cannot select this restriction as the final feature contract.

## Revised native component results

At `15e1ef85b1fbff552c980b50c7d22e4b35251608`, both
[x64 job 123743126666](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40958015885/job/123743126666)
and [arm64 job 123743126191](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40958015885/job/123743126191)
pass. Per architecture, all 16 ordinary host-socket attempts deny, all 16 stream
pair cases retain internal token exchange while rejecting bind/listen/reconnect,
and project reads independently match their capability. The unmodified production
helper still delivers the 12 public/outside/symlink tokens per architecture,
preserving the counterexample rather than concealing it behind candidate results.

The five runtime drivers each pass nine cases per architecture (90 cases total),
with no unsupported cases. Every runtime's input and executed helper SHA-256
matches its architecture's candidate helper hash. This proves the fixture matrix,
not all runtime applications, IP server support or production network acceptance.

[Retained original reports](../../../scripts/probes/plugin-native-entry/evidence/linux-ipc-candidate-15e1ef85/)
refer to merge checkout `a4ce695bcb8d591bb2b4f39fe55df1c9e6ede048`, whose parents
are `18791fbe0089c95708956da0f3c4a47e3035c31d` and
`15e1ef85b1fbff552c980b50c7d22e4b35251608`. Independent recomputation of each
manifest path, NUL and exact checkout bytes matches diagnostic source hash
`38213834f9fbd04afb435af7f5b173ed00ba3ab042fede482c48c30c7bd61f09` and each
runtime source report (runtime paths are relative to `plugin-confinement`). The
job-reported candidate helper hashes are
`7792832e9f12b58942eba0b0e2797b1089beb38dd03fed4b9e7bb6edfbbf58c1` (x64)
and `909a09be86ab2173c489e75a0a1b3bd4b75a0902ea511d4598518490535f3766`
(arm64). Binaries were not retained for independent rehashing. The support/design
gate and the explicit bind/listen limitation remain open despite these passes.

The compatible-mechanism investigation remains necessary: a descriptor-pinned
syscall broker is a possible next experiment for preserving private IP bind/listen
and internal pairs. It must first prove native descriptor-duplication and thread
identity support on both Linux hosts. It must operate on a pinned descriptor and
validated copied arguments rather than continue a guest-pointer syscall after a
racy inspection. No such broker is implemented or accepted here; its lifecycle,
thread/fork coverage, policy intersection and support gate remain required.
