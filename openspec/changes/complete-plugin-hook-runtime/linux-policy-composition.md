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

On local macOS arm64, all 10 policy unit tests pass, including the pure Linux
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


## Initial hosted test-scope correction

At `27f1a908`, both Linux packaged jobs failed a duplicate unsupported-host unit
assertion. The intended non-Linux check already lives in the common test module
behind `cfg(not(target_os = "linux"))`; an old unguarded copy remained in the
backend tests and incorrectly expected unavailable on Linux itself. That duplicate
is removed. The jobs stopped before the new native integration/controls, so none
of those native checks is counted as passing for that revision. Fresh hosted
execution remains required after this test-only correction.


At `b4822f2e`, both Linux jobs compile the new tests and pass private-procfs and
ancestor/positive-rename regressions. The matrix then fails `private_list`: its
primary private-file argument was under a public parent, while the shared native
fixture deliberately measures the parent directory's listing. A file mask does
not make its public parent an opaque directory. The matrix now supplies the
actual private-directory file for that directory assertion, while constructor and
symlink probes retain the private-file target. Separate direct file-read/write
assertions are added to the mutation regression. No directory or file denial is
removed; full fresh matrix and control execution remains required.

## Hosted component evidence at `546641f9`

Both Linux jobs in [run 40954544764](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40954544764)
completed successfully: [x64 job 123731111912](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40954544764/job/123731111912)
and [arm64 job 123731111847](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40954544764/job/123731111847).
This supersedes the pending Linux component results above, without closing the
remaining admission or runtime acceptance requirements.

Each job passes seven Linux policy unit tests and all eight native entry tests,
including the three new composed-policy regressions. The six individual source
mutations fail their named assertions, verified directly in the retained logs:
protected ancestor rename, immutable package write, private write, project read,
shell execution and private proc visibility. Both restored suites pass afterward;
the later descriptor-cleanup control also fails as intended and its restored
eight-test native suite passes. A failing mutation is evidence that its specific
assertion detects the removed safeguard, not a passing production test.

The [retained reports and control logs](../../../scripts/probes/plugin-native-entry/evidence/linux-policy-546641f9/)
bind these results to merge checkout `d273221de42084d0c0f45d64d7779a42b6ace819`,
whose parents are base `2548fdeaf68fdfe6e13f8ecfa87e77ae41e5bcd7` and PR head
`546641f97c0ef4dc539b017ad613397bbc250452`. Independently hashing every manifest
path plus NUL plus its exact checkout bytes reproduces both reports' source hash
`21e1e4adca3b9deaefa1a392c1da62c83386bff16c970012b5f6dbd6a4cb9545`.
The jobs report helper SHA-256 values
`22ad1c8ddc2828a2ad618dbe1289d30b91b996272cb4b1e279b38f86fb55cbff` (x64)
and `effbfd2d592bd9bdadbce618f9821a040b80949d0dc8069c8128e577dcd49d28`
(arm64). The artifacts retain provenance and logs, not helper binaries, so those
binary hashes are job-reported rather than independently rehashed downloads.

These are native fixture/component results. They do not establish production host
activation, full runtime/configuration acceptance, UNIX-socket isolation or the
owner/generation/settlement obligations listed above. Those gates remain open.
