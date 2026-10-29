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
