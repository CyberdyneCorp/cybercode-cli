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
binaries outside `LIBDIR`. For the Linux initial-exec proof, also run:

```sh
node --experimental-strip-types scripts/probes/plugin-confinement/run.mts --runtime node --initial-exec --report /tmp/node-initial.json
node --experimental-strip-types scripts/probes/plugin-confinement/run.mts --runtime python --python /absolute/trusted/python3 --initial-exec --report /tmp/python-initial.json
node --experimental-strip-types scripts/probes/plugin-confinement/run.mts --runtime native --report /tmp/native-initial.json
```

The native fixture implies `--initial-exec` on Linux. On macOS it runs the
four `shell: true` filesystem combinations and an unrestricted control; the
four `shell: false` combinations are explicitly reported as unsupported rather
than passed. It is an untrusted C executable with no confinement bootstrap; its
ELF/Mach-O constructor tests private reads and forks before `main`. Node headers must be under the selected Node prefix's `include/node`.
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

Each full runtime matrix runs all eight combinations of `fs.read`, `fs.write`
and `shell`, plus an unrestricted positive control. The macOS native-executable
subset reports its four unsupported no-shell combinations explicitly. Each case launches separate inspection
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

## Linux initial-exec extension

`--initial-exec` moves execution filtering into the single-threaded native
launcher before the target's first instruction. Landlock remains installed
before exec. When shell is false, `linux_initial_exec.h` transfers a seccomp
NEW_LISTENER descriptor to a trusted short-lived supervisor over a private
socketpair. It validates one notification's live ID, PID, syscall ABI and execve
operation, continues exactly that initial exec, closes the last listener, and
exits. Later execve calls receive ENOSYS; execveat and process forks are denied,
while thread-sharing clone and libc's clone3 fallback remain available.

This does not authorize syscalls from untrusted code based on pointer contents.
The only continuation occurs while the immutable trusted launcher is still
running, before any target/plugin initializer. There are no plugin handlers,
concurrent launcher threads or signal handlers that may rewrite arguments.
No later notification is continued, even if one is queued before listener close.
The filter rejects seccomp and prctl(PR_SET_SECCOMP) replacement, because a new
USER_NOTIF filter could otherwise attach an attacker-owned listener. The fixtures
attempt policy replacement with positive unrestricted controls. Without this
replacement guard, a one-shot listener is not sufficient enforcement.

Node/Python skip their late native policy call in this mode; fixture code cannot
provide enforcement. The native fixture additionally proves startup without
runtime cooperation and checks private access from an ELF constructor. These
cases run on both Linux CI architectures. The previous TSYNC bootstrap proof is
still available without `--initial-exec` for comparison; its historical evidence
must not be reattributed to the new mechanism.

The prototype's supervisor lifecycle (including reaping, early target failure,
cancellation and descriptor transfer), immutable target admission, startup
signals and packaging still require production integration and adversarial
failure tests. It is not an audited production supervisor. Read the kernel's
[user notification contract](https://docs.kernel.org/userspace-api/seccomp_filter.html)
and the [Linux API caveats](https://man7.org/linux/man-pages/man2/seccomp_unotify.2.html):
CONTINUE is unsuitable for inspecting mutable untrusted arguments. The trusted
pre-exec ordering is essential to this experiment.

## Bun, Deno and startup configuration

Use an absolute trusted runtime executable, with Node's development headers still
available to compile the Node-API fixture addon:

```sh
node --experimental-strip-types scripts/probes/plugin-confinement/run.mts --runtime bun --executable /absolute/bun
node --experimental-strip-types scripts/probes/plugin-confinement/run.mts --runtime deno --executable /absolute/deno
```

Add `--initial-exec` on Linux. The workflow pins Bun 1.4.4 and Deno 2.9.8.
Their Node-API support is a way for the fixture to enter native calls; it does
not implement the security boundary. Deno runs a CJS bootstrap with all language
permissions and disabled config/lock discovery. Deno's own `Command.outputSync`
API supplies explicit OS permission errors because its Node-compatible
`spawnSync` returned an empty object on a denied spawn in local testing. The
fixture does not interpret unknown spawn results as a successful denial.

Bun launches with `/dev/null` as its config, automatic dotenv loading disabled
and auto-install disabled. An unhardened control first proves that the dummy
project's `bunfig.toml` preload and `.env` actually load; every hardened launch
must then report neither startup artifact. HOME/config/cache directories are
private scratch. This matters on macOS because runtime initialization precedes
the native policy call. A project preload must never run with that bootstrap's
ambient authority. Native re-exec receives the dummy receipt path as an explicit
argument, avoiding Bun's JS/native environment synchronization difference.

The driver rejects runtime directory read roots that contain its private test
fixtures. Installing a runtime directly in `/tmp`, then permitting its whole
parent, must not turn a capability-denial test into broad temporary-file access.

## macOS native executable gap

```sh
node --experimental-strip-types scripts/probes/plugin-confinement/macos_native_exec.mts --report /tmp/macos-native-gap.json
```

This experiment isolates two insufficient strategies: a pre-exec no-exec policy
refuses the initial executable, and an exact-file exec exception permits that
file to re-exec itself. It deliberately grants broad reads to isolate the exec
question; it is not a candidate fs.read policy and touches no real private data.
A successful run means the assertions about these gaps passed, not that no-shell
native executables are supported. It does not prove all possible mechanisms
impossible. The native subset exercises `shell: true` only; a support revision
requires the explicit review in the approved design before integration.

## Stdio tool control and Linux Bun comparison

Inspection now consumes a bounded JSON-RPC `tool/execute` request from stdin and
returns a matching JSON-RPC response on stdout. This proves the confined fixture
can execute the native probe through a real tool request, even without filesystem
or process capabilities. It does not replace production App/server acceptance,
generation ownership or Session admission tests.

Bun 1.4.4 aborted during Linux initialization under the generic initial-exec
candidate on both CI architectures. The next workflow compares that candidate
with the native TSYNC bootstrap independently. The latter still installs
Landlock before runtime startup and applies a synchronized native execution
filter before any untrusted fixture code; it does not intercept runtime APIs.
Only a complete successful matrix can establish that candidate's behavior.
There is no production launch fallback or weakened capability contract here.

`--trace /absolute/diagnostic-directory` performs a separate strace retry after
an actual Linux failure, records it, then rethrows the original untraced failure.
A traced retry never counts as an acceptance pass. Traces contain only this
probe's dummy fixtures and sanitized launch arguments. The workflow retains both
the initial candidate's output and the independent bootstrap matrix's result.
