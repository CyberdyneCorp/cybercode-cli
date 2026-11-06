## 1. Review and prerequisites

- [x] 1.1 Inspect rewind, non-git snapshot, rule/context and structured-output/JSONL contracts and existing reusable machinery at `097b6278`.
- [x] 1.2 Draft proposal/design/scenarios and ordered tasks; identify destructive-revert wording and retry-count inconsistencies.
- [x] 1.3 Review artifacts with the user, including platform limits, the additive instruction_limits setting, schema/JSONL bounds, multi-input exits and interrupt/EOF semantics. Approved on 2026-10-27: after PRs #53/#54 merged, the user explicitly requested implementation of both merged plans and then instructed goal resume. Native proof, baseline and acceptance gates remain required.
- [x] 1.4 Record the 2026-11-03 user-approved priority change, establish relevant unchanged-main baseline checks and current clause coverage, and retain unresolved Plugins/hooks dependencies without claiming acceptance. The scheduling prerequisite is superseded. Baseline and current clause coverage are recorded in implementation.md; incomplete native/plugin guarantees remain explicit.

## 2. Rewind and undo/redo

- [x] 2.1 Reconcile Three-phase revert, Rewind targets, Session branches and structured retry wording in canonical specs and overlapping deltas without deleting historical recovery evidence.
- [x] 2.2 Add typed CLI rewind and server preview/revalidation/confirmation using existing owned revert operations; regenerate command docs and API/SDK where needed.
- [x] 2.3 Implement TUI rewind menu, Esc-Esc, `/undo` and `/redo`, composer restoration, explicit busy interruption, conflict/exclusion/effect preview and stale-context fencing.
- [ ] 2.4 Test code/conversation/both, snapshots disabled, inactive branches/export, external edits, busy/interrupt races, restage/clear/restart and retained unknown-effect evidence through actual clients.

Task 2.4 evidence now includes real service kill/reload/restage/clear, disabled snapshots, no-Git conversation continuation/export, explicit interruption and reopened-terminal draft preservation. Native partial-replacement cuts and complete journal/recovery acceptance remain open; see implementation.md.

## 3. Non-git snapshots

- [x] 3.0 Review the concrete [managed-recovery ownership protocol](managed-recovery.md), including exact native acknowledgement, legacy journal limits and two-phase reclamation, before changing recovery admission. Approved on 2026-11-06: "Approve this protocol and implement."

- [x] 3.1 Add private content-addressed BLAKE3 objects and Location-bound manifests behind the snapshot interface, including ignore/size/metadata reporting and same-size/same-mtime verification.
- [ ] 3.2 Implement conflict-aware owned restore and recoverable backups/journal, preserving Git behavior, protected paths, symlink/hardlink safety and interruption evidence.
- [ ] 3.3 Integrate 200 MiB cap, visible unavailable/overflow state, reference-aware retention and staged/branch/recovery protections.
- [ ] 3.4 Test real non-git editing/restoration, limits, deletes/renames, unsupported files, concurrent Session/editor changes, crashes and native filesystem privacy.

### Managed recovery implementation and acceptance map

These steps expand 3.2–3.4 and the remaining native cuts in 2.4. They do not
select the protocol by themselves. The user approved 3.0 on 2026-11-06. The
source map was inspected on main `db9e171e`; this breakdown is not evidence of
recovery implementation.

- [x] M1 Record the user's 3.0 decision in managed-recovery.md and reconcile any requested revisions with the snapshot spec delta before implementation. Approved without protocol revisions on 2026-11-06.
- [ ] M2 Add immutable snapshot-worker origin and request ownership to the runtime activity and snapshot ports. Retain actual lifecycle/checkout/store/mutation owners inside the synchronous worker through its last filesystem call. Test request disposal, cancellation, shutdown and a foreign/live worker; never reuse binary-document or generic-process acknowledgement authority.
- [ ] M3 Add versioned origin-bound Git and file-copy intents, preserved displaced preimages and resolution-intent records. Keep legacy journals inspectable without inventing origin proof. Test missing/corrupt records, private namespace replacement, aliases, metadata and Location mismatch before admitting effects.
- [ ] M4 Implement bounded keep/rollback planning and fingerprint revalidation under exact native locks. Preserve unrelated edits, refuse the complete plan on conflicts, and retain any interrupted resolution as independent evidence. Test keep and rollback after actual partial namespace replacement, including staged undo and inactive branches.
- [ ] M5 Atomically validate and commit the exact origin acknowledgement and resolution receipt at the Store writer. Determine the bounded owner aggregates before extending the single-aggregate append API. Test late CAS/projector/validation failure rollback, absence of uncommitted notifications and committed-but-lost writer replies; repeat request lookup must never mutate files again.

M5 prerequisite: `Store::append_pair_checked` implements the bounded two-owner
transaction with exact sequences, post-commit notifications and rollback/lost-ack
regressions. Native origin validation and resolution request ownership are not
implemented by that primitive; M5 remains open until the runtime uses it with
exact native proof and actual restart/retry acceptance.

