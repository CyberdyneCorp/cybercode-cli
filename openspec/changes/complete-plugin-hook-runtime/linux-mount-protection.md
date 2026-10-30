# Linux protected mount ancestry experiment

This continues native proof task 2.1 and the policy-composition work in task 2.2.
It does not activate production plugin confinement. The existing Landlock entry
requires a private mount/PID/network namespace and protected subtree masks.
[Landlock's kernel documentation](https://docs.kernel.org/userspace-api/landlock.html#layers-of-file-path-access-rights)
explains that a grant from a path ancestor can allow operations on descendants
within a policy layer; a child grant is not a subtractive denial.

The new experiment tests whether masking a private child or mounting an immutable
package read-only is sufficient to prevent moving its ancestor. Its explicit
unanchored writable control requires such parent/grandparent renames to succeed.
The candidate adds same-path bind mounts for every protected ancestor strictly
inside Location, in shallow-to-deep order, before mounting immutable objects and
masking private objects. Writable anchors must keep public sibling writes usable;
readonly Location uses readonly anchors. A protected ancestor should itself be a
busy mountpoint rather than relying on the child's read/write mode to protect it.

A separate character-device mask comparison records binding /dev/null over a
private file. The candidate uses an
owner-created, empty, mode-000 regular file on a readonly bind mount. Both views
must leave the owner's actual private bytes intact. This file is new owned
metadata; no private regular file is opened in an owner holding SQLite locks.

The driver runs system Python inside bubblewrap with cleared environment, dropped
capabilities, disabled nested user namespaces and private PID/network/IPC/UTS
namespaces. It exchanges a fixed JSON-RPC-shaped receipt, checks direct private
reads/writes, symlink reads, immutable writes, protected parent/grandparent renames,
ordinary public writes/renames, scratch writes and unchanged owner bytes. Paths
include spaces, quotes and a backslash; arguments never pass through a shell.
Reports bind the driver/workflow sources and checkout, record observed rows and
explicit pass/fail outcome, and are retained even if a later assertion fails.

Five cases are required: writable unanchored control, writable anchored candidate,
readonly unanchored/anchored cases, and the writable anchored character-mask
comparison. No failed bootstrap can count as a denial. The actor has a
15-second subprocess timeout and bounded output; each temporary fixture is owned
by the driver and removed after its actor settles.

## Current verification and scope

Local macOS checks pass TypeScript syntax, the actor's Python AST syntax, portable
ancestor ordering/sibling-prefix checks, workflow YAML and both Bash blocks, and
diff checks. Linux execution is unavailable locally. The new lightweight workflow
requires native Ubuntu x64 and arm64 receipts. Those mount-only receipts are now
recorded below; local checks alone did not establish them.

This is a mount primitive experiment, without Landlock or the packaged native
entry. It does not prove fs.read/shell/network/secret intersections, immutable
package/runtime/helper admission, pre-existing alias inventory, owner namespace
provisioning, generation lifetime or actual App/server enforcement. A successful
experiment must be followed by production Linux composition and real-helper
regressions. The approved support/design gates and all parent acceptance tasks
remain open.


## First native observations at `ee7268c6`

Run [40943429793](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40943429793)
failed both Linux jobs on the diagnostic's expected empty device read: both
observed a denial instead. The retained x64 report contains all five rows. Its
first four cases satisfy their expected mount assertions: all eight unanchored
writable ancestor renames succeed; all eight anchored writable attempts return
EBUSY; sibling/public writes and public renames remain usable; readonly cases
refuse workspace writes; regular-file masks refuse private reads/writes; all
owner bytes remain unchanged. This partial evidence does not turn a failed job
into acceptance.

The expected successful device write/empty read was an unproved hypothesis and
is not claimed as an observed bypass. The comparison now requires the observed
read/write denial and records file kind, mode and nodev mount flag to explain the
actual view without assigning a cause merely from EACCES. Candidate regular-file
requirements and the successful unanchored rename counterexample are unchanged.
Fresh complete native runs are required after this diagnostic correction.


## Verified mount-only receipts at `d4fcf3ee`

The corrected [x64 job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40943857344/job/123693935943)
and [arm64 job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40943857344/job/123693935579)
both pass all five cases. Both report Linux `7.0.0-1017-azure`, bubblewrap `0.9.0`
and system Python `3.12.3`. The exact passing reports are preserved at
[the evidence directory](../../../scripts/probes/plugin-native-entry/evidence/linux-mounts-d4fcf3ee/),
with the initial x64 failed report retained separately and explicitly marked failed.

Both source manifests independently recompute to
`fc98524c490a23bc18e373783cdea32b55ceb400dee2d22a9b78c3d3f32660ed`
from fetched checkout `d6afa580`. Its verified parents are `7261e888` and
`d4fcf3ee`. The retained initial failed report also matches its own exact source
checkout and is not counted as passing acceptance.

Every passing report retains positive actor startup and unchanged owner bytes.
Unanchored writable cases perform all eight protected ancestor moves. Anchored
writable cases refuse all eight with EBUSY while permitting all sibling writes,
public writes/renames and scratch writes. Readonly cases reject workspace writes
but keep scratch usable. Immutable writes refuse with EROFS; private reads and
writes are refused. The /dev/null comparison records a mode-0666 character device,
nodev mount flag and EACCES write result, rather than the originally hypothesized
successful device write. The regular-file candidate remains independent of that
device-specific outcome.

The scoped review covered driver/actor separation, direct argument passing,
cleared child environment, namespace/capability flags, shallow-first anchor order,
positive controls, exact errors and unchanged owner data, bounded receipts and
source-bound failure reporting. No blocking issue was found within this mount
experiment. These receipts prove the primitive only: production Linux composition,
Landlock/helper integration and complete owner admission remain unimplemented.
