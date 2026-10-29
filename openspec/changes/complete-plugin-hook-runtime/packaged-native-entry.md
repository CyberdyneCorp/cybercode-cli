# Packaged policy-before-target entry

This increment promotes the macOS C entry from the existing confinement probe
into the distributed `cyber-sandbox-exec` binary. Cargo builds its native archive
with the package, so starting a plugin will not need a user-installed C compiler.
The private binary module invokes it through a small reviewed FFI boundary. The
owner library exposes no function that could accidentally apply this replacing
exec to the owner process itself.

## Internal entry and bounds

`--plugin-native-exec <absolute-profile> <absolute-target> [target-args...]` accepts
at most 256 arguments and 64 KiB including terminators, with profile/target paths
at most 4,096 bytes. Owned NUL-terminated strings and the pointer vector remain
alive throughout the native call; the C ABI receives a bounded 32-bit count.
Relative paths, embedded NUL and the probe's `-` policy bypass are refused.
Errors do not echo arguments or profile contents.

The native entry opens the profile without following its final symlink, using
nonblocking/no-controlling-terminal flags. It requires a regular, singly linked,
nonempty file of at most 64 KiB, reads the exact captured size and rejects embedded
NUL. A FIFO is refused before waiting for a writer. The owner must protect the
profile and all ancestor bindings from every admitted untrusted writer; the file
checks alone do not establish immutable ownership or race-proof provenance.

The policy is installed before replacing the process with the target. Exec-time
`POSIX_SPAWN_CLOEXEC_DEFAULT` closes inherited descriptors, including descriptors
above a lowered soft limit; only present stdin/stdout/stderr are explicitly
inherited. `POSIX_SPAWN_SETEXEC` retains the process identity owned by the caller.
Policy failure refuses startup, with no ordinary-tool or unconfined fallback.
The C build intentionally permits the SDK deprecation diagnostics for the
`sandbox_init`/`sandbox_free_error` API used by the proven mechanism. Native
platform tests remain required; a successful compiler invocation is insufficient.

## Actual packaged-helper verification

Tests copy and sign the built helper with a fresh subject identifier and verify
all architecture slices. They reuse the original probe's native exec and
constructor fixtures. Real controls require:

- Normal initial execution and self-exec under a broad exec-only control profile.
- Initial execution followed by denied self-exec under the helper-subject/exact
  target rule, for a controlled native image and the original `/bin/bash`.
- An unchanged original system image, normal startup output and a permission
  error for denied exec; a signal-terminated image cannot satisfy the check.
- Exec/spawn attempts from constructors to see policy before `main`.
- Descriptor 500 to survive a direct control after the soft limit is lowered to
  64, then be absent when entering through the packaged helper.
- Refusal before target startup for malformed, missing, empty, oversized,
  NUL-containing, linked, directory, symlink and FIFO profiles.

A matching target/helper signing identifier deliberately self-execs successfully.
That observed counterexample remains mandatory: the native entry alone cannot
certify safe admission. The owner must establish immutable, all-architecture
identity separation and protected package/runtime/helper provenance before using
this mechanism. The profile used to isolate exec tests grants broad reads and
therefore does not prove `fs.read`, network or secret confinement.

On local macOS arm64, the argument unit test and six real-process integration
tests pass. Removing nonblocking profile open makes the named FIFO test fail at
its deadline; it kills/reaps its owned child before reporting that failure. The
correct code is restored before the positive suite. Other owner/copy regressions
remain required after this packaging change.

The initial focused workflow tested macOS arm64/Intel and explicit unsupported
internal mode behavior on Linux arm64/x86_64 and Windows.
[The Linux packaging follow-up](packaged-linux-entry.md) adds the Linux entry,
mandatory shell policy and native tests; Windows still reports unavailability. It records the checked-out source
hash and built helper digest alongside the job. Tests execute private freshly
signed helper copies, so that build digest identifies their input rather than
claiming the modified signatures have the same hash. Source provenance is not
itself a passing test result. Native Linux packaging remains implementation work;
the approved Linux runtime support scope stays required for integration.

## Remaining integration and gates

The entry was introduced for macOS; the linked follow-up adds Linux. Windows
returns the explicit unavailable-mode result without starting a target. Existing plugin/ordinary-tool
launches retain their existing behavior in this increment. The strict approved
platform/runtime contract remains the delivery target.

Task 2.2 remains open for native platform acceptance, reviewed runtime resolution, complete
private-root/alias composition, immutable image/helper identity, sanitized
environment and launch lifetime. The owner must bind its generation to the current
plugin/digest/Location and serialize the final launch check with revocation;
already-running processes must be stopped before their resources are released.
Actual App/server enforcement, independent fs.read/fs.write/shell/network/secret
intersections and required native acceptance remain before completion. The
existing proof/design gate before protocol integration remains in force.

## Verification recorded for this increment

On macOS arm64, one binary argument unit test, six packaged-entry tests, nine
preparation/authority tests and five actual copy-helper tests pass. The copy
suite's two SQLite/digest fixture entrypoints are explicitly invoked by its
regression. Scoped sandbox Clippy across all targets passes with warnings denied
for Rust; the native compiler emits the two explicitly permitted SDK deprecation
diagnostics described above. Formatting, diff checks, workflow YAML/all shell
blocks, the Windows test-profile mapping and independently recalculated source
and binary digests pass. Strict OpenSpec passes all 107 items; spec lint reports
0 errors and 25 warnings. Hosted platform execution is still required.

The scoped code/security review covers the private binary-only FFI, C string
lifetimes and count bounds, mandatory policy installation, profile types/limits,
error redaction, exec-time descriptor inheritance, real constructor controls and
the explicit identity-collision counterexample. Immutable helper/profile/package
ownership, kernel signature-mutation coverage and final launch authority remain
integration requirements; this review does not waive them.
