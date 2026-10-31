# Active implementation goal and delivery rules

The full approved scope from PRs #53/#54 remains active, in this order:

1. [Plugins and hooks](../openspec/changes/complete-plugin-hook-runtime/tasks.md):
   production native capability enforcement, complete host protocol operations,
   scoped prompt callbacks and missing hook-event producers.
2. [Daily coding](../openspec/changes/complete-daily-coding-surfaces/tasks.md):
   rewind/undo/redo, non-Git snapshots, path-scoped rules and context inspection,
   root structured output and streaming JSONL input.
3. [Binary hex editor](../openspec/changes/add-binary-hex-editor/tasks.md):
   the full reviewed CLI/TUI editor, owner/SDK/model operations and integrations.

## Delivery constraint approved on 2026-10-29

The user requested fewer simultaneously open PRs. Keep normally **one open goal
PR**, with a **maximum of two** only when an independent change genuinely needs
separate review. This changes delivery granularity, not feature scope or acceptance.

- Finish and verify the agreed delivery increment before opening the next PR.
  On 2026-10-30 the user authorized merging #76 as a foundations milestone once
  its CI is green and merge review is satisfied, then continuing the full goal
  in one follow-up PR. This milestone does not complete production enforcement.
- Keep related experiments, evidence, implementation fixes and CI fixes in the
  active PR. Do not open a PR for each probe or supporting component.
- Cancel superseded PR/revision CI runs after verifying their source is retained.
  Main CI, like the focused workflows, cancels older runs for the same PR/ref;
  completed evidence remains associated with its actual tested revision.
- Consolidate the existing #70–#76 stack into draft #76 against `main`, retaining
  the contained commits, source branches and evidence. Close #70–#75 as superseded
  after verifying their heads are contained in the consolidated branch.
- Use the active PR description to distinguish production behavior, experimental
  candidates, verified results and remaining requirements. Keep it draft while
  its delivery increment is incomplete.
- Do not open the next implementation PR until the current increment is merged,
  superseded or deliberately split for independent review within the cap.
- Prioritize fixing failing checks and making the active increment ready for
  review before starting another delivery increment. Report pending and failing
  checks explicitly; do not describe the active PR as ready while they remain.
- Count goal PRs separately from unrelated repository PRs. Inspect ownership and
  scope before consolidating or closing other contributors' work; the delivery
  cap does not authorize closing unrelated PRs.
- Preserve native proof and explicit support/design revision gates. Passing probes
  are not production feature acceptance. Never weaken the agreed contract merely
  to finish a PR. Merge only with user authorization; the conditional approval
  above originally applied to #76. On 2026-10-30 the user additionally authorized
  merging ready repository PRs as work continues. Check current-head CI, scope,
  review and mergeability before each merge, and recheck composition as main
  advances. This does not waive draft, native proof or design-revision gates.

PR [#76](https://github.com/CyberdyneCorp/cybercode-cli/pull/76) merged on
2026-10-30 at `ec31ff9fc7627a3a9262d99f0fc0606394368423`, after all 70 applicable
checks passed and the optional diagnostic was intentionally skipped. Its merge
tree matches the reviewed/tested `e6fd3d2f` tree. The user approved continuing the
[Linux broker direction](../openspec/changes/complete-plugin-hook-runtime/native-host-integration.md)
with runtime kernel capability checks and full private IPC/network compatibility
remaining required before activation. The original tested sources remain on
`evidence/plugin-native-host-pre-76-merge`. Follow-up #80 merged at `2745b4f0`
and #83 at `c3bd34c4` on 2026-10-31. These are foundations, diagnostics and the
observer resource correction; production native admission remains incomplete.
PR #84 merged at `dd0c9279` after all 55 configured checks passed on
`62a3068e`, with one intentional historical diagnostic skip. Its corrected
native result witness and deterministic missing-guard regression passed Intel
and arm64; that accepts the verifier correction rather than production native
admission or the historical unknown race cause.

PR #85 merged at `5d5826b7` after all 19 checks passed. It accepts bounded
FileChanged task 5.2 with verified native Windows retarget coverage and a reviewed
main composition; see the
[acceptance audit](../openspec/changes/complete-plugin-hook-runtime/file-observation-acceptance.md).
PR #88 merged at `9cbbf08b` after all 18 checks passed, including all five
CwdChanged lifecycle cases on Windows.

PR #89 merged at `bbc233d5` after all 16 current-head checks passed. Its
[receipt](../openspec/changes/complete-plugin-hook-runtime/evidence/cwd-settlement-9ad0b369.json)
verifies the original unacknowledged Location-settlement regression actually
executed on Linux/macOS/Windows. This accepts its simulated lease-owner case,
not native confinement or the complete producer matrix.

Continue the first workstream on `test/cwd-wal-commit-ack`: the
[Store commit correction](../openspec/changes/complete-plugin-hook-runtime/store-commit-uncertainty.md)
fences generic errors after a transaction has ended, preserves known rollback,
and verifies no CwdChanged or later shell dispatch follows the uncertain outcome.
The real WAL-error fixture and main comparison pass locally after correction;
current-head hosted acceptance remains required. The new replay case uses a fresh
runtime over the same Store rather than a process restart. Keep native activation
and Linux dependency changes behind their existing review/acceptance gates. The
complete Setup marker/migration choices and DirectoryAdded proposal also need
their recorded review before implementation.

Native confinement foundations and regressions exist, but
production enforcement and the remaining plugin/hook scope are incomplete. The
Linux UNIX-socket diagnostic found a real host-reachability gap. Its experimental
filter and runtime checks remain experimental. The broker direction is approved;
complete support acceptance and host activation remain pending. Daily coding and binary editor implementation have not
started. The linked task lists remain authoritative for completion.

The [Linux peer-identity diagnostic](../openspec/changes/complete-plugin-hook-runtime/linux-private-peer-identity.md)
confirms that the current broker changes the caller PID observed by a UNIX
listener, despite successful data exchange. The
[kernel UNIX revision](../openspec/changes/complete-plugin-hook-runtime/linux-kernel-unix-revision.md)
is a concrete proposal awaiting review. It does not reduce IPC requirements or
authorize a new production support floor. Preserve this review gate before
production activation or protocol-feature integration.
