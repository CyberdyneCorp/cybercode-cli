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

The native fixture implies `--initial-exec` on Linux. By default on macOS it runs the
four `shell: true` filesystem combinations and an unrestricted control; the
four `shell: false` combinations are explicitly reported as unsupported rather
than passed. The separate experimental candidate runs all eight combinations:

```sh
node --experimental-strip-types scripts/probes/plugin-confinement/run.mts --runtime native --macos-staged-native --report /tmp/native-staged.json
node --experimental-strip-types scripts/probes/plugin-confinement/macos_native_exec.mts --report /tmp/native-exec-gap.json
```

The candidate copies the fixture, replaces its signature with an ad-hoc signature
using a fresh identifier, and verifies every architecture slice. A system platform
launcher installs the profile before target execution; a platform-subject rule
intersected with the staged path admits initial execution but denies subsequent
exec when shell is disabled. Constructor exec, direct `posix_spawn`, platform
status before/after an attempted status change, and a formerly platform-signed
system payload are checked separately. The trusted launcher replaces itself
using `POSIX_SPAWN_SETEXEC | POSIX_SPAWN_CLOEXEC_DEFAULT`, inheriting only open
stdio descriptors. The gap test includes descriptor 500 above a lowered soft
limit of 64, with a positive inheritance control. CI now requires this candidate
on both macOS architectures; a queued or earlier default-subset run is not proof
of the new matrix. Native cases also reject turning readable package data into
a writable scratch hard link, with driver-side package-byte checks. Staging
controls modify a pre-existing input alias and verify that its distinct staged
inode/hash stay intact. Direct and sandboxed system-image positive controls are
reported independently; a signal termination cannot count as policy-denied exec.
Staging is fixture-only: immutable admission, transformed-image
integrity, entitlement compatibility and distribution remain unproved.

The native fixture is an untrusted C executable with no confinement bootstrap; its
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

## macOS private UNIX stream candidate

`--macos-private-unix` extends the pre-runtime signing candidate with scratch-only
UNIX inbound/outbound rules and real absolute/relative socket exchanges in every
capability row. For example:

```sh
node --experimental-strip-types scripts/probes/plugin-confinement/run.mts --runtime native --macos-signing-native --macos-private-unix --report /tmp/native-private-unix.json
```

Use `--macos-signing-runtime` for Node/Python/Bun/Deno and supply the same explicit
runtime arguments as the existing pre-runtime matrix. This opt-in experiment
uses an owned short temporary root under canonical `/tmp` to retain a genuine
absolute-path exchange within Darwin's UNIX address bound. CI includes a precise
missing-grant control. See the [evidence and remaining acceptance](../../../openspec/changes/complete-plugin-hook-runtime/macos-private-runtime-ipc.md).
This is not production launch policy or complete private IPC/network acceptance.

`macos_private_ipc.mts` also runs the shared profile against a fixture-owned host
listener. Its nine native cases require private socket aliases and correct peer
PID/UID/GID while denying actual host delivery. `--missing-scope` must expose the
owned host endpoint and fail the named isolation assertion. See the
[owned-listener evidence and limits](../../../openspec/changes/complete-plugin-hook-runtime/macos-owned-unix-boundary.md).
The same driver now requires real private SCM_RIGHTS reception after closing the
sender's original descriptor. Its owner observes effects on a pre-pinned file
after the guest replaces the pathname. `--drop-rights` and a copied path-reader
control must fail their precise assertions. See
[descriptor-passing evidence](../../../openspec/changes/complete-plugin-hook-runtime/macos-private-descriptor-passing.md).
It also requires private connected/sendto/sendmsg datagram exchanges and denies
six direct/symlink host routes. A bounded native owner records actual receipt
after guest settlement; startup failure cannot count as zero delivery. See
[datagram evidence and limits](../../../openspec/changes/complete-plugin-hook-runtime/macos-private-datagrams.md).
The [concurrent alias test](../../../openspec/changes/complete-plugin-hook-runtime/macos-datagram-alias-race.md)
replaces a scratch symlink while native sendto/sendmsg run and compares actual
private acknowledgements, policy refusals and owner-recorded host packets.
Its first unexpected operation/errno is retained in `failure_stage` and
`failure_errno`; successful rows require both to be empty/zero. The occupied-bind
control must report `client_bind`/`EADDRINUSE` and fail the real private exchange
assertion. Unknown errors never count as policy refusals. The retained Intel
failure now identifies a race-time send returning `EINVAL`, with its cause
still unresolved. The follow-up collects bounded kernel messages for the exact
fixture PID/directory on error and runs differential controls independently after
a positive-test failure. The positive failure still fails the job; missing logs
cannot count as confinement.

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
records local and exact-source native CI results. The dedicated GitHub Actions workflow repeats the matrix
on macOS arm64/Intel and Ubuntu arm64/x86_64 and retains reports/logs, including failure logs. Merely
adding that workflow does not prove those CI runs passed.

