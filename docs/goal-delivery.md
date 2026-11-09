# Active implementation goal and delivery rules

## Current audit: 2026-11-09

The user has set the active objective to complete implementation, verification
and accepted delivery of **#93**. #92 remains a separate unfinished workstream;
preserve its accepted behavior and dependency gates without assuming its missing
guarantees or selecting its pending policies. The owner's closed #94 is outside
this objective. The #92 rows below retain dependency context from the earlier
combined audit; they are not additional scope for the current goal.

Main `8266db6c` is integrated into `feat/managed-recovery-owner` at `2fcd745d`.
The merge retains snapshot admission/reclamation and the incoming root-schema
validation/settlement events, plus the binary-worker activity-drain wait. The
seven pre-existing uncommitted files were restored from a retained backup stash.
The branch still contains substantial undelivered recovery work; it must not be
confused with accepted main behavior. GitHub reported both issues open and no
open PRs at audit time. The issue #93 checklist trails the repository's merged
section 5/6 implementation state; reconcile it at delivery.

| Area | Evidence and remaining work |
| --- | --- |
| #93 rewind/snapshots, tasks 2.4/3.2–3.4 and M2–M8 | Local worker origins, reviewed keep/rollback, paired Store settlement, native acknowledgements, CLI/TUI recovery and several payload-reclamation paths exist. Managed Git partial-kill acceptance passes locally, but default Git admission remains gated. Finish partial resource construction, complete reference/CAS/metadata and managed/Git reclamation, remaining crash cuts, platform/client acceptance and delivery. Preserve legacy unknown fences. |
| #93 rules/context, tasks 4.1–4.4 | Implement trusted path-rule discovery/precedence/bounds and authorized-operation reminders with Epoch provenance. Extend the existing `/sessions/:id/context` endpoint beyond epoch/baseline/summary/task to effective source inventory, estimates/status/pending changes; add the owner-facing `/context` flow and leakage/restart/revocation acceptance. The sidebar meter is not that inspection feature. |
| #93 structured results, tasks 5.1–5.4 | #116 merged admission/schema bounds, finalization, retries, durable per-input outcomes and CLI/API/SDK rendering. Three real CLI and five server tests pass after integration. Compaction during an open attempt and budget/interrupt terminal outcomes still need acceptance; do not infer them from restart between completed prompts. |
| #93 JSONL, tasks 6.1–6.4 | #117 merged basic typed parsing, correlation, replies and EOF draining. Complete exact driver ownership, bounded retained state and output backpressure, hook-event output, safe atomic last-message publication, and steering/capacity/SIGINT/restart/lost-ack/shared-Session acceptance. Task 6.2 is reopened on inspected code evidence below. |
| #92 native enforcement, tasks 2/N/R | Foundation/probe evidence exists, but production launch still uses the legacy sandbox route. Finish generation/image-bound native admission, revocation/quarantine/lifetime, independently intersected capabilities, real tool execution and native runtime/platform positives/denials. Linux R1 direction/support review and later R6 activation remain distinct gates. |
| #92 protocol/callbacks/producers, tasks 3–6/S/D | Finish contribution ownership, hooks/providers/catalog transforms and TypeScript kit, durable scoped callbacks (`session/prompt` still explicitly refuses), Setup and DirectoryAdded plus combined producer acceptance. Preserve accepted FileChanged and CwdChanged fixes. Complete S1 epoch/directory-key/legacy-deletion and D1 directory-admission/Safe Boundary/child-policy review before their dependent code. |

### Focused code-review findings and next actions

This review covers incoming JSONL behavior and merge overlap, not acceptance of
the entire recovery branch. Rust integration tests are executable; native
cross-platform, SDK/plugin and full-workspace checks have not been rerun here.

1. **Integration regression: optional snapshot capture fails JSONL results.**
   Unchanged main passes all six JSONL tests. The integrated candidate initially
   fails four because `Driver::failed` treats `kind: snapshot` as a failed input,
   despite successful inference and ordinary exec's nonfatal warning handling.
   The local fix keeps these diagnostics nonfatal and adds a real-binary
   regression with unavailable Git snapshots. All seven JSONL tests and all
   three structured-output CLI tests pass after the fix.
2. **Major: request and interruption ownership is Session-wide.**
   `Driver::asked` accepts every matching Session request and `interrupt` targets
   the Session without binding to the driver's admitted messages. Track exact
   input/request authority and test two drivers/owner clients sharing a Session,
   including unrelated work present before driver startup.
3. **Major: long runs retain completed payloads and output blocks the loop.**
   `inputs` retains every completed text/structured result; the pending-input cap
   does not bound that history. `Out::line` writes/flushes stdout synchronously
   on the same loop that handles controls and lease renewal. Retire emitted
   payloads into bounded accounting and use a bounded output owner with explicit
   I/O-failure settlement; test long runs, slow readers and broken pipes.
