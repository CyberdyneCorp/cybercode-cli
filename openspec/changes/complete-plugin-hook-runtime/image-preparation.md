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

## Supervised owner-side preparation

`cyber-sandbox::plugin_image::prepare` now invokes the trusted worker with a
cleared environment, null stdin, bounded stdout (32 KiB) and stderr (8 KiB), and a
30-second deadline. Shared request validation performs no source IO. Invalid
frames and source/destination overlap are refused before spawning. Temporary
parents are created with mode 0700 explicitly; `tempfile`'s default directory
permissions do not provide that guarantee. The owner-side integration test
failed with worker refusal before this explicit mode was applied, then passed.

A supervisor owns the child and staging parent. Dropping the awaiting future
closes its reply receiver; the supervisor kills and reaps the exact trusted
no-descendant worker before cleanup. Timeout, malformed response, refusal and
stream overflow use the same settle-before-cleanup path. Captured helper payloads
are not included in errors. Uninterruptible OS IO can delay reaping and cleanup.
If the runtime drops the supervisor or reaping itself fails, its guard retains
the protected directory instead of deleting a possible live writer's namespace.
This exceptional retention is not normal cancellation cleanup or successful
preparation, and requires later owner startup housekeeping after authority to
remove stale generations is established.

A successful response must match the request destination, approved digest and
bounds. The owner opens only directories, with no-follow on the image child,
and compares the returned native directory identity. The resulting `Package`
retains the directory handle and private parent until drop; dropping a response
whose caller has already left also releases that settled package. Reading or
closing private regular files in the owner remains prohibited by the SQLite
lock invariant. The actual helper/SQLite regression checks locks before and after
the owner wrapper and after dropping its directory handles.

Six owner tests cover invalid requests before staging, live-worker cancellation and process disappearance,
timeout, malformed/nonzero/flooding workers, forged identity and the dropped
supervisor guard. Real helper integration covers positive copying, a pinned
result and cleanup on Package drop. Windows explicitly refuses the owner API
without creating staging. The focused five-platform workflow includes these
tests. Local macOS tests do not supply hosted platform acceptance.

The caller must still supply a trusted immutable helper binding and protect the
staging base and its ancestors from every admitted untrusted writer. This wrapper
is not yet wired to plugin launch. Plugin generation/session/location ownership,
complete private roots/aliases, native runtime compatibility and first-instruction
OS enforcement remain open; retaining a directory handle alone cannot authorize
execution. Task 2.2 and all later native acceptance tasks stay open.

Local verification of the supervised wrapper passed 26 core plugin tests, six
owner tests and four real helper integration tests (plus the two explicitly
invoked SQLite/digest subprocess fixture entrypoints). Scoped library/helper and
sandbox-test Clippy passed with warnings denied. Formatting, diff, workflow
YAML/shell, Windows test-profile parsing, all 107 strict OpenSpec items and spec
lint (0 errors, 25 warnings) passed. The scoped review additionally checked
receiver-drop cancellation, reaping before cleanup, runtime-shutdown retention,
bounded error handling and directory-only identity verification. These results
do not complete private-root provenance, generation admission, native launcher
integration or hosted OS acceptance.

## macOS workflow CI fixture correction

At `47453b14`, 23 checks passed and the broad macOS Rust job failed
[solve settlement](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40807803387/job/123230844927).
Its native fixture returned from the parent immediately after `fork`, before the
child necessarily called `setsid`. The host could terminate that still-attached
child and correctly report a settled leader exit, while the test expected an
unknown detached descendant. Its four-second child sleep also made the expected
open-pipe state depend on scheduling.

The focused unchanged test passed locally on main `a0199b34` and on the PR
revision. Those passes do not reproduce or dismiss the observed CI failure.
The fixture now checks `pipe`, `fork` and `setsid`, and requires a readiness
receipt before its parent exits. A short adversarial child delay exercises that
parent/child ordering; synchronization depends on the receipt. The descendant
keeps its output pipes open until the test owner releases it after the settlement
assertions. A bounded fallback and drop guard provide failure cleanup. The test
also requires `settled=false` and unchanged source, so source mutation cannot
stand in for unknown native settlement. Linux retains its namespace-wide
settlement expectations; Windows does not run this Unix-only fixture.

The corrected focused test passes locally on macOS arm64. The same linker
unwind-table warning was observed during both unchanged-main and PR test builds;
it is not a failed test. Hosted CI must rerun the updated fixture before the PR
is accepted. This changes test synchronization, not production settlement policy
or the native plugin capability scope.
