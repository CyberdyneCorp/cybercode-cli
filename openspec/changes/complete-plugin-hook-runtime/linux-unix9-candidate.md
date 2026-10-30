# Path-scoped UNIX socket alternative (experiment only)

The ABI-3 seccomp candidate's unconditional bind/listen denials are not selected
as the production contract. This alternative tests a stronger kernel primitive
to preserve real private networking and local IPC. It is built only from temporary
copies of the native entry; production sources and ABI requirements are unchanged.

The copied helper installs an additional Landlock layer, then its existing
filesystem policy, both before any runtime instruction. This layer requires ABI 9 and
handles pathname UNIX resolution and socket-node creation. It grants those actions
only beneath explicit owned scratch, never Location/bootstrap roots merely because
they have read/write grants. Abstract UNIX connections are scoped to the new
domain. The layer also handles REFER and grants REFER at root so it does not
silently add an unconditional cross-directory rename prohibition to the existing
filesystem policy. No other filesystem rights are granted at root by this layer.

The additional layer reads one bounded `U` scratch entry from the owned experimental
profile. This syntax is only in the copied C helper/probe, not the production
composer or SDK. A socket-family filter permits UNIX, INET and INET6 and UNIX
pairs, while denying unrelated ambient families. It leaves bind/listen available.
The existing namespace, descriptor and initial-exec mechanisms are retained.

The diagnostic must deny the host pathname socket routes under every independent
read/write choice, while retaining stream/datagram/seqpacket pairs, real TCP
loopback bind/listen/connect/accept/token exchange and private scratch UNIX
bind/listen/connect/accept/token exchange. Pair reconnect and binding outside
scratch remain denial assertions. A successful listen on an unnamed pair alone
is not counted as host reachability. The existing five-runtime/nine-case matrix
also runs with an explicit experimental scratch policy flag.
A separate copied-helper control removes pathname resolution from the handled
rights and must fail the exact outside-socket assertion after an actual host token
receipt. Startup/compilation failure cannot satisfy that control.

These checks are configured, not native verified. ABI 9 changes the potential
Linux support floor; this is not approved or integrated. If the alternative passes,
present its exact native evidence and the support tradeoff for explicit review.
Do not silently fall back to fewer capabilities, weaker IPC protection or global
bind/listen denial. Older kernels, network proxy transport, complete secret/stdio
intersections, owner admission, actual plugins and production acceptance remain
open. This experiment does not prove incoming host access to scratch impossible.

The ABI constants and struct layout follow the kernel's
[UAPI header](https://github.com/torvalds/linux/blob/master/include/uapi/linux/landlock.h)
and [Landlock documentation](https://docs.kernel.org/userspace-api/landlock.html).
Native results, once available, supersede assumptions about what these primitives
enforce. No parent capability or support checkbox is closed.

Local verification: example Clippy with Rust warnings denied passes; the native
C fixture compiles with `-Wall -Wextra -Werror` and its stream-pair, TCP and UNIX
token exchange controls pass on macOS. Python syntax plus all three temporary
source-copy/injection/compiler-argument variants pass without claiming Linux C
compilation. TypeScript syntax, workflow YAML/five shell blocks, Rust formatting,
diff checks, 107 strict OpenSpec items and spec lint (zero errors, 25 warnings)
pass. The C build reports two deprecated Seatbelt SDK warnings; their origin is
not assigned. Native Linux execution remains required.

The scoped code/security review checked all changed probe, copied-helper,
workflow and documentation inputs. It caught the need to install the UNIX layer
before the existing filesystem layer, since the latter may already deny reopening
the profile. Both now install after a single-threaded check and before runtime
instructions. Profile IO is bounded and no-follow; the scratch entry is explicit
and unique. Root receives REFER only. Family filters restore the syscall-number
register before later checks, and positive internal IPC/IP controls are required
alongside exact missing-policy failure. These checks are experiment review, not
production native admission or acceptance of a new support floor.

## Hosted kernel limit at `2f3b3c29`

Both [x64 job 123750657048](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40960197456/job/123750657048)
and [arm64 job 123750656386](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40960197456/job/123750656386)
compile the candidates but report Landlock ABI 8. The ABI-9 helper refuses before
target output, with `requires Landlock ABI >= 9 (observed 8)`. Consequently none
of its confinement/local-IP/local-UNIX/runtime acceptance passes on these hosts;
the missing-resolution control does not run. This is a kernel availability result,
not a proof that the alternative works or that a newer kernel is sufficient.

The workflow now records the actual ABI in a source-bound availability JSON. On
older hosts it verifies the exact refusal, explicitly records ABI-9 acceptance as
unverified, and skips its kernel-specific control/runtime matrix. The separate
ABI-3 socket-family experiment still runs normally. On a capable kernel, all
ABI-9 positives, denials, exact missing-policy control and runtime checks remain
mandatory. There is no best-effort downgrade of the policy and no production
support change. These skips do not close any acceptance gate; a compatible
mechanism or authoritative execution on a capable host remains required.
