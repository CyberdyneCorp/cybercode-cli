## 1. Review and prerequisites

- [x] 1.1 Inspect rewind, non-git snapshot, rule/context and structured-output/JSONL contracts and existing reusable machinery at `097b6278`.
- [x] 1.2 Draft proposal/design/scenarios and ordered tasks; identify destructive-revert wording and retry-count inconsistencies.
- [ ] 1.3 Review artifacts with the user, including platform limits, the additive instruction_limits setting, schema/JSONL bounds, multi-input exits and interrupt/EOF semantics.
- [ ] 1.4 Record accepted Plugins and hooks completion before starting this workstream. Establish relevant unchanged-main baseline checks and current clause coverage.

## 2. Rewind and undo/redo

- [ ] 2.1 Reconcile Three-phase revert, Rewind targets, Session branches and structured retry wording in canonical specs and overlapping deltas without deleting historical recovery evidence.
- [ ] 2.2 Add typed CLI rewind and server preview/revalidation/confirmation using existing owned revert operations; regenerate command docs and API/SDK where needed.
- [ ] 2.3 Implement TUI rewind menu, Esc-Esc, `/undo` and `/redo`, composer restoration, explicit busy interruption, conflict/exclusion/effect preview and stale-context fencing.
- [ ] 2.4 Test code/conversation/both, snapshots disabled, inactive branches/export, external edits, busy/interrupt races, restage/clear/restart and retained unknown-effect evidence through actual clients.

## 3. Non-git snapshots

- [ ] 3.1 Add private content-addressed BLAKE3 objects and Location-bound manifests behind the snapshot interface, including ignore/size/metadata reporting and same-size/same-mtime verification.
- [ ] 3.2 Implement conflict-aware owned restore and recoverable backups/journal, preserving Git behavior, protected paths, symlink/hardlink safety and interruption evidence.
- [ ] 3.3 Integrate 200 MiB cap, visible unavailable/overflow state, reference-aware retention and staged/branch/recovery protections.
- [ ] 3.4 Test real non-git editing/restoration, limits, deletes/renames, unsupported files, concurrent Session/editor changes, crashes and native filesystem privacy.

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
