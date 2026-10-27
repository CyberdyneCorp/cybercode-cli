# Native plugin confinement proof

This is the executable early proof for task 2.1 of
[`complete-plugin-hook-runtime`](../../../openspec/changes/complete-plugin-hook-runtime/tasks.md).
It does not change the production plugin launcher or establish its security.

Run from the repository root with Node 22.18 or later and the selected runtime's
development headers. macOS needs Xcode command-line tools. Linux needs a native
x86_64 or little-endian arm64 compiler, Landlock ABI >= 3, seccomp TSYNC,
bubblewrap with unprivileged namespaces and `--disable-userns` support:

```sh
node --experimental-strip-types scripts/probes/plugin-confinement/run.mts --runtime node --report /tmp/node-proof.json
node --experimental-strip-types scripts/probes/plugin-confinement/run.mts --runtime python --python /absolute/trusted/python3 --report /tmp/python-proof.json
```

Python must have an embeddable `libpythonMAJOR.MINOR.dylib` on macOS or
`libpythonMAJOR.MINOR.so` on Linux under its reported `LIBDIR`; framework-only
and static-only installations are not supported by this probe. The resolved
shared-library target receives an exact file read grant, including framework
binaries outside `LIBDIR`. Node headers must be under the selected Node prefix's `include/node`.
The compiler is discovered through `/usr/bin/xcrun` on macOS; Linux uses `/usr/bin/cc`. No package/runtime is
downloaded by the probe. Runtime discovery executes the explicitly selected
trusted interpreter; do not point it at untrusted programs.

The driver builds into a private temporary directory, constructs dummy package,
workspace, owner-private and scratch fixtures, runs assertions, and removes those
directories in `finally`. It clears ambient environment variables. Reports contain
runtime/platform versions, a timestamp, a digest and ordered list of the executable proof
sources, and actual observations. A failure exits nonzero and does not produce a
new success report. Use a fresh report destination or check its timestamp and
digest before treating an existing report as evidence.

## Boundary and coverage

`macos_python.c` embeds an isolated Python and preloads trusted standard-library
modules. `native_exec.c` closes inherited non-stdio descriptors before starting
Node; `node_bootstrap.cjs` loads trusted built-ins and the `macos_node.c` native
addon. Each bootstrap calls native `sandbox_init` before executing fixture code.
On macOS enforcement is in Seatbelt, with direct C/libc operations used by the fixtures;
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
- Direct native fork, shell spawning, native `/bin/cat` children, interpreter children with
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
on macOS arm64/Intel and Ubuntu arm64/x86_64 and retains reports/logs, including failure logs. Merely
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

## Linux prototype

The historical `macos_python.c` and `macos_node.c` filenames now build for either
platform via `process_policy.h`. On Linux the outer bubblewrap namespace has a
read-only host root, approved writable overlays, private PID/network/IPC/UTS/user
namespaces, disabled nested user namespaces and no retained capabilities. It
mounts mode-000 read-only empty tmpfs directories over protected subtrees;
Landlock grants are additive and cannot subtract an exception beneath an allowed
workspace. The real owner files remain outside that mount view.

`linux_exec.c` installs the Landlock read/write allowlist **before initial exec
and runtime thread creation**, then closes non-stdio descriptors and starts only
the trusted runtime/bootstrap. The initialized bootstrap installs `linux_filter.h`
with mandatory `SECCOMP_FILTER_FLAG_TSYNC` before fixture code. This covers
already-created Node runtime threads as well as subsequent workers. When shell
is disabled, execve/execveat/fork/vfork are denied; clone requires thread-sharing
flags and clone3 returns ENOSYS so libc can fall back for pthread startup.
Native ABI checks refuse alternate syscall architectures. Both shell settings
also deny ptrace, cross-process memory/descriptor access, io_uring setup,
namespace/mount mutation and selected kernel escape surfaces. These are proof
inputs, not a comprehensive audited syscall policy. Linux network namespaces
here always isolate networking; declared network/proxy support is not proved.

ABI 3 is mandatory because truncation must be controlled independently of reads.
There is no weaker fallback if Landlock, TSYNC or namespace setup fails. `fs.write`
grants write/truncate/create/remove rights without read-file/read-directory
rights. The exact system OpenSSL config file `/etc/ssl/openssl.cnf` is readable when
present for Node initialization; other private files under `/etc/ssl` are not
thereby admitted. Broad trusted runtime/system roots are experimental: their admission,
containment and overlap with owner/workspace data still require production review.

Linux code must pass the real native CI matrix; merely compiling or checking
macOS does not establish Linux acceptance. The kernel documents why Landlock
must precede runtime threads and why seccomp is only one sandbox component:

[Landlock](https://docs.kernel.org/userspace-api/landlock.html),
[seccomp filtering](https://docs.kernel.org/userspace-api/seccomp_filter.html).
