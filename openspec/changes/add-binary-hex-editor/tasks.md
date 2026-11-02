Delivery update, 2026-11-01: the user requested implementing issue #94 now,
superseding the earlier workstream scheduling prerequisite. The full editor scope,
native storage proof and shared snapshot/permission/FileChanged invariants remain.
See [implementation progress](implementation.md); the live view does not close the
staged-edit/save/model/integration requirements below.

## 1. Specification and review

- [x] 1.1 Verify PR #53 merged; inspect current explorer/preview, project file admission, text tools and snapshot contracts at `d36388d6`.
- [x] 1.2 Draft proposal, byte-service/save design, UX wireframes, testable capability deltas and phased tasks.
- [x] 1.3 Review the new extension with the user before implementation: manual plus model scope, resize behavior, resource limits, exact offset representation, native save guarantees and scheduling after existing workstreams. Approved on 2026-10-27: after PRs #53/#54 merged, the user explicitly requested implementation of both merged plans and then instructed goal resume. Native proof, baseline and acceptance gates remain required.
- [ ] 1.4 Record the approved native support/storage matrix and relevant unchanged-main baseline; align the accepted Daily coding snapshot and Plugins/hooks FileChanged contracts.

## 2. Byte service and native storage proof

- [ ] 2.1 Add typed byte-document/revision/offset/identity primitives with regular-file and project-root admission, exact integer transport and preserved permission ceilings.
- [ ] 2.2 Implement bounded pages, cancellable literal search and stable revision/continuation behavior without eager whole-file allocation or decoding.
- [ ] 2.3 Prove native private immutable base preparation, same-filesystem atomic replacement with displaced preimage, metadata preservation and durable intent/outcome recovery; define unsupported outcomes honestly before write integration.
- [ ] 2.4 Test native file/parent replacement, symlink/hardlink refusal, every byte value, NUL/invalid UTF-8, sparse/large files, offsets above 2^53 and resource exhaustion with positive controls.

## 3. Editing, review, save and recovery

- [x] 3.1 Add expected-revision overwrite/insert/delete splice batches, exact preimage checks, bounded payloads and atomic draft undo/redo.
- [x] 3.2 Add stable byte-diff review with original offsets, base/proposed fingerprints, length/changed-range totals, metadata and effect disclosure.
- [ ] 3.3 Implement edit permission/Mode/checkout lease gates, capacity reservation and journaled save with operation idempotency and queryable acknowledged/conflict/unknown outcomes.
- [ ] 3.4 Preserve dirty drafts and unresolved receipts over disconnect/restart; add explicit discard and reviewed Restore Last Save without unguarded overwrite.
- [ ] 3.5 Test stale revision/review, same-size/same-mtime edits, all-or-nothing multi-splice changes, concurrent Sessions/external writers, cancel/kill cuts, retry identity and backup integrity.

## 4. Owner API, SDK and model tool

- [ ] 4.1 Implement authenticated `fs` document/page/search/patch/history/review/save/restore/operation/close routes with exact Session/Location scope and no raw-byte URLs/logging.
- [ ] 4.2 Generate OpenAPI/TypeScript types and shared client helpers with lossless offsets, bounded decoding, operation lookup and revision validation; preserve text content routes.
- [ ] 4.3 Register the capability-owned `binary` tool and implement bounded inspect/read/search/patch through the shared service, isolated from unsaved owner drafts.
- [ ] 4.4 Enforce read+edit, plan/read-only/deny ceilings, existing output budgets, permission byte-diff review and plugin filesystem intersections; retain normal text-tool behavior.
- [ ] 4.5 Test owner auth/Origin, cross-document/Session/Location access, stale generation, read-denied/write-allowed cases, malformed encodings, oversize payloads and secret-safe errors.

## 5. TUI and direct CLI

- [ ] 5.1 Build the virtualized themed hex/ASCII workspace, shared cursor/selection, adaptive rows, inspector and change rail with explicit view/edit/review/save/conflict states.
- [ ] 5.2 Add goto, literal search, bookmarks, encoding-labelled text/hex input, partial-nibble cancellation, bounded copy/share, draft undo/redo and unsaved-change navigation.
- [ ] 5.3 Add explorer entry/palette/keybindings, `/hex` and `cyber hex` without inference, preserve Session selection and global interrupt/permission/native Stop priority; regenerate CLI documentation.
- [ ] 5.4 Add wide/narrow/light/dark/NO_COLOR keyboard/mouse/accessibility snapshots and actual terminal captures; verify dirty-state, byte-diff totals and stale response fencing visually.

## 6. Snapshots, hooks and delivery

- [ ] 6.1 Integrate durable binary preimages/receipts, byte diffs, conflict-aware restore and retention for Git/non-git Locations even when ordinary snapshots skip the file.
- [ ] 6.2 Publish only acknowledged successful file-change observations through the existing producer contract; preserve unknown effects and avoid duplicate notification after save retry.
- [ ] 6.3 Benchmark first viewport, bounded memory, navigation/search responsiveness and cancellation on a named reference machine; publish fixture sizes and limitations.
- [ ] 6.4 Run relevant native Rust/integration/real-binary/TUI tests, SDK typecheck/tests, generated artifact checks, strict OpenSpec/lint, formatting/Clippy and scoped platform CI. Record skipped/unperformed acceptance.
- [ ] 6.5 Perform code/security review using the code-review skill; resolve findings, update README/feature/roadmap/operator guides with actual scope and deliver reviewable PRs. Do not claim format repair or arbitrary external-writer atomicity.
- [ ] 6.6 After authorized acceptance/merge, archive and reconcile canonical specs; do not close the existing combined implementation goal based on this specification alone.
