# Private-root alias inventory

This implements a metadata-only admission component within task 2.2, following
[the observed pre-existing alias bypass](native-proof.md#pre-existing-aliases-require-a-separate-admission-boundary).
An OS denial for the private pathname cannot prevent reading the same inode
through a hard link that already exists in a permitted package or workspace.

`plugin_image::private_roots::Inventory` captures explicitly supplied absolute
roots using only directory handles, directory enumeration and non-following
metadata. Regular private files must have exactly one link, including when two
links would both be inside protected roots. Symlinks and non-file/non-directory
objects refuse capture. Every path component used to open directories rejects
symlinks; replacing an expected directory with a regular file never opens that
file. No private regular file is independently opened or closed in the owner.
This matters for SQLite's process-wide POSIX record locks.

Capture records native object identity, type, link count, length, ordinary/native
mode, owner, modification and change time. It checks directory handles against
the enumerated entry before descent, and checks each entry again afterward.
Recheck captures fresh metadata and compares it with the retained records and
parent directory identities. Replacement, new/deleted entries, permission changes,
content metadata changes and new aliases refuse the old incarnation. Captured
metadata contains paths and native identities; callers must not log it as public
payload. Errors are static and do not expose private contents or pathnames.

Limits are 64 supplied roots, 8,192 unique entries, 32 nested directory levels,
4,096 native bytes per path and 4 MiB of retained path bytes. Records have fixed
bounded fields in addition to those path bytes. At most one parent descriptor per
supplied root is retained; traversal uses at most one directory per depth. The
inventory does not allocate memory proportional to file contents. Native metadata
IO can still block; callers must own cancellation/settlement rather than infer a
wall-clock deadline from entry limits. Windows returns
an explicit unavailable result; it does not silently skip alias checks.

## Preconditions and integration still required

The caller must supply **every** private root, synchronize legitimate owner writes,
and exclude all untrusted writers/renames/mount changes during capture, recheck
and launch. This API does not enumerate the system's private roots, freeze file
metadata, establish immutable ancestors or close the race after a recheck. It
cannot replace native protected-root masks or the generation-bound launch commit.
Sockets, FIFOs and symlinks require a separately reviewed object policy before a
caller can claim to admit an owner namespace containing them; refusal here is
not evidence of complete production support.

No production host currently uses this inventory. Namespace provisioning,
protected/alias-root composition, helper/runtime admission, native launch wiring,
network/secret intersections and the explicit support/design review remain open.
The primitive is intentionally not represented as a sealed image or launch permit.
Task 2.2 and the full plugin/hook acceptance checklist remain incomplete.

## Verification

On local macOS arm64, 22 owner/authority/inventory tests and five actual-helper
integration tests pass. Two ignored subprocess fixture entrypoints are invoked
by the SQLite regression. Seven inventory tests cover nested and file roots,
metadata changes, internal/external hard links before and after capture,
symlinked ancestors, root replacement, special objects, request/depth bounds,
and real-directory entry/path-byte limits. The exact entry limit is accepted;
one extra entry refuses. A separate long-path fixture exceeds the 4 MiB path
budget before the entry limit.

The independent SQLite connection still observes the owner's exclusive lock
after inventory capture, recheck and drop. Treating the locked database as an
expected directory ancestor also refuses without releasing the lock. Three
behavioral controls execute the named regressions: removing link-count refusal
allows unsafe capture; removing snapshot comparison misses changed data; adding
an owner-side private regular-file open/close releases the SQLite lock. Each
control fails its intended assertion, and restored positive suites pass.

Scoped Sandbox all-target Clippy passes with Rust warnings denied. The native C
compiler emits two deprecated Seatbelt SDK-call warnings; no origin is assigned
here. Formatting/diff checks pass, the package workflow YAML and all four Bash
blocks parse, strict OpenSpec validation passes 107 items, and spec lint reports
zero errors and 25 warnings. The exact new CI control block also executes locally
and restores the passing 22-test and five-test suites. Hosted platform results
remain pending.

The code/security review covered every changed source, test, workflow and
implementation note. Directory opens use the existing capability-based no-follow
primitive; regular files have only metadata operations. Requests are validated
before root-path cloning and traversal, and retained paths/entries and directory
handle counts are bounded. Error strings are static. Tests exercise both safe
capture and unsafe-alias refusal, rather than counting refusal alone as admission.
No blocking issue was found within this inventory scope. The review does not
certify complete private-root coverage, writer exclusion, immutable namespace
ownership, cancellation of blocking native IO or production launch admission. The focused package
workflow already runs `cargo test -p cyber-sandbox --lib plugin_image` and the real
`plugin_copy` integration suite, including the SQLite independent lock probe.