- [ ] M6 Publish journal acknowledgement only after observed durable Store settlement. Add reference/reader/lease-aware eligibility and durable two-phase reclamation with retained retry receipts. Test cap pressure, disabled/zero retention, incomplete reference views and crashes at writer/journal/reclaim boundaries without deleting unresolved evidence.
- [ ] M7 Add owner recovery review/confirm/lookup HTTP routes, typed CLI and SDK operations, and TUI recovery entry from rewind unavailability. Regenerate OpenAPI/SDK/command artifacts; exercise the real clients with no inference or transcript movement and with stale/foreign request refusal.
- [ ] M8 Run process-kill cuts before/after intent, stage preparation, every exchange, progress, filesystem completion, writer commit, journal acknowledgement and reclaim. Obtain Linux x64/ARM and macOS Intel/ARM positives/denials, explicit Windows results/limits and shared Git/binary regressions; update evidence and perform substantive code/security review before accepting parent tasks.

Implementation source map:

The non-Git reviewed-confirm path now uses a runtime-issued synchronous worker,
origin-bound version-2 intent and paired Session/native commit followed by journal
acknowledgement. M2–M8 remain open: Git and legacy entry-point integration,
managed keep/rollback and request lookup, resolution receipts, lost-reply journal
reconciliation, cleanup and complete native/platform acceptance are still required.

Before rollback admission, M3 must retain exact per-change source/staged file and
parent identities, including creation evidence for any new parents. Version-2
worker origin binds the activity and journal, but file content hashes/mode alone
cannot prove that a current same-byte replacement still belongs to that restore.
Do not infer per-file rollback ownership from the worker binding or from missing
progress. This prerequisite remains open with the complete native crash cuts.
Crash acceptance must also cover reservation before origin publication and origin
publication before intent publication. Those durable fences cannot be force-cleared
or resolved by inventing a missing journal; implement exact pre-effect ownership
proof/reconciliation for these cuts before claiming complete managed recovery.

- `cyber-server/src/runtime/activity.rs` owns native activity records; `binary_documents.rs` retains a real synchronous worker's resources and `binary_recovery.rs` demonstrates document-specific recovery proof. Snapshot workers require their own origin/proof rather than calling binary recovery.
- `cyber-server/src/runtime/host.rs::Snapshots` currently exposes restore and read-only recovery inspection; `rewind_preview.rs` owns reviewed confirmation. A dedicated owner recovery admission must work while ordinary admission remains fenced.
- `cyber-snapshot/src/copy/restore.rs` reads legacy version-1 intents and writes worker-bound version-2 intents for the owned path; `copy/storage.rs` owns private descriptor-relative I/O. `restore.rs` and `git_history.rs` cover Git restore and historical exclusions. Keep historical evidence readable as the new protocol is added.
- `cyber-store/src/store.rs::append_checked` currently appends one aggregate. Separate independent appends cannot establish atomic acknowledgement plus resolution ownership.

## 4. Rules and context inspection

- [ ] 4.1 Implement trusted path-scoped rule discovery/precedence, validated globs, overrides and bounded instruction assembly without changing `instructions` array compatibility.
- [ ] 4.2 Bind reminder delivery to successful authorized operations and Context Epoch identity, including edits/patches, partial settlements, compaction/restart and changed/revoked rules.
- [ ] 4.3 Add authenticated read-only context inspection and `/context`, source estimates/origins/status/pending updates and honest utilization; synchronize OpenAPI/SDK and doctor diagnostics.
- [ ] 4.4 Test no rule leakage across Location/Epoch, denied/failed calls, truncation, untrusted sources, stale response and token-count consistency with actual request assembly.

## 5. Root structured output

- [ ] 5.1 Add validated per-prompt root format admission, bounded schema compilation and durable schema/attempt ownership without changing existing child contracts.
- [ ] 5.2 Add `structured_output` finalization, two correction retries, budget/interrupt handling and durable successful/error result settlement.
- [ ] 5.3 Add `--output-schema`, API/SDK result fields and text/JSON/stream rendering; preserve unstructured commands and validate option combinations before admission.
- [ ] 5.4 Test valid/invalid schemas, malformed results, retry exhaustion, missing finalizer, compaction, restart/retry identity, budget exits and independent queued prompts with a real server/scripted provider.

## 6. Streaming JSONL input

- [ ] 6.1 Define typed version-2 envelopes, correlation, resource bounds, aggregate exit rules and driver-owned interactive request lifetime; document compatibility and unsupported combinations.
- [ ] 6.2 Implement concurrent bounded parsing/admission and exact owned permission/question replies, retained request lookup, interrupt and end/EOF draining without stopping unrelated work.
- [ ] 6.3 Emit per-input results, optional partial/hook events and atomic last-message output with bounded backpressure and secret-safe errors.
- [ ] 6.4 Test multiple inputs, steering/queueing, malformed/oversized input, capacity, stale/cross-request replies, EOF while awaiting input, slow stdout, SIGINT, server restart and lost admission acknowledgement.

## 7. Review and delivery

- [ ] 7.1 Update guides, roadmap/feature evidence, original task lists and generated API/SDK/CLI artifacts for actual delivered behavior.
- [ ] 7.2 Run affected Rust tests, format/Clippy, SDK typecheck/tests, TUI/real-binary acceptance, strict OpenSpec/lint and relevant native/platform CI. Record skips and unperformed external checks explicitly.
- [ ] 7.3 Use the code-review skill for substantive review, address findings and deliver reviewable PRs. Merge only when authorized; archive after accepted merge and overlap reconciliation.
- [ ] 7.4 Verify all approved first- and second-workstream outcomes against evidence before marking the combined goal complete.
