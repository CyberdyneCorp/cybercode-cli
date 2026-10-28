# Hook producer integration

This draft integration combines PR #56 (`27f561ba`), PR #57 (`84c964bd`) and
PR #59 (`5aa08018`) without merging any PR into main. Individual acceptance
does not prove the composed runtime. DirectoryAdded, Setup completion, plugin
enforcement/protocol work and later workstreams remain open.

## Shared transition

`exit_idle_worktree` captures the committed CwdChanged observation and resulting
SessionInfo from the same acknowledged exit. After releasing the Session lock
and deletion fence, it refreshes host-owned file-observer membership, releases
its shutdown admission, then dispatches or defers CwdChanged through the existing
worktree owner mechanism. File callbacks follow the returned Location without
a new Turn; the committed CwdChanged producer retains stable transition identity
and cannot veto the move. Both event names remain in runtime/unavailable lists.

The real HTTP-hook idle-return regression now enables both event types together.
It checks exactly one enter and one exit observation with the moved Session and
resulting envelope, future delivery at the returned Location, exclusion at the
old worktree and no manufactured model Turn. Existing worktree and isolated-child
suites cover their own failed/no-op/replay/permission and pre-Git veto paths.

## Regression controls

Whole-file transplants from main now erase APIs required by another increment.
The dedicated workflows therefore retain independent implementations while
removing the specific production fix under test:

- Child admission control restores the base child implementation and only the
  matching resume call signature, retaining observer additions to BuiltinHost.
- Cwd control suppresses the committed observation constructor while retaining
  worktree ownership helpers and file membership.
- Idle membership control removes only its refresh call, preserving CwdChanged
  production references and callback behavior.

These are intentional missing-fix controls, not unchanged-main checks. Each
requires the expected test to execute and fail at its designated behavioral
assertion; compilation failure cannot satisfy the control. The earlier standalone
PR workflows retain historical unchanged-base evidence.

## Verification

Local Rust formatting and workflow YAML parsing are checked before publication.
Strict OpenSpec and spec lint are also run. Full local Rust builds are not run
because available disk cannot accommodate the workspace build. Focused native
Linux/macOS/Windows and broad CI on this exact integration revision are pending;
no composed runtime acceptance or final security approval is claimed yet.
