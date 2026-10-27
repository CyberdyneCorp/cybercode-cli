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

Exact proof-source SHA-256 (ordered filename, NUL, file bytes as in `run.mts`):
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
