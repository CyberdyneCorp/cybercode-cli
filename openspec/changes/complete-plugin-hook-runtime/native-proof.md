# Native capability proof ledger

This records an experimental proof, not production launcher acceptance. The
approved task 2.1 remains open until the required platform/runtime support and
production-admission decisions are proved.

## Local execution on 2026-10-27

Darwin 25.6.0, arm64; Node 22.18.0 and
CPython 3.13.5. All nine cases passed for each runtime: one
unrestricted positive control plus all eight read/write/shell combinations.
Each case runs inspection and direct native interpreter re-execution in separate
processes (36 primary launches across both runtimes, plus workers and children).

Historical proof at commit `7ab2f662` (before Linux extension), SHA-256
(ordered filename, NUL, file bytes as in that commit's `run.mts`):
`33d0d660bb6748bb4f4d86d3c25dd64b42979f1402cd86559bcc66adcdbf6a10`.

Full observed outcomes and timestamps:
[Node report](../../../scripts/probes/plugin-confinement/evidence/macos-arm64-node.json),
[Python report](../../../scripts/probes/plugin-confinement/evidence/macos-arm64-python.json).
Reproduction, boundary details and limitations are in the
[probe README](../../../scripts/probes/plugin-confinement/README.md).

| Case | Workspace read/list | Workspace write | Shell/exec | Private read/write |
|---|---|---|---|---|
| Unrestricted positive control | allowed | allowed | allowed | allowed |
| No capabilities | denied | denied | denied | denied |
| Read only | allowed | denied | denied | denied |
| Write without read | denied | allowed | denied | denied |
| Shell without read | denied | denied | allowed | denied |
| Read/write without shell | allowed | allowed | denied | denied |
| Read/shell without write | allowed | denied | allowed | denied |
| Write/shell without read | denied | allowed | allowed | denied |
| Read/write/shell | allowed | allowed | allowed | denied |

The same read and shell decisions were observed inside worker threads. Declared
interpreter and native `/bin/cat` children inherited read restrictions. Both
runtimes retained readable/writable private scratch, rejected private symlink and
hard-link escape, protected nested owner state and workspace-parent rename, lost
the inherited private descriptor, and could not relax the installed policy.

## Review findings and remaining gate

The proof uses a native bootstrap that installs Seatbelt before fixture code;
it does not intercept language filesystem/process APIs. Python starts with
isolated configuration and trusted standard-library imports. Node closes owner
descriptors before runtime startup and loads a native addon before fixture code.
The selected runtime and headers are explicitly trusted proof prerequisites.

Review found that Node worker startup with no workspace read requires scratch
cwd. It also found that checking thread startup alone was insufficient: the
matrix now performs native reads and shell attempts from the worker. Descriptor
assertions compare fixture device/inode identity because runtime-owned fd 9 can
legitimately exist after startup. Unrestricted controls demonstrate that the
denied fixture operations actually work without the installed policy.

Remaining gates include the deprecated/custom Seatbelt API support decision,
Linux native execution/read proof including existing runtime threads, explicit
Windows availability limits, other runtime decisions, immutable package/runtime
admission, pre-existing hard-link aliases/races, protected-root composition,
network/secret intersection, packaged launcher integration, and actual plugin
protocol execution/revocation/crash acceptance. No production capability promise
is changed by these passing probes. macOS Intel and CI results are not implied
by local arm64 results; inspect the dedicated workflow artifacts.

Strict TypeScript checking of the driver passed. The runtime C files compiled
under `-Wall -Wextra -Werror` (the known deprecated API warning is explicitly
excluded from error promotion). Rust/application checks are still required;
this proof adds no production Rust changes and does not convert the recorded
out-of-space baseline build failure into a pass.

## Linux extension under validation

The Linux prototype now installs Landlock before runtime exec/thread creation,
then a synchronized execution filter before fixture code, inside bubblewrap's
private mount/PID/network boundary. Protected subtrees are hidden with read-only
mode-000 tmpfs mounts. The same capability matrix adds direct native fork checks.
See the probe README for the exact mechanisms, prerequisites and limits.

Local macOS arm64 Python/Node reruns of the shared sources passed all nine cases,
including direct native fork. The first native Linux Python results are recorded
below; Node/Linux and production integration still require acceptance. Historical committed reports above remain
bound to commit `7ab2f662`, rather than being silently reattributed to new sources.

The initial CI run at `7ab2f662` passed Node on macOS arm64 and Intel, while both
Python jobs failed: framework Python's `libpython` symlink resolves to the
framework binary outside `LIBDIR`; dyld refused that missing read root in
`shell-without-read`. The policy now grants the exact resolved trusted library
file, rather than broadening all of `/Library`. The rerun must establish this
fix on both macOS architectures; local Anaconda results do not prove it.

## First Linux native results

[Native CI run 40545245707](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40545245707),
commit `70c059d3`, passed Python on both Linux architectures:

| Architecture | Kernel | CPython | Capability cases |
|---|---|---|---|
| x64 | 6.17.0-1022-azure | 3.13.16 | 9 passed |
| arm64 | 7.0.0-1017-azure | 3.13.17 | 9 passed |

Full source-bound observations:
[x86_64](../../../scripts/probes/plugin-confinement/evidence/linux-x64-python.json),
[arm64](../../../scripts/probes/plugin-confinement/evidence/linux-arm64-python.json).
These are native process tests, not cross-compilation. All expected positive and
denial assertions passed, including writes without reads, worker reads/spawns,
raw fork, direct re-exec, protected mount masks and inherited child restrictions.

Node on both Linux architectures failed during initial OpenSSL startup because
`/etc/ssl/openssl.cnf` was not among the read roots. The proof now grants that
exact system configuration file when present, preserving its configuration
rather than disabling OpenSSL settings or granting all of `/etc/ssl`. A Node
Linux rerun is required. This failure does not contradict the independently
observed Python results and does not establish Node/Linux support.

## Linux initial-exec mechanism under test

At `1226aeb5`, [run 40546015851](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40546015851)
passed all nine capability cases for both Node and Python on both Linux
architectures. The exact OpenSSL configuration grant resolved Node startup.
These results concern the earlier Landlock + late TSYNC bootstrap mechanism.

The next experiment installs the execution filter before the target's first
instruction using single-use initial-exec notification admission with policy
replacement blocked. It adds an untrusted native executable and a constructor
read assertion. This tests a route that does not require runtime/native-plugin
cooperation, while retaining positive startup, threads, native children and
exec/fork denial controls. See the probe README for ordering and caveats. No
passing result is claimed for this new mechanism until native CI executes it.

## Verified experimental matrix at `59cae487`

[Native CI run 40548917492](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40548917492)
completed successfully with all ten jobs. Each job compiled native proof code
and asserted one unrestricted control plus all eight read/write/shell cases:

| Platform | Architecture | Target | Version | Cases |
|---|---|---|---|---|
| darwin | x64 | node | 22.23.2 | 9 passed |
| darwin | x64 | python | 3.13.15 | 9 passed |
| darwin | arm64 | node | 22.23.2 | 9 passed |
| darwin | arm64 | python | 3.13.15 | 9 passed |
| linux | arm64 | native | fixture-v1 | 9 passed |
| linux | arm64 | node | 22.23.3 | 9 passed |
| linux | arm64 | python | 3.13.16 | 9 passed |
| linux | x64 | native | fixture-v1 | 9 passed |
| linux | x64 | node | 22.23.4 | 9 passed |
| linux | x64 | python | 3.13.16 | 9 passed |

Full reports, including source lists, actual observations and timestamps, are
committed under [evidence/59cae487](../../../scripts/probes/plugin-confinement/evidence/59cae487).
All ten reports have source digest `a833c04e587d7ff74aa579f7193509892d20e3deb3209746b1b41971eccb91de`.
This is 90 capability/control cases and 180 primary process launches, plus
workers, raw forks and native/interpreter children. The native ELF constructor
read/fork assertions passed, as did the Linux filter-replacement denial tests
with unrestricted controls that could install a filter. macOS framework Python
startup also passed on both architectures after the exact library-file grant.

The earlier Linux Node reports at `1226aeb5` are separately retained as
[x86_64](../../../scripts/probes/plugin-confinement/evidence/linux-x64-node.json)
and [arm64](../../../scripts/probes/plugin-confinement/evidence/linux-arm64-node.json).
They prove the earlier TSYNC mechanism, not the later one-shot mechanism.

This closes the experimental runs named in this table, not task 2.1 or production
security acceptance. Bun/Deno and macOS native combinations still need proof
or an explicit reviewed support decision. Production package/runtime admission,
read-root and protected-state composition, environment/descriptor and network/
secret intersections, packaged launcher lifecycle and actual plugin acceptance
remain implementation work. No unsupported combination may silently inherit
weaker fs.read/shell behavior.

## Remaining runtimes and native macOS boundary

The follow-up prototype adds explicit Bun/Deno executable selection, private
startup configuration and caches, and native macOS filesystem cases when shell
is declared. Local macOS arm64 matrices passed for Bun 1.4.4 and Deno 2.9.8. The
Bun control demonstrated automatic project preload/dotenv loading, which the
hardened launch disables before native policy admission. Deno command observation
uses typed native permission errors rather than treating its empty spawnSync
result as proof of denial. Neither adapter supplies filesystem/process security
through API interception.

Local native macOS testing passed all four shell-enabled filesystem combinations
plus the unrestricted control, including constructor reads/forks, write without
read, protected paths and inherited restrictions. Four no-shell combinations
remain explicitly unsupported in the report. The separate native-exec experiment
verified that denying exec prevents the initial launch, while permitting that
exact file also permits its later self-exec. This is evidence against those two
strategies, not a claim that every other mechanism is impossible. Native CI on
both architectures must verify the new combinations and negative experiments.

The original [full CI run](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40539570936)
at `7ab2f662` also finished with all 16 jobs successful: both Rust jobs, Windows
build/lint, all platform builds, SDK, dashboard and OpenSpec. Production Rust/SDK
sources were unchanged from the main baseline. This resolves a remote baseline
check; the earlier local disk-exhausted compilation is still not a local pass.

## Runtime extension CI observations at `3c8efc33`

[Run 40553987420](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40553987420)
passed all Deno platform/architecture combinations, both macOS Bun matrices,
both shell-enabled macOS native subsets, both macOS native exec-gap experiments,
and the existing Node/Python/Linux-native matrices. Bun 1.4.4 aborted at Linux
runtime initialization under the initial-exec candidate on both architectures;
no Bun/Linux pass is implied. This is an observed experimental failure, not a
claim about the project's prior baseline or a filed upstream bug.

The follow-up adds actual stdin/stdout JSON-RPC tool dispatch to every matrix,
retains a diagnostic strace attempt without accepting a traced result, and tests
Bun/Linux's native TSYNC bootstrap independently. All five macOS runtime
fixtures passed local tool round trips (the native executable subset still has
four explicit unsupported no-shell cases). New native CI results are pending.

## Bun/Linux diagnostic follow-up

The retained `728db0f4` diagnostics show that both initial-exec and TSYNC
candidates abort after denied private process metadata reads, with
`/proc/self/cgroup` immediately preceding SIGABRT. The next candidate tests
read-only metadata inside mandatory private PID/proc isolation, masks PID 1 and
`/proc/sys`, marks the trusted initial-exec supervisor non-dumpable, and asserts
that `/proc/self/root` cannot bypass workspace/private reads. No successful
Bun/Linux result is claimed yet. Existing confinement assertions remain required;
there is no ordinary-sandbox fallback or permission-based widening.

The `ace76b81` Bun/Linux candidate progressed past the prior runtime abort on
both architectures, but then failed module loading: opening the bootstrap file
succeeded while opening its source directory returned EACCES. The follow-up
stages the trusted bootstrap and native addon in the already admitted read-only
fixture package for both inspect and exec launches. It does not grant the source
repository or a temporary ancestor directory. Bun/Linux acceptance remains
pending the untraced native matrix and all existing positive/negative controls.

## Bun/Linux initial-exec and TSYNC evidence at `d5bee987`

[Run 40574108971](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40574108971)
passed all ten Linux runtime/architecture jobs. Both Bun 1.4.4 architectures
passed the untraced initial-exec candidate and the independent TSYNC matrix:

- [x86_64 initial-exec](../../../scripts/probes/plugin-confinement/evidence/d5bee987/linux-x64-bun-initial.json)
- [arm64 initial-exec](../../../scripts/probes/plugin-confinement/evidence/d5bee987/linux-arm64-bun-initial.json)
- [x86_64 TSYNC](../../../scripts/probes/plugin-confinement/evidence/d5bee987/linux-x64-bun-tsync.json)
- [arm64 TSYNC](../../../scripts/probes/plugin-confinement/evidence/d5bee987/linux-arm64-bun-tsync.json)

All four reports bind source digest
`503c658fb27147ffa703a8519d092f121eb08e801d8d1e01c046230269d5a311`.
Each contains nine capability/control rows with actual stdio tool dispatch and
separate self-exec launches. The initial-exec rows also assert immutable filter
replacement denial with a working unrestricted control. Private proc-root aliases
remain unable to bypass workspace/private reads, and PID 1 remains masked.

The workflow now requires the generic initial-exec mechanism for Bun as for the
other Linux runtimes; the successful candidate is no longer an optional
comparison. The macOS jobs on this source revision remain pending. These results
resolve the earlier experimental Bun/Linux startup failure; they do not prove
production confinement, network/secret intersections, immutable package
admission or the proposed macOS native no-shell availability revision.

## macOS platform-subject exec predicate experiment

The [local arm64 report](../../../scripts/probes/plugin-confinement/evidence/platform-attribute-local/macos-arm64-native-exec-gap.json)
binds source digest `37b114b07de092ebe63ae6c5137ad620df3f902c660dcff3c4145c70d57e384e`.
The extended `macos_native_exec.mts` retains both existing negative strategies
and adds a platform-subject filter intersected with an exact canonical path:

- The system sandbox launcher starts the custom fixture; the fixture cannot
  re-exec itself under that policy.
- A platform executable (`/bin/bash`) starts and re-execs itself successfully
  under the equivalent path-specific policy. Thus the subject exception is
  not intrinsically one-shot for an arbitrary payload.

The first result is a positive distinction, not a complete mechanism. No
production target-admission or immutable-staging rule has yet been proved to
exclude that exception from untrusted execution. Broad reads deliberately
isolate the exec experiment and are not filesystem-capability evidence. A
separate copied-system-binary experiment was killed by the local OS; that
termination is not a reliable admission boundary and is not counted as proof.
The existing native-gap CI jobs now require both new assertions on macOS arm64
and Intel. Their results for this extension are pending. The support revision
remains proposed; no production integration or task completion is implied.


## Staged native macOS candidate and descriptor regression

The new local arm64 candidate passes all eight `fs.read`/`fs.write`/`shell`
combinations and an unrestricted positive control. The
[full report](../../../scripts/probes/plugin-confinement/evidence/staged-native-local/macos-arm64-native.json)
binds source digest `955059e9d5d18d5865296adc4b80da4af8fecaee18910ef1f596d849524d7437`.
The [default comparison](../../../scripts/probes/plugin-confinement/evidence/staged-native-local/macos-arm64-native-unstaged.json)
retains five executed rows and four explicitly unsupported combinations.

`macos_stage.mts` copies a controlled native fixture into the private test root,
re-signs it ad-hoc with a fresh identifier, and verifies that identifier and the
absence of an Apple anchor across all architecture slices. The fresh identifier
also changes the CodeDirectory hash. The report records input/image SHA-256,
identifier and architecture list. The trusted launcher replaces itself with the
system sandbox program, which installs the profile before untrusted instructions.
A platform-subject predicate intersected with the canonical staged path admits
the initial launch. The re-signed fixture cannot retain that subject authority.

Separate launches assert constructor-time self-exec and direct `posix_spawn`
behavior as well as main-time exec/spawn. The constructor queries platform
status before and after attempting `CS_OPS_SET_STATUS`; both queries succeed
and report no platform bit. A successful setter return is not treated as
authority: [XNU masks the accepted status bits](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_proc.c),
and this local probe checks actual status afterward. This does not prove every
other kernel surface or OS release safe.

The [counterexample report](../../../scripts/probes/plugin-confinement/evidence/staged-native-local/macos-arm64-native-exec-gap.json)
binds digest `392ab27738098b637073e7fb5843c7143b118f6dda8c8c521b21f3882a29b18c`.
It preserves the no-exec/literal/platform counterexamples, then stages `/bin/bash`:
initial execution succeeds, unrestricted self-exec succeeds, restricted self-exec
is denied. This removes the previously demonstrated platform-payload exception
for that staged fixture; it is not proof of arbitrary package admission.

A new controlled descriptor regression exposed a leak in the test launcher's
`getdtablesize()` loop: an already-open descriptor 500 remained inherited after
the subprocess reduced its soft limit to 64. The corrected launcher uses
`POSIX_SPAWN_SETEXEC | POSIX_SPAWN_CLOEXEC_DEFAULT` with explicit stdio inheritance.
The control observes descriptor 500; the launcher target cannot observe it.
[Apple's flag documentation](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man3/posix_spawnattr_setflags.3)
describes process replacement and explicit descriptor inheritance. An attempted
`closefrom` implementation failed to compile in the local SDK and is not used.

The prior `30c61cf2` [CI run](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40635690482)
now passed all 22 jobs, including both platform-predicate counterexamples and
the native shell-enabled subset. Those jobs predate this candidate. The workflow
now requires the staged nine-row native matrix and descriptor regression on
macOS arm64 and Intel; exact-source results for this change remain pending.

Review scope: executable probes only. No production launcher or availability
contract changes. Mode 0500 alone does not prove immutable ownership; pre-existing
writable aliases/hard links, transformed-image integrity, package/entrypoint
admission, entitlement/signature compatibility, helper packaging without
developer tools, protected-root races, network/secret intersections and real
owner/tool execution still require implementation and acceptance. Task 2.1
and the explicit support/design review remain open.

Scoped code/security review of this increment inspected the three TypeScript
probe files, three modified C fixtures/launcher, new descriptor fixture, workflow
and evidence bindings. No production architecture changes were introduced.
Trusted tool invocations use argument arrays and a cleared environment; the
stager uses exclusive copy creation and verifies all slices. The known staging
limitations above prevent treating this helper as a production admission API.
No additional must-fix finding remains within this experimental scope.

Verification: both local native modes and the exec/descriptor experiment passed;
all three TypeScript syntax checks and `git diff --check` passed. Pinned OpenSpec
1.13.2 strict validation passed all 106 entries; spec lint reported zero errors
and 25 warnings (no origin assigned). Local Linux/Intel and production owner
acceptance were not run for this increment; CI and integration remain required.


## Staged native CI at `ba2d597e`: positive matrices, unresolved system payload

[Run 40649398655](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40649398655)
completed with 21 successful jobs and one failure. All 20 runtime jobs passed,
including all nine staged-native rows on macOS
[arm64](../../../scripts/probes/plugin-confinement/evidence/ba2d597e/macos-arm64-native.json)
and [Intel](../../../scripts/probes/plugin-confinement/evidence/ba2d597e/macos-x64-native.json).
Both native reports independently match executable source digest
`955059e9d5d18d5865296adc4b80da4af8fecaee18910ef1f596d849524d7437`.
The constructor-time exec/spawn and platform-status assertions therefore have
both-architecture native fixture evidence.

The separate [Intel counterexample/descriptor report](../../../scripts/probes/plugin-confinement/evidence/ba2d597e/macos-x64-native-exec-gap.json)
passed with source digest
`392ab27738098b637073e7fb5843c7143b118f6dda8c8c521b21f3882a29b18c`.
The equivalent arm64 job failed before the required staged `/bin/bash` positive
control returned successfully. Its [retained log](../../../scripts/probes/plugin-confinement/evidence/ba2d597e/macos-arm64-native-exec-gap-failure.log)
reports null exit status at that control; this version did not print the signal.
No successful arm64 system-payload or descriptor assertion from that job is
claimed. Local arm64 success on a different kernel does not override this failure.

The follow-up adds termination status/signal/output diagnostics and staging/kernel
metadata at the positive-control boundary. Assertions remain required: an OS
termination is not counted as successful initial admission or safe confinement.
The cause and portable signature/architecture compatibility remain unresolved,
and production support/design approval is still pending. No unrestricted retry
or architecture-slice removal is introduced.


## Package aliases and staged-system termination follow-up

The `6a819a19` arm64 [retained diagnostic log](../../../scripts/probes/plugin-confinement/evidence/6a819a19/macos-arm64-native-exec-gap-failure.log)
identifies SIGKILL with no output from the staged `/bin/bash` positive control
on Darwin 24.6.0. Its input contains x86_64 and arm64e slices and passed all-slice
signature verification. Local success was on Darwin 25.6.0. This proves a host
difference and failed positive admission, not its cause or safe confinement.
The next experiment independently records direct/unconfined and sandboxed
positive controls to separate image compatibility from the test policy. Neither
control can satisfy the other, and neither is a production fallback.

New local arm64 reports bind these exact source digests:

- [Nine-row native matrix](../../../scripts/probes/plugin-confinement/evidence/package-alias-local/macos-arm64-native.json)
  and [default subset](../../../scripts/probes/plugin-confinement/evidence/package-alias-local/macos-arm64-native-unstaged.json):
  `c5cf1f1ca293b88722d3b7cbfe159bf237688ae4f796831884f675fea686c5b1`.
- [Exec/staging controls](../../../scripts/probes/plugin-confinement/evidence/package-alias-local/macos-arm64-native-exec-gap.json):
  `778d239d4a6e693ab4a664c08ce624d6818c8dc6f61a8b193a15fa9869f338f0`.

The native fixture now tries linking readable package data into writable scratch
and writing through that alias. The unrestricted control succeeds and changes
the dummy package file. Every confined local case rejects the link/write, and
the driver independently reads back the original package bytes after inspection.
This checks a read-root alias attack distinct from the unreadable-private-file
link control; it does not establish complete package admission.

A separate staging test creates two hard links to a controlled native input,
stages it, then modifies the input through its pre-existing writable alias. The
staged inode has one link, is distinct from the input, keeps its recorded SHA-256
and still executes the positive control. This demonstrates copy separation for
these controlled fixtures; concurrent input replacement/copy races and production
parent/image ownership remain unproved.

The staged-system restricted control now requires a normal nonzero exit and
a native permission error after the initial marker; termination by signal is not
accepted as a denied exec. Local Bash reports exit 1 and Operation not permitted;
an initial assertion expecting 126 was corrected to the required behavioral
contract. Direct and sandbox positive controls both execute initial and self-exec
markers locally. The high-descriptor regression still passes.

All three fresh reports were independently rebound to the current source list
and SHA-256 before copying. Local native modes, staging/exec experiment, syntax
and diff checks passed. This increment changes only executable tests and evidence.
Native CI for these new assertions remains required, the Darwin 24.6.0 system-image
issue remains unresolved, and the explicit production support/design gate stays
open. No runtime availability or production launcher change is made.


## Original-signature target candidate and detached-signature control

[Run 40674537036](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40674537036)
at `74ae15f1` completed with 21 successful jobs and one failure. All 20 runtime
matrices and the Intel exec-gap job passed. The retained
[arm64 log](../../../scripts/probes/plugin-confinement/evidence/74ae15f1/macos-arm64-native-exec-gap-failure.log)
now distinguishes both positive controls: direct staged `/bin/bash` terminated
with SIGSEGV, sandboxed execution with SIGKILL, neither producing an initial
marker. This establishes failed positive admission on Darwin 24.6.0, not its cause
or a successful policy denial. The staged-image experiment and assertions remain.

A separate candidate signs the trusted native launcher with a fresh identifier,
then intersects `(signing-identifier ...)` with the exact target path in the
Seatbelt process-exec rule. The target image is not re-signed. Apple's
[WebKit WebProcess profile](https://github.com/WebKit/WebKit/blob/main/Source/WebKit/WebProcess/com.apple.WebProcess.sb.in)
uses the signing-identifier filter; the exec behavior below is experimental
native evidence rather than an inference from that profile.

On local Darwin 25.6.0 arm64, the new
[native matrix](../../../scripts/probes/plugin-confinement/evidence/signing-identity-local/macos-arm64-native.json)
passed all nine rows, including constructor-time exec/spawn, reads, writes,
threads, fork, package-alias protection, and JSON-RPC tool execution. Its source
SHA-256 is `7be22e5a112b64c55498cda8a68cfab46182e0d6f5e93682733437b2d219e63a`.
The separate
[exec controls](../../../scripts/probes/plugin-confinement/evidence/signing-identity-local/macos-arm64-exec.json)
bind source SHA-256 `7a06af58c27b8b134dd4833f6947b1a27623c4c3ef34c74542528d05ae284723`.
Both native and original `/bin/bash` controls execute/self-execute unrestricted,
start normally when confined, and then fail self-exec with a native permission
error. Bash's bytes remain unchanged. High-descriptor closure also passes.

A copied payload deliberately signed with the bootstrap identifier **can
self-exec** under the candidate. This is an observed bypass unless admission
proves target/helper identity separation across every usable architecture and
keeps both images immutable. The fresh identifier alone is not a complete
production admission contract.

A second controlled payload supplies F_ADDSIGS with an alternate valid ad-hoc
signature for identical code, using only a read-only self descriptor. The
alternate signature bears the bootstrap identifier; prefix equality independently
checks unchanged code bytes. The call returns success in both controls, but the
confined payload still receives EPERM on self-exec, and the source image hash
remains unchanged. This is a behavioral check of one mutation interface, not a
claim that every successful fcntl changed identity or that all mutation routes
are covered. Apple's [fcntl implementation](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_descrip.c)
can return an existing cached signature, while
[ubc_cs_blob_add](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/ubc_subr.c)
rejects different overlapping signature blobs; neither source inspection alone
proves behavior on every supported kernel.

The new candidate has separate macOS 15 arm64/Intel CI jobs requiring both
matrices and exec controls. Those current-source jobs have not yet run. Immutable
package/helper admission, kernel-interface coverage, distribution compatibility,
production owner integration and the explicit design/support review gate stay
open. No production Rust launch path, capability enforcement, or availability
contract changes in this increment.


Increment verification also passed the current-source default native comparison
(5 rows, 4 explicitly unsupported cases) and staged native matrix (9 rows),
TypeScript syntax, all four workflow shell snippets, YAML parsing, diff checks,
strict OpenSpec (106 items on this branch), and spec lint (0 errors, 25 warnings).
The code-review pass covers the changed probe, controlled C fixture, workflow,
reports and documentation. This is test infrastructure, with no new production
API or architecture boundary. Commands use argument arrays, a sanitized environment,
bounded dummy data and a private temporary directory; reports bind exact sources.
The collision/admission and cross-platform coverage gaps above remain explicit;
local passes do not supply the pending macOS 15 or production acceptance.
