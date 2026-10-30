# Package runtime startup isolation

Bun and Deno startup now selects configuration from the plugin package rather
than leaving discovery to the Session's Location. Host CWD is the package root;
workspace operations use the Location in initialize. No committed Session
Location transition or CwdChanged event is manufactured by the child CWD choice.

Bun uses a regular package bunfig.toml or the platform's empty device, with
--no-env-file and --no-install. Relative package preloads therefore resolve from
the package root. Deno selects regular package deno.json/deno.jsonc, or --no-config
when absent. An explicit package lock is frozen; without one, --no-lock prevents
ambient lock discovery. Node module directories remain manual and the explicit
host environment disables Deno's external update check. Directories and symlinks
cannot supply a selected runtime config. These metadata checks are not immutable
source/ancestor admission and do not open source file contents in the owner.

## Prepared workspace boundary

Deno 2.9.8 still searches ancestors for a workspace even with an absolute config
argument. The controlled fixture reproduces that failure under a malformed
ancestor config. A Deno workspace listing the image as a member would also change
root-only configuration semantics. Instead, successful Unix Package binding
creates a fixed parent package.json with {"private":true,"workspaces":[]} in its
already-required protected namespace. The empty workspace stops ancestor lookup
without converting the package into a Deno workspace member. This follows the
runtime's [workspace/config model](https://docs.deno.com/runtime/fundamentals/workspaces/)
and is verified with the actual pinned executable, not inferred from the flag.

The file is outside image/, so the approved image digest and source file count
remain unchanged. Creation refuses an existing entry, writes only fixed bytes,
uses mode 0400, and retains a read-only file plus namespace directory handle until
Package drops. Revocation still rejects publication/access; it does not delete
resources while an owner may still have to stop a process. Bootstrap metadata
adds one fixed-size file and two retained descriptors per prepared package. Only
new owned metadata is opened/closed by this addition, preserving the source-IO
separation needed for owner SQLite locks.

The caller must protect the namespace and ancestors from every untrusted writer,
retain it through process reaping, and allow the exact boundary file as a reviewed
bootstrap read in native policy. Production host launch does not yet use the
prepared-image path or complete native admission; current raw-source Deno config
selection alone does not certify ancestor isolation. Immutable runtime/helper
resolution, private/alias composition, generation-bound spawn and native
fs.read/shell/network/secret intersections remain required. No enforcement or
support gate is closed here.

## Regression and platform scope

Real Bun/Deno processes use the production argv/environment constructors and
exchange initialize, tool/execute and shutdown messages. Cases cover absent and
bundled Bun config, Deno JSON and JSONC import maps, frozen package locks, relative
bundled imports/preloads, poisoned ancestor configuration, dotenv, unchanged
source lock/config and no implicit node_modules. Unix Deno cases with bundled
config use the actual preparation helper and Package ownership. Windows uses
controlled fixture staging only: its copy/native admission remains unavailable,
so those CLI receipts cannot establish Windows confinement.

The focused workflow installs Bun 1.4.4 and Deno 2.9.8, explicitly runs the two
normally ignored runtime tests, checks missing-filter controls, restores positive
source and retains counterexamples. All parent native integration and complete
plugin acceptance obligations remain open.

## Local verification (2026-10-29, macOS arm64)

- Production plugin-host tests: 20 passed, including both explicitly enabled
  runtime tests (two Bun and three Deno process cases). Production App plugin
  integration tests: 15 passed, including package CWD and environment receipts.
- Sandbox owner/authority unit tests: 15 passed. Actual-helper integration tests:
  five passed; two ignored subprocess fixture entrypoints are explicitly invoked
  by the SQLite-lock regression, rather than counted as independent passes.
  The package test checks unchanged approved image digest, boundary bytes/mode,
  and cleanup after owner drop. Boundary collision tests refuse regular files
  and symlinks without modifying existing bytes.
- Removing Bun's dotenv filter, Deno's no-config flag, or the recognized parent
  package.json boundary each fails its named runtime regression. Restored source
  passes. The workflow's exact missing-boundary script was executed locally and
  its restored launch suite passed all six tests.
- Scoped Tools/Sandbox/App all-target Clippy passes with Rust warnings denied.
  The macOS C compiler reports two deprecated Seatbelt SDK calls; their origin
  is not assigned here.
- Formatting and diff checks pass; all eight workflow Bash blocks parse.
  Strict OpenSpec validation passes all 107 items. Spec lint reports zero errors
  and 25 warnings; this change does not establish their origin.

Bun 1.4.4 and Deno 2.9.8 came from the official runtime release assets for macOS
arm64. The downloaded archives matched their release API SHA-256 digests:
Bun c240d7f0ccb90c263a849fcdb09b7ee62630d921348ed6aafda86b9d577bc510;
Deno 9f0bf660393da2eaff044cdc7444cb08c73f235849b61d224d825a194ce7c98e.
Only their binaries were extracted into a private temporary tools directory.
No global runtime installation or user PATH change was made.

Separate existing packaged-helper native matrices passed nine cases per runtime
for these Bun/Deno executables on macOS arm64. Those exercise the experimental
native policy fixture, not this change's bundled-config/ancestor-boundary launch
path. They do not satisfy production admission or cross-platform acceptance.
Hosted results for the new startup matrix must be inspected after publication.

## Scoped implementation review

Review covered the production constructors, package lifetime and metadata,
App/owner/CLI regressions, workflow controls, SDK documentation and spec delta.
The runtime helper logic stays within the existing process adapter; no new
protocol or public schema is introduced. Config filenames and argv are explicit,
without shell interpolation. Source contents remain confined to the existing
copy worker/runtime; owner-side selection uses metadata only. The new static
boundary is created exclusively in the owned namespace, retained until Package
drop, and never inserted into the digest-approved image. Tests cover package
preloads/import maps, directory/symlink refusal, digest preservation and cleanup.
No blocking issue was found within this bootstrap scope.

Remaining security requirements are explicit: metadata selection does not close
source/ancestor races; retained handles do not establish an immutable namespace;
initial runtime lookup still uses owner PATH; production launch is still the
legacy sandbox; prepared-image native integration and protected/alias-root
composition remain unimplemented. Windows fixture receipts prove CLI behavior
only. These are open parent tasks, not claims of completed capability enforcement.


## Intel hosted job budget

At source `9e802ec1`, [Intel job 123660939230](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40934385871/job/123660939230)
was cancelled by its 30-minute job limit. GitHub's annotation explicitly records
that maximum execution time. Helper build, environment construction, bundled
runtime startup, missing-filter controls and the prepared workspace boundary
step passed. The job was still compiling Rust dependencies for the production App
plugin tests when cancelled. Those App tests and the later overlay control are
not passing results for that job.

The workflow job budget is now 60 minutes to accommodate that compilation plus
the required regressions. Runtime receipt deadlines, control assertions, worker
limits and source restoration are unchanged. Fresh hosted Intel completion is
still required; neither the cancellation nor increasing the build allowance
establishes acceptance.
