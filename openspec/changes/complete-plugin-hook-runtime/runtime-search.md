# Production runtime search guard

The host's initial runtime lookup now ignores relative/empty PATH entries and
canonical Location/package directories. It also rejects candidates whose resolved
target is inside either excluded tree, including an outside shim symlink pointing
back into a project. Selection returns the canonical absolute path rather than
the original mutable directory alias. Missing/unresolvable scope roots refuse
selection. Unix candidates must be executable regular files; Python3 preference
and owner PATH ordering remain intact.

This guard is used by the real production launch command, before wrapping and
spawning the host. A missing safe runtime produces a plugin-named diagnostic
asking for an executable on an absolute PATH entry outside Location/package.
The source package's declared native entry remains its own explicit entry; it is
not selected from PATH. Ordinary command-tool lookup is unchanged.

This is not complete reviewed runtime admission. Outside PATH entries are still
owner environment input. Mutable outside binaries/dependencies, hard-link aliases,
directory replacement races, fixed runtime provenance and generation-bound image
ownership still require the native admission work. Canonicalization is a lookup
guard, not exclusion against concurrent writers. Windows legacy `.cmd` selection
is retained without claiming that such a shim is a native-admitted runtime.

Regressions use explicit fixture PATH values rather than mutating the process
environment. They cover Location/package shadows, outside file/directory symlink
aliases, an otherwise safe target exposed through a project PATH entry, real
relative/empty entries, non-executable files/directories, unresolved scope roots
and runtime-name preference. On Unix, the selected fixture executable actually
returns its owner token. A missing-exclusions control must execute the project
fixture instead and fail that exact token-receipt assertion; a compiler/bootstrap
failure cannot count. Windows additionally checks a case variant of Location.

The existing host-bootstrap workflow runs the search suite on Linux x64/arm64,
macOS arm64/Intel and Windows, with actual receipt controls on Unix and restored
search tests afterward. Complete runtime startup/production host regressions
remain in that workflow. Hosted execution of this new guard remains required.

## Local verification and review

On macOS arm64, all seven search regressions pass. Removing both scope exclusions
makes the real selected fixture return `project` instead of `owner`, failing the
exact receipt assertion. After restoring the source, the broader plugin-host suite
passes 25 tests; the two configured Bun/Deno startup receipts remain ignored.
Neither runtime is installed on this host, so those checks are not locally
verified. The focused hosted workflow installs them and explicitly runs them.
Tools library/test Clippy passes with Rust warnings denied; two deprecated C
Seatbelt SDK warnings are observed without assigning their origin. Rust formatting,
workflow YAML/ten shell blocks, diff checks, all 107 strict OpenSpec items and
spec lint (zero errors, 25 warnings) pass. Windows case/alias behavior and the
new hosted receipt controls still require CI evidence.

The scoped code/security review covers the selector, production wiring, launch
tests, SDK guidance, OpenSpec and workflow. It confirms that both directory and
resolved file target exclusions are necessary, preserves name ordering, keeps
environment injection out of the tests, and verifies a restored suite after the
specific failing receipt. No additional source finding remains within the lookup
guard. The unresolved immutable identity, hard-link and writer-race obligations
above are not claimed fixed by this increment, and no parent task is closed.

## Hosted control correction at `19cf223c`

The Linux x64/arm64 and Windows bootstrap jobs pass the new search suite and
the six bundled startup tests. The later Bun/Deno missing-filter control then
fails before executing a test: its mutation script still used the removed
`find_program` function as an end marker. This is a workflow refactor error,
not a passing denial control. It now selects the actual Bun/Deno argument-builder
functions; both exact mutation scripts are verified against temporary source
copies locally. Fresh hosted control and restored-source execution remain required.
