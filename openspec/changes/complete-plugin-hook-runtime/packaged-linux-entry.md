# Packaged Linux startup entry

This extends the packaged entry in PR #66, reusing the existing reviewed native
proof sources rather than replacing them. Task 2.2 remains open: these startup
primitives are not yet wired into production plugin admission or launch.

The helper-only `--plugin-native-exec` mode accepts an absolute profile, a
mandatory `--initial-shell=0` or `--initial-shell=1`, and an absolute target plus
arguments. There is no unfiltered startup mode. The Rust entry retains the same
256-argument / 64 KiB total and 4096-byte path limits, and never runs in the owner.

The C entry requires a single-threaded helper (attested through procfs), Landlock
ABI 3 or newer, a bounded regular single-link no-follow profile, and successful
`close_range(3, UINT_MAX, 0)`. Opening a FIFO profile cannot wait for a writer.
Landlock is installed before any target instruction. The shell=false path uses
the proof's one-shot seccomp listener: a nondumpable helper child authorizes the
trusted initial exec once; subsequent exec requests fail after listener close.
The filter rejects replacement, fork/process creation and escape syscalls while
allowing native threads. Shell=true permits descendants; they inherit Landlock
and the escape/filter-replacement restrictions.

The caller must still establish immutable helper/profile/runtime ownership,
cleared environment, private mount/PID/network namespaces, protected-subtree
masks, generation-bound final admission and runtime compatibility. Landlock
cannot subtract a deny under an allowed ancestor. These tests deliberately use
broad system-runtime roots to isolate the packaged kernel entry. They do not
prove protected-state masking under those roots, network/secret policy, Windows
support, all supported runtimes, or actual plugin-host integration. No support
reduction or completion of tasks 2.1–2.4 is implied.

## Verification

On local macOS arm64: all three native argument tests and all six existing packaged
native-entry integration tests passed; scoped sandbox all-target Clippy passed.
The SDK's two deprecated Seatbelt calls remain explicitly allowed C warnings.
Linux C compilation and execution cannot run on this macOS host (no local Linux
container runtime was found). At `6b349331`, native Linux x86_64 CI compiled the
packaged entry and its musl C object, passed both then-present argument tests,
and passed four of five integration tests. The positive shell/descendant fixture
failed because it opens `/dev/null` with O_RDWR but the test profile granted only
writes. The profile now grants read/write for that explicit bootstrap device;
project/private grants remain unchanged. Full current-source Linux x86_64/arm64
acceptance and the missing-cleanup control remain pending CI.

Linux CI runs four new packaged-entry regressions (plus the shared refusal test):

- Read and write independence, pre-main private-read/fork denial, threads,
  symlink/private aliases, scratch writes, filter replacement and protected-file
  content preservation.
- Identical-target self-exec denied/allowed by shell policy; positive descendants
  retain project access and private-read denial.
- A dummy inherited descriptor 500 survives an unconfined control despite a
  lowered soft limit of 64, but is closed through the packaged entry.
- Invalid/oversized/NUL/root profiles, FIFO profiles and omitted shell policy
  fail before target receipts.

CI also compiles the C entry with native musl on each Linux architecture. For
Debian's native musl compiler, `-idirafter` adds kernel UAPI directories after
libc's standard headers, following the [GCC include-search contract](https://gcc.gnu.org/onlinedocs/gcc/Directory-Options.html).
Full workspace GNU/musl release builds remain separate CI checks. A behavioral
control removes close_range, requires the named descriptor assertion to fail,
restores the exact source, and reruns the positive suite. Build provenance hashes
both native implementations, promoted headers, entry/test sources and fixtures;
test results remain in the associated job logs.

## Code/security review

The review covers the binary dispatch, bounded Rust FFI and ownership, C profile
reads, Landlock rules, seccomp architecture guard and listener transfer, and
source/build/test workflow. Domain APIs are unchanged; the platform adapter stays
private to the helper. No shell command strings or profile contents are echoed
by the new entry. The owner must not invoke this C entry in-process.

The former probe's optional filter and soft-limit descriptor loop are removed
from the packaged path. The original probes are preserved as separate evidence.
Remaining composed-admission and runtime gates above are required before this
entry can replace production plugin launch. This is an implementation review,
not an independent security audit or completed plugin acceptance.

## Packaged runtime matrix

The existing confinement driver now accepts an explicit `--packaged-helper`.
It copies the distributed binary into the private test root and invokes its
mandatory mode rather than compiling a substitute launcher. On macOS it gives
that helper copy a fresh signing identifier; targets keep their original
signatures. On Linux it requires the one-shot initial-exec mode and existing
private mount/PID/network namespace composition. Python runs its actual reviewed
executable directly with `-I -S`; it does not use the old embedding launcher.

The unconfined control executes the target directly. Native, Node, Python and
Bun must actually retain the inherited dummy descriptor; Deno reports it absent
even in this direct control, so that runtime case cannot prove helper cleanup.
The independent native high-descriptor test remains required for that proof. Confined cases require that descriptor to be absent,
plus all existing capability expectations. The report records input and executed
helper hashes (different after macOS signing), exact probe sources, runtime and
kernel versions and each of nine cases. The workflow's separate provenance also
hashes the native entry sources, matrix driver and workflow. This binds observed
results without treating build hashes alone as passing enforcement evidence.

Locally, the packaged matrix passed all nine capability combinations for native,
Node and `/usr/bin/python3` on macOS arm64, with zero unsupported cases. The
unchanged legacy invocation is separately checked to preserve historical proof
coverage. CI adds Node, Python 3.13, Bun, Deno and native on macOS/Linux arm64 and
x86_64, with an additional system Python run on each macOS host. Those hosted
current-source runtime outcomes remain pending; they cannot be inferred from the
three local runtimes or from earlier separately compiled probes.

At follow-up `afdf44fe`, both native Linux architectures passed all three argument
tests and all five packaged integration tests. Both also produced the exact
missing-cleanup assertion (descriptor present when it must be absent). The jobs
then failed because their log verifier assumed `rg` was installed. That verifier
now uses the already required Python standard library. A fully restored positive
rerun and the expanded packaged-runtime matrix still require current-source CI.

At `95848c1c`, Linux x86_64 completed all nine packaged runtime cases for Node,
Python and Bun. Deno then failed only the direct unconfined FD expectation:
it reported the descriptor absent. The driver now records that specific direct
control behavior, without treating it as evidence for native descriptor cleanup.
All confined cases still require absence; the separate native FD test and
missing-cleanup control remain mandatory. The matrix now collects all runtime
failures before exiting nonzero, so one runtime failure cannot hide later ones.

## Recorded Linux packaged acceptance at `758b86f8`

The focused [Linux x86_64 job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40870601578/job/123436350500)
and [Linux arm64 job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40870601578/job/123436351018)
both pass: native musl C compilation, all three argument tests, all five packaged
integration tests, all nine capability combinations for each of Node, Python,
Bun, Deno and native (45 cases per architecture), the exact missing-cleanup
assertion, and the restored positive integration suite. Windows' focused job
passes its explicit unavailable-mode behavior; this is not Windows confinement.

Both retained artifacts were inspected. Each of the five runtime reports has
nine rows and no unsupported cases; each input helper digest matches the
retained built-helper digest. Independently recalculated source manifests match
the checked-out PR sources. The retained checkout `1b54f95b` is GitHub's test
merge with parents `d90f4c16` and `758b86f8`, verified through the commit API.
These source/binary bindings accompany actual passing tests, rather than being
substituted for them. macOS hosted packaged matrix execution and the remaining
production admission/integration gates above remain outstanding.