4. **Fixed locally: last-message publication used a predictable temporary path.**
   Publication now uses an exclusively created sibling file, synchronizes it and
   atomically replaces the destination, with automatic temporary-file cleanup on
   failure. Three unit regressions cover pre-created temporary/destination links,
   concurrent complete answers and cleanup after failed publication. All eight
   real JSONL tests pass, including failed publication preserving the answer while
   returning aggregate exit 1 and an `end` error. Interrupted/protocol exit codes
   retain precedence. This does not complete the ownership/backpressure findings.

Verification: three structured-output CLI tests, seven JSONL CLI tests, five
structured-result server tests and six binary-worker ownership tests pass;
formatting and whitespace
checks pass. CI-pinned OpenSpec 1.13.2 validates 100 items after the incoming
archives; spec lint reports zero errors and 25 warnings. The preparation case now
passes with an independent staging deadline. A separate unconstrained-run
first-prompt startup stall remains unresolved; its isolated rerun and the complete
suite with four parallel threads pass. The stalled fixture's diagnostics are
retained rather than counted as a passing run.
The integration regression is locally corrected; finish the current recovery
delivery increment before starting another broad implementation stream.

The full approved scope from PRs #53/#54 remains required. Its original delivery order was:

1. [Plugins and hooks](../openspec/changes/complete-plugin-hook-runtime/tasks.md):
   production native capability enforcement, complete host protocol operations,
   scoped prompt callbacks and missing hook-event producers.
2. [Daily coding](../openspec/changes/complete-daily-coding-surfaces/tasks.md):
   rewind/undo/redo, non-Git snapshots, path-scoped rules and context inspection,
   root structured output and streaming JSONL input.
3. [Binary hex editor](../openspec/changes/add-binary-hex-editor/tasks.md):
   the full reviewed CLI/TUI editor, owner/SDK/model operations and integrations.

