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
