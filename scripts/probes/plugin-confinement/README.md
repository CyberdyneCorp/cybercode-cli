# Native plugin confinement proof

This is the executable early proof for task 2.1 of
[`complete-plugin-hook-runtime`](../../../openspec/changes/complete-plugin-hook-runtime/tasks.md).
It does not change the production plugin launcher or establish its security.

Run from the repository root on macOS with Node 22.18 or later, Xcode command-line
tools, and the selected runtime's development headers:

```sh
node --experimental-strip-types scripts/probes/plugin-confinement/run.mts --runtime node --report /tmp/node-proof.json
node --experimental-strip-types scripts/probes/plugin-confinement/run.mts --runtime python --python /absolute/trusted/python3 --report /tmp/python-proof.json
```

Python must have an embeddable `libpythonMAJOR.MINOR.dylib` under its reported
`LIBDIR`; framework-only and static-only installations are not supported by this
probe. Node headers must be under the selected Node prefix's `include/node`.
The compiler is discovered through `/usr/bin/xcrun`. No package/runtime is
downloaded by the probe. Runtime discovery executes the explicitly selected
trusted interpreter; do not point it at untrusted programs.

The driver builds into a private temporary directory, constructs dummy package,
workspace, owner-private and scratch fixtures, runs assertions, and removes those
directories in `finally`. It clears ambient environment variables. Reports contain
runtime/platform versions, a timestamp, a digest of the seven executable proof
sources, and actual observations. A failure exits nonzero and does not produce a
new success report. Use a fresh report destination or check its timestamp and
digest before treating an existing report as evidence.

## Boundary and coverage

`macos_python.c` embeds an isolated Python and preloads trusted standard-library
modules. `native_exec.c` closes inherited non-stdio descriptors before starting
Node; `node_bootstrap.cjs` loads trusted built-ins and the `macos_node.c` native
addon. Each bootstrap calls native `sandbox_init` before executing fixture code.
Enforcement is in Seatbelt, with direct C/libc operations used by the fixtures;
there is no filesystem or subprocess API interception. The unrestricted `-`
profile is a test control, never a permitted production launch mode.

Each runtime runs all eight combinations of `fs.read`, `fs.write` and `shell`,
plus an unrestricted positive control. Each case launches separate inspection
and interpreter-reexecution processes. Assertions cover:

- Native package reads and private scratch round trips.
- Independent workspace reads/listing and writes, including writing without
  gaining read access to the resulting file.
- Private sibling and nested owner-state read/write/list denial, symlink escape,
  a hard-link attempt and a workspace-parent rename attempt.
- Removal of an inherited descriptor referring to the dummy private file.
- A worker thread performing native workspace/private reads and attempting a
  shell spawn under the same restrictions.
- Direct shell spawning, native `/bin/cat` children, interpreter children with
  inherited read restrictions, and native interpreter re-execution.
- Failure to relax an already installed policy.

The root directory has a literal directory-data grant for runtime startup. This
does not grant descendant reads. Trusted system/runtime read roots are explicit
in `run.mts`; they are experimental bootstrap inputs, not a reviewed production
allowlist. With no workspace read capability, cwd is private scratch: Node worker
startup otherwise fails when it asks for its inaccessible current directory.
Node re-execution uses a dummy scratch receipt because its stdout descriptor can
be close-on-exec; process success and receipt content are both asserted.

## Evidence and limits

The [native evidence ledger](../../../openspec/changes/complete-plugin-hook-runtime/native-proof.md)
records local results. The dedicated GitHub Actions workflow repeats the matrix
on macOS arm64 and Intel and retains reports/logs, including failure logs. Merely
adding that workflow does not prove those CI runs passed.

Apple marks `sandbox_init` deprecated in the command-line SDK's `sandbox.h`, and
custom profile flags are reserved. Local success is evidence about that host,
not a maintained distribution/support promise. This probe leaves task 2.1 open.
Before production integration, remaining work includes Linux native proof,
explicit Windows availability limits, runtime/package admission and immutable
staging (including pre-existing hard links), protected-state aliases and races,
trusted startup/dependency resolution, network/secret policy intersections,
production packaging, crash/revocation tests and actual plugin tool execution.
Bun, Deno, native plugins and other runtime/OS combinations are unproved.

Linux investigation must account for both native filesystem and execution
restrictions. The kernel documents that Landlock restricts the calling thread
and future children; existing runtime threads need deliberate treatment.
Seccomp filtering is a component rather than a complete sandbox. These are
design inputs, not implemented or accepted Linux support:
[Landlock](https://docs.kernel.org/userspace-api/landlock.html),
[seccomp filtering](https://docs.kernel.org/userspace-api/seccomp_filter.html).
