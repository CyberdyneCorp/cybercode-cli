# Composed Linux plugin policy

`cyber-sandbox::plugin_policy::LinuxPolicy` now constructs the native helper's
Landlock profile and its bubblewrap arguments together. This promotes the
[verified mount primitives](linux-mount-protection.md) into a production library
component. It does not activate the production host or supply an execution permit.

The owner supplies canonical, bound helper/target/profile/package/scratch paths,
explicit reviewed bootstrap roots, private file/directory masks, read-only roots,
Location, target arguments and independent read/write/shell choices. The mask
source must be a new owned, singly linked, mode-000 regular file protected until
process settlement. Composition does no filesystem IO and cannot attest any of
those native facts. It never discovers executables through PATH.

The plan requires private user/PID/network/IPC/UTS namespaces, drops capabilities,
disables nested user namespaces and installs fresh procfs and minimal devices.
Location and every protected ancestor inside it are mounted first. Immutable
resources are rebound read-only next, followed by private masks and writable
scratch. Nested private roots collapse beneath an opaque private directory so no
later mount tries to create a child inside a sealed parent. Conflicting declared
kinds and private file ancestors refuse composition.

Landlock grants package, target, explicit bootstrap and scratch reads. Only
scratch and /dev/null get unconditional writes; Location reads and writes are
independent. Shell reaches the packaged entry as its mandatory initial-shell flag,
not as an unfiltered fallback. The existing helper installs Landlock and its
one-shot process boundary before the runtime's first instruction. Host CWD is
the package directory, consistent with the startup contract.

Optional procfs reads refer only to the fresh private procfs. /proc/1 and /proc/sys
are masked. Host bootstrap paths beneath /proc or /dev refuse input rather than
being rebound over the new namespace objects. Only the explicit private-procfs
option and fixed null/entropy devices add those reads.

## Bounds and refusals

At most 64 supplied paths and 64 KiB of path text are accepted, with 4,096 bytes
and 64 components per path. Relative/root/parent-traversing/control-character or
non-UTF-8 paths refuse. Scratch cannot overlap immutable/private roots or contain
Location. The target's internal helper arguments fit its 256-argument/64-KiB
frame; generated bubblewrap arguments are bounded to 1,024 entries and 128 KiB.
The profile is capped at 64 KiB while rendering. Errors are static and do not echo
private paths or argument values.

Public composition returns unavailable on non-Linux hosts. The pure backend is
also compiled in Unix unit tests so local macOS can exercise serialization and
bounds without claiming native Linux execution. This changes no ordinary command
sandbox or App host behavior.

## Verification status

On local macOS arm64, all 11 policy unit tests pass, including the pure Linux
backend and explicit public unavailability on this host. All eight existing
packaged macOS entry regressions pass. Sandbox all-target Clippy passes with Rust
warnings denied; the C compiler reports two deprecated Seatbelt SDK calls, without
assigning their origin. Both new C fixtures pass syntax-only compilation. Rust
formatting, provenance-script TypeScript syntax, workflow YAML/all seven Bash
blocks, diff checks, all 107 strict OpenSpec items and spec lint (zero errors,
25 warnings) pass. Linux-specific test compilation and kernel execution cannot
run on this macOS host and remain required in CI.
Linux CI must execute the three new actual-helper regressions:

- Eight read/write/shell combinations, including constructor-time policy,
  threads, child inheritance, real JSON-RPC receipt, private file/directory masks,
  symlink/hard-link attempts and unchanged owner fixture bytes.
- Private/package parent and grandparent renames, immutable package writes,
  allowed public sibling writes and public directory rename.
- Private procfs visibility and /proc/self/root aliases with Location read both
  disabled and enabled, without exposing init/sys or restoring private reads.

Six source-mutation controls must fail their named native assertions, followed by
restored unit/native suites. Their execution is configured, not locally passed:
ancestor binds, immutable binds, private masks, read capability, shell capability
and host procfs re-exposure. Source provenance includes the builder and fixtures.

## Remaining native admission and intersections

All source/runtime/helper/mask/ancestor ownership, all-alias coverage, current
plugin/digest/Location generation binding, serialized start, revocation and child
settlement remain caller obligations. The owner must supply every configuration,
protected-root and trust ceiling; these are not derived by the composer.

A private IP namespace is not acceptance of the complete network capability.
Pathname UNIX-socket/host-IPC isolation, proxy transport and secret intersections
still require dedicated native verification and implementation before activation.
The kernel documents distinct [network-namespace scopes](https://man7.org/linux/man-pages/man7/network_namespaces.7.html)
and [Landlock rights/scopes](https://docs.kernel.org/userspace-api/landlock.html);
this increment neither changes the helper's ABI floor nor certifies those missing
intersections. No weaker support decision is substituted for them.

The App still uses its legacy sandbox. Reviewed runtime resolution, immutable
resource admission, complete native runtime/platform acceptance and explicit
support/design review remain open. No parent checkbox is closed, and protocol
features or the later Daily coding/binary workstreams are not declared complete.


The scoped implementation review checked every changed composer, unit/native
fixture, workflow/control and provenance input. Paths and target argument lengths
are bounded before copying or pairwise analysis; output is bounded incrementally.
The builder performs no owner filesystem IO. Mount ordering preserves writable
siblings while sealing immutable/private subtrees, and no host procfs/devices are
rebound over the new namespace objects. Positive native assertions, exact failing
control reasons and restored checks are configured rather than waived. Review
found no remaining local source issue within this component; native results are
still pending and production activation remains blocked by the obligations above.