Apple marks `sandbox_init` deprecated in the command-line SDK's `sandbox.h`, and
custom profile flags are reserved. Local success is evidence about that host,
not a maintained distribution/support promise. This probe leaves task 2.1 open.
Before production integration, remaining work includes the pending native macOS
no-shell support decision, explicit Windows availability limits, runtime/package admission and immutable
staging (including pre-existing hard links), protected-state aliases and races,
trusted startup/dependency resolution, network/secret policy intersections,
production packaging, crash/revocation tests and actual plugin tool execution.
The ledger records passing Linux matrices for all five runtimes, passing earlier
macOS Node/Python/Bun/Deno matrices and the shell-enabled native macOS subset.
A local staged-image candidate now starts native macOS no-shell fixtures;
general production admission remains unproved. Experimental success does not
close production or platform acceptance.

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

This experiment isolates insufficient strategies: a pre-exec no-exec policy
refuses the initial executable, and an exact-file exec exception permits that
file to re-exec itself. A platform-subject predicate plus canonical literal path
lets the system sandbox launcher start the custom fixture while denying that
fixture's later self-exec. However, using a platform executable (`/bin/bash`)
as the payload retains the exception and permits its self-exec. Therefore the
predicate alone is not a general native boundary; independently enforced target
admission would need its own proof before this can be a candidate. It deliberately grants broad reads to isolate the exec
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

## Bun/Linux metadata candidate

The retained strace showed `/proc/self/cgroup` and other process/system metadata
reads denied immediately before Bun aborted, including in the independent TSYNC
bootstrap attempt. The next candidate admits read-only private procfs and selected
public cgroup/CPU metadata. It requires the existing private PID namespace,
masks its init process (`/proc/1`) and `/proc/sys`, and makes the initial-exec
supervisor non-dumpable before any target code runs. It never uses host procfs
as a fallback. Filesystem reads through `/proc/self/root` are checked against the
same workspace/private expectations, and listing the masked init process must
fail with an unrestricted positive control. These tests are pending native CI.

This is not a production read-root policy: immutable runtime/package admission,
bootstrap/protected-path overlap, all privileged-helper lifecycle/failure cases
and the complete owner/network/secret intersection still require integration
and acceptance. A metadata startup requirement must not become broad access to
owner files, parent descriptors or another plugin's state.


## Signing-identity native candidate

On macOS, run the separate experimental candidate with:

```sh
node --experimental-strip-types scripts/probes/plugin-confinement/run.mts --runtime native --macos-signing-native --report /tmp/native-signing-matrix.json
node --experimental-strip-types scripts/probes/plugin-confinement/macos_signing_exec.mts --report /tmp/native-signing-exec.json
```

The first command requires all nine native capability rows, including no-shell
constructor behavior. It signs only the trusted launcher with a fresh ad-hoc
identifier and admits the exact target path when the subject has that identifier.
The target retains its signature; the staged-platform mechanism is a separate
option and cannot be combined with this one. Other runtime modes are unchanged.

The second command requires normal initial admission and denied self-exec for
both a controlled native image and the original `/bin/bash`, without re-signing
that system image. It also requires an explicit matching-identifier counterexample
to self-exec successfully. This observed bypass makes immutable, all-architecture
helper/target identity separation a prerequisite, not an optional hardening step.
A detached-signature attempt through an O_RDONLY descriptor must leave self-exec
denied; the executable bytes must remain unchanged. The return value of
F_ADDSIGS alone does not establish that the requested signature was adopted.
The original descriptor-above-soft-limit closure control is retained.

Local arm64 evidence is in `evidence/signing-identity-local/`. Native macOS 15
arm64/Intel CI requires this candidate, including the native capability matrix
and original-signature exec controls. The older staged-system gap experiment is
available only through the workflow dispatch input `staged_system_diagnostic`.
It retains its strict positive-control assertions and known arm64 startup failure;
it is not an acceptance mechanism and its failure is not counted as passing.
These probes do not establish production immutable package admission, distribution,
all kernel signature-mutation interfaces, owner integration, or Windows support.
No production confinement or runtime availability changes are made.


### Policy before interpreter startup

`--macos-signing-runtime` applies the same candidate to Node, Python, Bun, Deno,
or the native fixture. It cannot be combined with staging or Linux initial-exec
mode. For example:

```sh
node --experimental-strip-types scripts/probes/plugin-confinement/run.mts --runtime node --macos-signing-runtime --report /tmp/node-pre-runtime.json
node --experimental-strip-types scripts/probes/plugin-confinement/run.mts --runtime python --python /absolute/trusted/python3 --macos-signing-runtime --report /tmp/python-pre-runtime.json
```

The native launcher installs policy before entering the runtime. The JS test
bootstrap loads the adversarial native addon after that boundary and does not
reapply policy. Direct Python mode requires no embedding library or headers.
Framework Python discovery uses the actual loaded image from dyld; a command-line
shim that spawns another interpreter must not receive a reusable exec exception.
The driver admits the actual stdlib and exact framework image rather than a stale
build-machine LIBDIR. Older framework Python also needs an exact read grant for
`/dev/urandom`; no general device read grant or predictable seed is substituted.
Embedded/default Python modes retain their embedding prerequisites.

Local reports in `evidence/pre-runtime-signing-local/` cover all nine cases for
native, Node 22.18.0, standalone Python 3.12.12 and framework Python 3.9.6. Bun and
Deno are not installed locally; their new pre-runtime candidate jobs and Intel
results remain required. CI separately checks both configured and OS-provided
Python, including direct framework-image resolution. The existing runtime
bootstrap matrices continue to run independently. All production identity,
package, lifecycle and support-review gates above remain open.
