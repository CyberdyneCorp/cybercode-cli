# Explicit production host environment

The production `LaunchContext::start` now builds the plugin environment from an
explicit template instead of ordinary-tool `policy_env(std::env::vars(), ...)`.
That former path could carry runtime loader options and arbitrary non-secret
variables; tool environment `allow`/`set` could also inject startup settings or
credentials. A plugin capability must not implicitly delegate that owner state.

The host gets private scratch for HOME/temp/XDG directories and Bun/Deno caches,
fixed C locale and UTF-8 Python I/O, plugin name/root, and only proxy variables
provided by the owner's sandbox wrapper. Unix child PATH is a fixed system/tool
search list. On Windows, only the absolute system-root locator is additionally
read from the trusted owner's startup environment for OS bootstrap; project/tool
overlays do not supply it. Windows profile/application-data paths use scratch.
The final owner-credential value filter still runs after construction.

No inherited/configured loader option, language import path, runtime option,
proxy override, user identity, provider credential or arbitrary metadata variable
is included. Plugin settings and secrets continue through validated initialize
configuration and the existing capability-checked secrets callback. Ordinary
tool environment policy is unchanged. The template accepts no owner-environment
iterator or resolved tool policy, so future additions require an explicit review.

This changes the real host spawn path, but is not complete native admission.
Initial runtime lookup still needs fixed reviewed resolution; implicit runtime
startup/project configuration, immutable helper/runtime/package ownership,
private-root policy, generation-bound launch and full native fs.read/shell
intersections remain open. Separate plugin MCP/hook subprocesses must also be
covered by their required enforcement work. No parent task or Windows native
support gate is closed here.

## Verification and scoped review

Unit regressions check private bootstrap paths, owner proxy inputs, rejection of
unreviewed wrapper fields and final owner-credential alias removal. A real
production App/HTTP regression starts the Python host with deliberately invalid
Python loader/search paths, dynamic-loader settings, Node options, Deno auth,
proxy overrides and arbitrary metadata in tool-policy overlays. It requires
successful initialize/tool execution and reports only dummy fixture booleans for
their absence, private paths, identity, fixed PATH and UTF-8 output. It does not
print the owner's environment or credential values.

Local checks, behavioral controls and hosted platform outcomes must be recorded
before acceptance. The scoped code/security review covers the production call
site, explicit template, proxy source and final reserved-value filtering,
platform differences, credential/configuration channels and omitted runtime
admission gates. No OS enforcement result is inferred from environment tests.

On local macOS arm64, both template unit tests and all 15 real production plugin
App/HTTP tests pass, including settings changes, secret callbacks and network
intersections. Reintroducing tool-policy overlays at the real spawn call site
makes the named bootstrap test fail because the host cannot initialize under the
deliberately invalid Python prefix. The regression shuts down the App before
asserting, including this failing control. Restoring the exact positive source
makes the bootstrap test pass again.

Scoped Tools/App Clippy across all targets passes with Rust warnings denied.
Formatting/diff checks and the new workflow's YAML/four Bash blocks pass. Strict
OpenSpec passes all 107 items; spec lint reports 0 errors and 25 warnings. Native
compilation emits the two explicitly permitted deprecated Seatbelt diagnostics;
the App test linker also reports a Mach-O EH-frame size warning. No origin is
assigned to that observed linker warning here. Current-source hosted Linux,
macOS Intel and Windows results remain pending. Windows only runs the template
unit checks; its existing unavailable native-admission scope is not expanded by
this change. The Unix workflow executes the complete production plugin suite,
checks the unsafe-overlay counterexample and restores the positive regression.


The first focused Linux run at `a4e97c1c` compiled successfully but failed 14 of
15 production tests because the fresh job had not built `cyber-sandbox-exec`.
The protocol-version case reported the exact SandboxUnavailableError for the
missing helper. Local macOS had passed with the existing distributed helper in
the shared target cache; that did not verify the fresh Linux job dependency.
The workflow now builds the actual sandbox helper explicitly before environment
and production tests, and watches sandbox sources as a dependency. No unconfined
fallback or skipped host acceptance is introduced. Updated native Linux results
remain required; this correction is not itself a passing Linux execution claim.
