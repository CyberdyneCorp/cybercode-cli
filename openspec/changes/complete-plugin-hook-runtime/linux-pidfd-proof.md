# Descriptor-pinned broker prerequisites (experiment only)

The ABI-3 global bind/listen restriction and currently unavailable ABI-9 alternative
remain unselected. This experiment checks whether an unprivileged native parent
can safely pin the exact requesting thread's socket and operate on a copied
address. It is not the broker itself, a capability policy or production admission.

The controlled child creates a worker with an independently unshared descriptor
table. The same descriptor number denotes a UNIX socket in its thread-group
leader and an INET socket in the worker. A TGID-only duplicate must expose the
wrong-family counterexample; `PIDFD_THREAD` plus `pidfd_getfd` must select the
worker's actual socket. The parent reads only the fixed fixture sockaddr from
that child, then has the child overwrite the original address before binding
and listening through the pinned descriptor and copied address. The child must
observe the resulting bound port.

Next the worker replaces its descriptor number with a UNIX socket. A fresh
lookup must see UNIX while the retained descriptor remains INET and completes
a real TCP token exchange. After the worker exits, its pidfd must fail rather
than select the still-live leader's table. Finally the controlled child becomes
nondumpable and both descriptor duplication and memory copying must refuse with
EPERM; no privilege fallback is attempted.

The driver runs this in the ordinary host and in private user/PID/network/IPC/UTS
namespaces with capabilities dropped. Only fixture-owned sockets and memory are
used. Pipe frames are bounded; each transfer has a deadline. Failure paths kill
and reap the exact owned child before reporting, and parent death closes the
command pipe so the child cannot remain a daemon. Reports bind checkout/run,
source manifest/hash and probe binary hash to observations.

Explicit kernel/permission refusals are recorded as prerequisites unavailable,
not passing confinement or runtime acceptance. Resource exhaustion and failed
assertions remain failures. Native Linux x64/arm64 execution is required; macOS
can validate only the TypeScript driver and workflow. `PIDFD_THREAD` is a kernel
feature dependency, not an approved new production support floor.

The kernel [pidfd UAPI](https://github.com/torvalds/linux/blob/master/include/uapi/linux/pidfd.h)
defines the thread flag. The [pidfd_open manual](https://man7.org/linux/man-pages/man2/pidfd_open.2.html)
documents its Linux 6.9 introduction, and the
[pidfd_getfd manual](https://man7.org/linux/man-pages/man2/pidfd_getfd.2.html)
documents duplication of an open file description and ptrace permission checks.
The actual hosted result must establish availability rather than infer it from
a kernel version string.

Even a positive prerequisite result leaves seccomp notification ownership,
copied-address validation, cancellation, descendants/threads, namespace and
network/proxy/secret ceilings, runtime compatibility and explicit support/design
review open. No production source or support decision changes in this experiment.

Local verification: TypeScript syntax and the driver's positive/unavailable plus
four negative classification controls pass. Workflow YAML and nine shell blocks,
diff checks, all 107 strict OpenSpec items and spec lint (zero errors, 25 warnings)
pass. Linux C compilation and kernel execution cannot run on this macOS host and
are not claimed verified; both native architecture jobs are required.

The scoped source/security review checks fixed-size pipe frames, thread-table
ownership, snapshot/pinned-object ordering, explicit unavailability classification,
child reaping on every report path and cleanup on parent death. It tightened the
reap result check, distinguishes short memory copies from permission refusals,
and makes guest-address mutation volatile so the counterexample is not removed
by optimization. The probe reads only its own fixture child's memory. It does not
change the one-shot exec supervisor, authorize later exec or inspect real plugins.
Native results remain authoritative for whether the proposed prerequisites work.

## Native prerequisite receipts

[Run 40990090248](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40990090248)
completed successfully on Linux x64 and arm64. The retained
[x64 report](../../../scripts/probes/plugin-native-entry/evidence/linux-pidfd-7936cb6d/x64.json)
and [arm64 report](../../../scripts/probes/plugin-native-entry/evidence/linux-pidfd-7936cb6d/arm64.json)
each contain passing host and private-namespace observations on kernel
`7.0.0-1017-azure`. Both establish thread-table selection, socket pinning across
descriptor replacement, copied-address mutation followed by a TCP exchange,
ESRCH after thread exit and EPERM for nondumpable-child access.

The reports bind merge checkout `c42831be5ee522833b802960c9e568db8433a522`
to source SHA-256 `9ef84af7a5ee191fbaa25127844674c7758e00dc31330616d4905f5488462a9f`.
That manifest was independently recomputed from the committed checkout when
retaining these receipts. Probe binary hashes are reported by the native jobs
inside each receipt; local macOS verification does not independently execute
those Linux binaries. These four positive cases support implementing an isolated
broker candidate. They do not prove confinement, compatibility on other kernels,
production admission or an approved support-floor change.
