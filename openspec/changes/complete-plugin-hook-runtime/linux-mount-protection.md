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
requires native Ubuntu x64 and arm64 receipts before the candidate can be treated
as proved. Expected control/candidate outcomes here remain hypotheses until those
jobs finish; no Linux native acceptance is claimed by these local checks.

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
