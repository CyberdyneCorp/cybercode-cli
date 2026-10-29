# Content-bound package preparation worker

This implements the copy foundation of task 2.2. It does not close the native
proof/design gate or admit plugin execution. The approved confinement design
requires an immutable plugin package; the original-signature experiments also
require copies to preserve executable bytes and signatures. The worker prepares
those bytes without running package code or changing ordinary sandbox behavior.

## Protocol and ownership

The internal `cyber-sandbox-exec --plugin-copy` mode accepts a strict JSON request
with absolute source/destination paths and the approved `sha256:` tree digest.
Its frame is at most 64 KiB; its response is at most 32 KiB. Errors are static,
without source contents, payloads or filesystem paths. Success returns the digest,
file/byte counts and ephemeral Unix destination directory identity.

Source reads happen in this short-lived trusted helper, never in the process
holding owner SQLite record locks. Opening then closing a raced source alias in
the owner can release those process-wide locks. An actual locked SQLite database
is copied by the helper while an independent connection verifies the exclusive
owner lock remains held. The same test has an explicit owner-IO negative control
that must fail with the lock-release assertion; it is followed by the positive
helper check. Ignored fixture functions are explicitly invoked as subprocesses.

The owner must supply a private staging parent, retain it until the helper exits,
protect it from plugin/other untrusted writes, then open and retain the returned
root beneath that parent and verify its identity. This API alone cannot establish
same-user namespace ownership or complete private-root provenance. Cancellation
must join/terminate the worker before deleting the staging parent; owner-side
lifetime and launch wiring remain to be implemented.

## Capture and bounds

Unix capture opens each source ancestor without following symlinks. Inventory
records directory identities and file identity, link count, size, modification
time and ordinary permission bits. Only regular files/directories are accepted;
top-level `.git` is excluded to match the existing trust digest. Every hard link
must be accounted for within the captured tree before copying. Unknown external
aliases are refused. Copies use new independent inodes, retaining ordinary
executable bits while stripping setuid/setgid bits.

Limits are 4,096 files, 8,192 total entries, 64 MiB of file bytes, 32 nested
levels and 4,096 UTF-8 bytes per path. Files stream through a 64 KiB buffer;
opens use no-follow, nonblocking and no-controlling-terminal flags. Each opened
file must match its captured state before and after reading; growth/truncation
is refused. A source replaced by a FIFO cannot block on open. The final source
path binding and inventory must still match. The actual copied bytes must match
the approved canonical tree digest, rather than trusting an earlier hash.

Output must be a new child of a private parent (no group/other permissions).
Failed captures remove their partial output only while its directory identity
still matches; existing destinations are never overwritten. Output parent/root
protection is an owner precondition, not something a path-only check proves.
Manifest, entrypoint and contribution consumers must subsequently resolve from
the verified copy, including rejecting references into excluded `.git` data;
using previously parsed mutable source metadata would break content binding.

Windows currently returns an explicit unavailable result before output creation.
This is a worker support boundary, not a claim of plugin confinement or completion
of the required Windows support decision. Windows helper refusal/diagnostic tests
are configured in both the focused workflow and the regular Windows test profile.

## Verification and remaining integration

Core tests cover canonical digest compatibility, empty directories, modes,
independent copies of package-internal hard links, external links, symlinks,
special files, root overlap, private-parent permissions, mismatched content,
existing output preservation, file/entry/byte/depth bounds and deterministic
file/directory/root/FIFO/symlink replacement after inventory. Helper integration
tests check real process framing/errors, copied bytes and native SQLite locks.
A controlled C executable is compiled and, on both Mac architectures, explicitly
signed before preparation. Its copied bytes must match, all-slice verification
must succeed on macOS, and the copied executable must emit its startup marker.
The output is never re-signed or repaired to obtain a pass.

Required next steps remain: typed owner generation/lifetime, complete private
root/ancestor composition, immutable package/runtime/helper provenance and
all-architecture identity checks, bounded helper cancellation/output handling,
OS policy integration before the first untrusted instruction and real App/server
acceptance. No task 2 checkbox is completed by a package-copy pass alone.

## Local review and results (2026-10-28)

On macOS arm64, all 26 core plugin tests pass (including eight new image tests).
The actual helper suite passes three tests; its two ignored fixture entrypoints
are invoked explicitly by the SQLite regression. Scoped Clippy with warnings
as errors, formatting, workflow YAML/shell checks, Windows test-profile parsing,
107 strict OpenSpec items and spec lint (0 errors, 25 warnings) pass.
The owner-process IO negative control fails with `owner lock was released`;
the subsequent helper suite passes. Hosted platform checks remain required;
no local Linux or Windows acceptance is claimed.

A separate compatibility diagnostic is retained in the same real helper test:

```sh
CYBER_PLUGIN_COPY_SYSTEM_IMAGE_CONTROL=1 cargo test --locked -p cyber-sandbox --test plugin_copy helper_copies_approved_bytes_and_refuses_changed_content_without_leaking_bytes -- --exact
```

Locally the original `/bin/sh` startup control succeeds. The worker copies it
byte-for-byte and its original signature verifies across all architectures, but
the relocated output terminates with signal 9 before output. This diagnostic
fails its startup assertion; it is not a confinement denial, a supported relocation
or a passing native compatibility result. It is not enabled in routine acceptance,
which requires startup of the controlled signed package executable. The eventual
runtime plan must establish whether a particular OS runtime can run from a copy
or needs a separately protected original-image binding; a content hash alone
cannot certify that choice. PR #55's original-system-image bootstrap proof does
not demonstrate relocating an OS platform binary.

The scoped code/security review covers request bounds, non-following traversal,
file identity and hard-link checks, exact digest binding, partial-output cleanup,
static diagnostics, process isolation and platform coverage. Preparation is a
filesystem adapter with explicit owner preconditions. Existing ordinary launch
modes are unchanged. Owner provenance/generation, full confinement and runtime
compatibility remain required integration work, rather than review exceptions
that authorize an unconfined launch.
