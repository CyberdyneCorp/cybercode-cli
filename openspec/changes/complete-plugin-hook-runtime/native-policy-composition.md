# Native plugin policy composition

The macOS filesystem/process policy from the confinement experiment now has a
bounded Rust composer in `cyber-sandbox::plugin_policy`. This is a production
library component, not production host activation or a native admission permit.
It keeps the plugin-specific contract separate from ordinary command sandboxing.

The owner supplies the selected target, fresh helper signing identifier, package,
scratch, Location, typed file/directory bootstrap reads, private roots and
write-protected paths. Nothing is discovered from project configuration or PATH.
Inputs must be canonical, bound native paths; validation here is syntactic only.
The caller still attests immutable runtime/helper/package provenance, complete
private/alias inventory, protected ancestors, all-architecture signing identity
separation and generation-bound start/lifetime before executing the profile.

Profiles deny by default. Bootstrap/package/scratch reads and executable mappings
are explicit. Scratch remains readable/writable. Location reads and writes are
independent flags. All immutable bootstrap/package/target write denials follow
Location grants, so an enclosing writable Location cannot make them writable.
Private roots deny reads and writes. Every ancestor of immutable/private and
write-protected paths denies unlink/rename, including ancestors inside Location.
Other public directory renames remain possible with workspace writes enabled.

Without shell capability, only the exact target may exec while the subject has
the freshly signed helper identity; process fork remains denied and threads are
allowed by the native mechanism. With shell capability, fork/exec are permitted
under the same inherited filesystem profile. Matching target/helper signing
identity remains a known bypass unless the caller establishes separation before
launch; composing text cannot establish it.

The component supplies no network grant. Native network/proxy/secret intersections
must be integrated and tested separately before activation. Linux needs its own
Landlock plus private mount/PID/network composition; this Mac profile is never
silently supplied for Linux or Windows. No platform support gate is closed.

## Bounds and validation

Inputs have at most 64 paths in total, 64 KiB of path text, 4,096 bytes per path,
and 64 path components. Relative/root/parent-traversing/control-character paths
and unsafe scratch/private/bootstrap overlaps refuse. The helper identifier is
bounded to 256 ASCII letters, numbers, dots, underscores and hyphens. Paths quote
backslashes and double quotes for the native profile syntax. Rendering checks the
64 KiB output ceiling on each write rather than allocating an unbounded profile.
No filesystem contents, metadata or verification subprocesses are opened here.

## Verification and integration status

Real packaged-helper tests use the Rust composer, independent native payloads,
fresh all-slice-verified helper/target identities and bounded process receipts.
One payload exercises all eight fs.read/fs.write/shell combinations, including
threads, constructor-time reads/forks, child/shell inheritance, private symlink
and hard-link attempts, package alias writes and real JSON-RPC tool execution.
Another tests protected ancestor rename, direct package/runtime writes and a
positive public directory rename. The capability matrix uses a workspace name
containing spaces, double quotes and a backslash to verify native quoting.

On local macOS arm64, all three composer unit tests and all eight packaged native
entry tests pass. The two new native tests include eight capability combinations
and the mutation/positive-rename cases. Twenty-two owner/authority/inventory
tests and five actual-helper integration tests also pass; two ignored fixture
entrypoints are explicitly invoked by the SQLite regression.

Four controlled mutations each fail a named native assertion: missing ancestor
unlink denials permit protected parent rename; missing immutable write denials
permit package data modification; missing private-root denials permit protected
writes; unrestricted fork/exec permits a shell without shell capability. The
correct source is restored, and all three unit/eight native tests pass again.
The workflow retains these logs and verifies their precise assertion reasons,
rather than accepting an arbitrary failed compilation as a successful control.

Scoped Sandbox all-target Clippy passes with Rust warnings denied. The C compiler
reports the two deprecated Seatbelt SDK calls; no origin is assigned here.
Formatting, standalone formatting of the included native test helper, diff
checks, TypeScript syntax for the source-provenance script, workflow YAML/all six
Bash blocks, all 107 strict OpenSpec items and spec lint (zero errors, 25 warnings)
pass. Hosted macOS arm64/Intel execution and other-platform explicit-unavailable
results remain required; local execution is not their substitute.

The scoped code/security review covered every changed builder, unit/native test,
workflow, provenance script and implementation note. The composer stays in the
filesystem/process adapter and performs no owner IO. Inputs are bounded before
collection; output is bounded incrementally. It distinguishes exact-file grants
from directory grants, quotes native paths, applies immutable/private denials
after broad Location grants and protects ancestor bindings. Tests retain actual
positive startup, threads, permitted child execution and public writes/renames.
No blocking issue was found within this composition scope. Syntactic path checks
and caller-supplied identity strings do not attest immutable native provenance;
that obligation and the activation gates below remain open.

The production host still uses its legacy sandbox. Namespace provisioning,
runtime/helper admission and resolver, prepared-image integration, revocation and
process settlement, Linux composition, native network/secret intersection and
explicit native proof/design review remain required. The full approved task 2
and all later plugin/hook, Daily coding and binary editor obligations stay open.