On 2026-11-01 the user explicitly requested implementation of [binary editor
issue #94](https://github.com/CyberdyneCorp/cybercode-cli/issues/94) now. This
supersedes the scheduling dependency on completing the first two workstreams;
implement the binary editor while preserving all shared snapshot, permission,
native-save and FileChanged contracts. Remaining plugins/hooks and daily coding
stay tracked in issues #92 and #93. Their separate pending policy decisions are
not selected by this scheduling change. The binary workspace implementation is
partial; its live view does not complete the editor goal.

On 2026-11-03 the user requested merging #105 and moving to [daily coding
issue #93](https://github.com/CyberdyneCorp/cybercode-cli/issues/93). Daily coding
is now the next priority after publishing the already tested manual-draft CLI
recovery increment. This supersedes the old requirement to finish #92 before
starting #93. Plugins/hooks remain incomplete; their pending design decisions
and enforcement guarantees are not implicitly accepted. Remaining binary
retention/checkpoint/rewind integration and UX/performance acceptance are also
unfinished despite the owner's manual closure of #94. Follow the approved daily
coding implementation order, beginning with rewind preview/confirmation; preserve
shared snapshot, permission, recovery and FileChanged contracts.

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

PR #90 merged at `5ed50bff` after all 16 checks passed. Its
[source-bound receipt](../openspec/changes/complete-plugin-hook-runtime/evidence/store-commit-bb91e32d.json)
verifies both commit-boundary cases and runtime lost-ack/replay behavior on
Linux/macOS/Windows, with suite totals and the unverified Windows leaked-process
warning retained. It accepts that correction, not the full producer matrix.
The replay case uses a fresh runtime over the same Store rather than a process
restart.

Continue review on `spec/setup-legacy-admission-review`. The
[legacy Setup owner-resolution candidate](../openspec/changes/complete-plugin-hook-runtime/setup-legacy-bootstrap-review.md)
proposes the missing migration policy and its compatibility exception. The
complete S1 decision is pending; publishing or merging the proposal does not
approve it or implement markers/init. Preserve the separate Linux support and
DirectoryAdded design-review gates. No dependent behavior or native activation
is selected by this documentation follow-up.

Native confinement foundations and regressions exist, but
production enforcement and the remaining plugin/hook scope are incomplete. The
Linux UNIX-socket diagnostic found a real host-reachability gap. Its experimental
filter and runtime checks remain experimental. The broker direction is approved;
complete support acceptance and host activation remain pending. Daily coding now includes reviewed CLI/TUI rewind and the initial private non-Git
snapshot backend (PRs #107–#110). Managed recovery resolution, native
acknowledgement/reclamation, path rules/context, root structured output and
streaming JSONL remain unfinished. Binary editor implementation began
on 2026-11-01 under the scheduling override above and remains partial. The linked task lists remain authoritative for completion.

The [Linux peer-identity diagnostic](../openspec/changes/complete-plugin-hook-runtime/linux-private-peer-identity.md)
confirms that the current broker changes the caller PID observed by a UNIX
listener, despite successful data exchange. The
[kernel UNIX revision](../openspec/changes/complete-plugin-hook-runtime/linux-kernel-unix-revision.md)
is a concrete proposal awaiting review. It does not reduce IPC requirements or
authorize a new production support floor. Preserve this review gate before
production activation or protocol-feature integration.


The current user request prioritizes finishing Recovery/snapshots and Daily coding
as listed in the roadmap. Continue with exact-worker managed recovery, safe cleanup
and native acceptance, then the already approved rules/context, structured-output
and JSONL scope. PR #114 implements immutable Git exclusion history and proposes the
[concrete recovery protocol](../openspec/changes/complete-daily-coding-surfaces/managed-recovery.md)
for the required design review; that protocol is not implemented or approved by
publication alone. Issue #93 was automatically closed by #110 despite remaining
scope and has been reopened. The owner's closed #94 is unchanged. Keep the existing
one-PR delivery constraint and separate Plugins/hooks policy gates.

On 2026-11-06 the user approved the concrete managed-recovery protocol and requested
implementation. Its worker and file-copy journal prerequisites are in progress;
keep/rollback, reconciliation, Git journaling, reclamation and full native/client
acceptance remain required. Approval and current evidence are recorded in the daily
coding change's tasks.md and implementation.md; publishing a prerequisite does not
complete recovery or either issue.


The local pre-effect recovery continuation adds version-2 native source phases,
positive cancellation of an exited published origin before source admission, and
metadata acknowledgement reconciliation. Database and native admission evidence
must both permit cancellation; a database backup cannot erase native admission.
Original intents and payloads remain retained. This is progress on the approved
recovery protocol, with independent checkout ownership, Git journaling, safe cleanup,
complete clients and full platform/cut acceptance still required before delivery.


The next local ownership continuation moves snapshot and recovery checkout claims
from Session IDs to independent native activity IDs through a dedicated host port.
CLI/App retain those locks through settlement; unsupported managed hosts refuse.
Unknown checkout claims remain protected. Exact interrupted-claim proof and
reconciliation are still required before accepting managed resource recovery.


Acquired snapshot checkout claims now carry pinned native route/creation evidence.
The actual worker and consuming Store writer verify and retain that proof; aliases,
replacement and changed records refuse. Binding publication before interrupted
acquisition and recovery-owned native acknowledgement remain required. Git rewind
still follows its legacy path; compatibility tests do not establish owned Git recovery.


The local initialized-publication continuation adds staged no-replace checkout
claims and exact lifetime context. Runtime verifies complete managed bindings before
native settlement; prior writer catalogs refuse mutation. Construction kills retain
evidence and refuse replay. Unknown-claim acknowledgement and reclamation remain
incomplete, and Windows lifetime-bound durability is not yet admitted. This does not
accept the complete recovery feature or either issue.


The local acquired-resource continuation adds exact read-only exited-claim
reacquisition and durable complete checkout references before source dispatch.
Publication failure preserves the unknown reservation; no new native acknowledgement
or cleanup authority is granted. Interrupted-claim consuming receipts and native
reconciliation remain the next recovery work, alongside the existing Git,
reclamation and platform/client acceptance obligations.


Preparation receipts now consume a digest of the complete acquired checkout set,
with actual original claim locks retained through paired commit/reply. CLI/App
reacquire via an explicit read-only host port; lookup remains database-only and
journal metadata reconciliation checks the committed resource binding. Original
native checkout acknowledgement, partial-constructor recovery, Git journaling and
reference-aware cleanup remain required before the complete recovery increment is
reviewable for delivery.


The pre-effect native checkout acknowledgement continuation publishes an exact
receipt-bound disposition while preserving original claim bytes and independent
unknown fences. Complete resource publication now precedes checkout admission
settlement. Candidate journal/checkout receipts are checked before the paired
commit, and interrupted publication resumes only through explicit metadata
reconciliation. Acknowledged creation metadata is retained across authorized
worktree removal/recreation. Partial acquisition/corrupt construction recovery,
post-effect/owned Git integration, reference-aware reclamation and full native/
client acceptance remain open; this does not complete the recovery delivery increment.


The recovery consuming-receipt continuation binds both retained checkout owners to
the durable pair and publishes exact resolution-then-original native disposition
markers only after observing commit. Explicit reconciliation reacquires both
published resource sets without source replay; original claim bytes and legacy
unknown fences remain protected. This is local implementation progress, not full
recovery delivery: actual managed Git source callbacks, partial construction,
reference-aware reclamation and complete native/platform/client acceptance remain
open. See the daily coding implementation evidence and authoritative parent tasks.

The non-default Git candidate now exercises an actual ordinary owned restore in a
managed checkout created under the default application data directory. Source
observation is registration-bound; source intents require the runtime worker's
complete retained native checkout proof. Admission releases the Session mutex for
host policy lookup and fences captured revisions. The App verifies persisted
resources, unchanged user Git metadata and reopened acknowledgement without source
replay. Default Git admission stays off. Managed partial replacement/recovery,
construction/reclamation and complete native/platform acceptance remain required;
this positive does not complete recovery or either issue.
