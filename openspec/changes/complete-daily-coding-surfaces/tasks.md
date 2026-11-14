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

Runtime-construction disposal now has the same ordered ownership as a completed
worker. Ordinary/recovery checkout-acquisition failures and requester cancellation
retain activity completion and admission until gated lifetime disposal finishes,
with durable uncertainty already recorded. The regression fails on the previous
order; all four entry-point/disposal cases pass after the correction. Native
partial-journal construction and complete crash/platform acceptance still leave
M2 open.

- [ ] M3 Add versioned origin-bound Git and file-copy intents, preserved displaced preimages and resolution-intent records. Keep legacy journals inspectable without inventing origin proof. Test missing/corrupt records, private namespace replacement, aliases, metadata and Location mismatch before admitting effects.
- [ ] M4 Implement bounded keep/rollback planning and fingerprint revalidation under exact native locks. Preserve unrelated edits, refuse the complete plan on conflicts, and retain any interrupted resolution as independent evidence. Test keep and rollback after actual partial namespace replacement, including staged undo and inactive branches.
- [ ] M5 Atomically validate and commit the exact origin acknowledgement and resolution receipt at the Store writer. Determine the bounded owner aggregates before extending the single-aggregate append API. Test late CAS/projector/validation failure rollback, absence of uncommitted notifications and committed-but-lost writer replies; repeat request lookup must never mutate files again.

M5 implementation now uses `Store::append_pair_checked` for the exact original
native owner and the resolution request owner. A Session-creation/request-derived
reservation precedes native work, and the consuming writer checks captured
recovery authority, current Session revision, both immutable origins and the exact
request/receipt. A WAL-hook reply-loss regression reopens the actual Store and
finds both committed owners without notifications or replay. M5 remains open for
complete failure/reconciliation and native acceptance; the primitive alone did not
satisfy these obligations.

M4/M7 continuation: review, confirm and request lookup HTTP/SDK operations now
exist for the file-copy backend. Dedicated snapshot-only admission preserves
captured closed boundaries; ordinary native/hook admission is fenced at the writer.
Actual CLI/API service-kill cases resolve a partial namespace with keep and
rollback, preserve unrelated/later owner files and the transcript, and repeat only
lookup after restart. This is local non-Git evidence, not complete operator/client
or platform acceptance. Typed CLI/TUI recovery, pre-origin cuts and Git-native equivalents remain required. Explicit recovery-request
acknowledgement reconciliation now uses the exact committed pair and persisted
immutable resolution digest; it changes only journal metadata under retained native
locks and never rescans or reinstalls source files.
Ordinary rewind commit still refuses recovery receipts. Filesystem completion alone
never clears a native fence. Created source directories remain protected for safe
acknowledged cleanup.

Late original-scope disposal now checks durable identity and status within the
writer transaction after native store-lock release. It preserves an already
acknowledged activity and its complete receipt; changed ownership/sequence refuses
the append. The consuming recovery pair now removes only its exact snapshot
reason after observed durable commit, preserving opaque/generic Location
uncertainty. Snapshot uncertainty is tracked
by exact activity ID separately from the opaque flag; reload reconstructs generic
fences and snapshot owners independently. A late-scope no-op clears only its exact
snapshot reason after observing the matching durable acknowledgement at the writer.
Explicit recovery-request reconciliation distinguishes a durable writer
acknowledgement with missing journal acknowledgement from an uncommitted filesystem
outcome. Regression coverage refuses the latter before calling the native adapter,
and preserves later owner edits, including a source replaced with a symlink.
Older receipts without an immutable resolution digest cannot authorize new journal
writes. New ordinary file-copy restores now persist a separate immutable intent/Session-batch
receipt in the paired append. Its lookup and metadata-only acknowledgement path
checks the exact batch without replay. Historical settlements lacking that proof
cannot authorize new journal writes.
Unprepared reservation/origin/intent cuts still require exact resource ownership
proof; missing records alone do not license acknowledgement.

- [ ] M6 Publish journal acknowledgement only after observed durable Store settlement. Add reference/reader/lease-aware eligibility and durable two-phase reclamation with retained retry receipts. Test cap pressure, disabled/zero retention, incomplete reference views and crashes at writer/journal/reclaim boundaries without deleting unresolved evidence.
- [ ] M7 Add owner recovery review/confirm/lookup HTTP routes, typed CLI and SDK operations, and TUI recovery entry from rewind unavailability. Regenerate OpenAPI/SDK/command artifacts; exercise the real clients with no inference or transcript movement and with stale/foreign request refusal.
- [ ] M8 Run process-kill cuts before/after intent, stage preparation, every exchange, progress, filesystem completion, writer commit, journal acknowledgement and reclaim. Obtain Linux x64/ARM and macOS Intel/ARM positives/denials, explicit Windows results/limits and shared Git/binary regressions; update evidence and perform substantive code/security review before accepting parent tasks.

Lifetime continuation: new file-copy runtime reservations now retain an exact
worker-specific private lock acquired before reservation and checkout admission.
The actual worker keeps it through resource teardown, and source authorization
revalidates its native route. Lock metadata and orphan stages count toward the cap
and conservatively protect GC. Native tests reacquire only after actual process
death and preserve an interrupted pre-publication stage. This is a prerequisite,
not completion of all pre-origin recovery. The following continuation adds the
consuming path only for a positively bound reservation with no published origin.
Positive source-admission phases for origin-before-intent, independent managed-
checkout proof and Git equivalence remain open; no force-clear is enabled.

Implementation source map:

The non-Git reviewed-confirm path now uses a runtime-issued synchronous worker,
origin-bound intent and paired Session/native commit followed by journal
acknowledgement. The recovery path adds globally reserved request ownership and
paired original/resolution settlement. M2–M8 remain open: Git and legacy entry-point
integration, pre-origin resource proof, cleanup,
complete clients and native/platform acceptance are still required.

New version-3 file-copy intents retain native per-change source/staged file and
parent identities, including privately staged identities for new parents before
publication. Read-only inspection classifies actual source/private slots without
using progress as proof; five storage-level process-kill cuts verify this after
fresh ownership. Legacy version-1/2 intents have no historical per-file proof.
Do not infer rollback ownership from matching bytes or missing progress. Git
equivalence, the remaining runtime/crash boundaries and the complete native platform
matrix remain open with M3/M8. Two real non-Git CLI/API scenarios now kill the
service after a partial exchange, restart, keep/rollback and verify lookup without
replay; these do not cover every cut or reclamation.
Crash acceptance must also cover reservation before origin publication and origin
publication before intent publication. Those durable fences cannot be force-cleared
or resolved by inventing a missing journal; implement exact pre-effect ownership
proof/reconciliation for these cuts before claiming complete managed recovery.

- `cyber-server/src/runtime/activity.rs` owns native activity records; `binary_documents.rs` retains a real synchronous worker's resources and `binary_recovery.rs` demonstrates document-specific recovery proof. Snapshot workers require their own origin/proof rather than calling binary recovery.
- `cyber-server/src/runtime/host.rs::Snapshots` exposes restore, recovery inspection and owned resolution; `rewind_preview.rs` owns reviewed rewind confirmation. `snapshot_recovery.rs` owns durable recovery admission/lookup, with snapshot-only captured authority and the paired native consuming commit in `activity.rs`.
- `cyber-snapshot/src/copy/restore.rs` reads legacy version-1/2 intents and writes version-3 native ownership records. `copy/ownership.rs` prepares file/parent evidence and inspects actual slots; `copy/storage.rs` owns private descriptor-relative I/O. `restore.rs` and `git_history.rs` cover Git restore and historical exclusions. Keep historical evidence readable as the new protocol is added.
- `cyber-snapshot/src/copy/resolution.rs` freezes native observations, reads retained preimages, publishes independent intents and rejects request replay. Acknowledged child journals settle incomplete parents through validated links without inventing original completion. The retained worker guard pins immutable intent/receipt data. Origins bind the exact private lock inode; older origins without that proof remain inspectable and fenced.
- `cyber-store/src/store.rs::append_pair_checked` commits the bounded original/resolution owner pair. Independent single-owner appends cannot establish this atomic acknowledgement.

## 4. Rules and context inspection

- [ ] 4.1 Implement trusted path-scoped rule discovery/precedence, validated globs, overrides and bounded instruction assembly without changing `instructions` array compatibility.
  - `cyber doctor` now diagnoses rule discovery with the runtime loader: it counts discovered path-scoped definitions without adding them to the baseline it lists, and reports held, read-denied, invalid, compatibility-disabled and bound-exceeded definitions by reason, honouring `compat.claude` (see 4.3 for the tests). Directory precedence is now covered through the real App: from a nested Location the baseline holds the root's ordinary files, sorted `.claude/rules`, sorted `.cyber/rules`, then the same order for the nested directory, and `compat.claude: false` leaves every `.claude/rules` definition out (`rule_directories_load_claude_then_cyber_rules_root_before_nested`). Platform acceptance and the remaining 4.1 obligations stay open.
- [ ] 4.2 Bind reminder delivery to successful authorized operations and Context Epoch identity, including edits/patches, partial settlements, compaction/restart and changed/revoked rules.
  - Settlement now fences each delivered project rule to the approval it was discovered under: under the Session settlement lock (group settlement and exact `/approve` replay) the runtime re-resolves the Location's rule gate and drops the whole nested reminder, keeping the tool result and recording no `rule_reminders`/`reminded_rules`, unless every project binding still authorizes with the same checkout root, approval generation and definition digest (`RuleGate::still_authorizes`; owner-global bindings pass). A revocation between discovery and commit therefore neither delivers nor consumes the rule, and a later matching read after re-approval delivers it once (`a_rule_revoked_while_its_read_settles_is_neither_delivered_nor_consumed`, through the `Runtime::configure_settlement_probe` test seam). Earlier slices cover denied/failed operations, edits, partial patches, compaction and changed bytes. Each delivered binding now persists in the Epoch (`Epoch::delivered_rules`, replayed from `rule_reminders`, with the definition's size/modification time recorded at delivery), so inspection reports a rule revoked or changed after delivery (see 4.3). Restart and replay are now covered end to end over a file database with a second App: the restarted server rebuilds `reminded_rules` from durable settlements and keeps the Epoch, so a matching read after restart gets no second reminder (`a_delivered_scoped_rule_is_not_redelivered_after_server_restart`); a read cut off by the restart is recovered as `interrupted` and consumes nothing, and replay ignores even an interrupted settlement carrying a binding and proof (`a_read_interrupted_by_restart_does_not_consume_its_rule`); an approval withdrawn while the server was down suppresses delivery after restart until re-approval (`a_rule_revoked_while_the_server_was_down_is_not_delivered_after_restart`). No defect was found. Remaining: withdrawal of a rule already delivered in the current Epoch when it is later revoked (inspection only reports it), and platform acceptance.
- [ ] 4.3 Add authenticated read-only context inspection and `/context`, source estimates/origins/status/pending updates and honest utilization; synchronize OpenAPI/SDK and doctor diagnostics.
  - The Session endpoint and TUI panel expose stored source groups and an estimate based on request replay. The default response omits stored text. Individual source metadata now persists with Epoch creation and ordinary observation, including retained-unavailable state, bounded file provenance, historical unknowns and deferred output-style updates. The last prepared request now records the complete assembly estimate, including callable schemas and turn instructions, for comparison with actual provider input. Inspection now also reports instruction files edited or removed since they were read: each file's size and modification time are recorded from the handle it was read through, and inspection compares them with current metadata (bounded to 512 files, no file opened), marking each source and counting changed sources in the API and TUI. Sources without complete stamps report unknown; files added since the read are found only at the next request. Inspection also gives a next-request estimate: the stored projection plus the last prepared request's tool schemas, listing its assumptions (tool schemas from the last request, changed model, pending Epoch rebuild, sources changed on disk, excluded turn-only instructions); it is absent before the first prepared request. It is not an exact projection: tool visibility and configuration are resolved only at the next request boundary, which inspection does not run. Inspection and the TUI panel now list up to 128 rules delivered in the current Epoch (path, root, 12-digit digest prefix, approval generation, omitted count) with an `approval` of current/changed/revoked/unknown, computed from the host's trust approval record and the definition's file metadata without opening it; a changed or revoked rule is counted and adds the `delivered_rules_changed` next-request assumption (`inspection_lists_delivered_rules_and_marks_revoked_approval_without_reading_them`, `inspection_reports_a_delivered_rule_as_revoked_after_its_approval_is_withdrawn`, TUI `inspection_shows_revoked_delivered_rules`, HTTP default response without rule bodies; OpenAPI/SDK regenerated). Inspection now also reports withheld definitions (scenario "Inspection of an untrusted source"): at the ordinary request boundary the instruction loader records, as typed entries rather than only warnings, each rule definition or refused rule directory it left out, with reason `trust_held`, `read_denied`, `invalid_frontmatter`, `compat_disabled` or `bound_exceeded`, bounded to 64 per source plus an omitted count and with no sizes; they persist in the source metadata and are listed as `withheld_sources`/`omitted_withheld_sources` in the API and under "Withheld definitions" in the TUI panel (`inspection_reports_held_rule_definitions_without_reading_or_counting_them`, which makes the held file unreadable before inspecting, checks no events are appended and that the next-request estimate is the same for a 64 KiB and a tiny held body; unit `withheld_rule_definitions_are_recorded_with_their_reason`, `a_refused_rule_directory_is_withheld_as_a_whole`; HTTP default response lists only `source`/`path`/`reason`; TUI `inspection_shows_withheld_definitions_without_sizes`; OpenAPI/SDK regenerated). Withheld non-rule instruction files (outside the project, refused by read rules) and refused imports of ordinary files remain warnings only. `cyber doctor`'s `instructions` check now reuses the same typed entries: it counts valid path-scoped definitions discovered but not attached to the baseline (`Instructions::scoped`) and lists withheld definitions by reason without their text or a duplicate warning, warning when any is withheld; with `compat.claude` false, `.claude/rules` definitions are neither counted nor listed, and one reached by import is listed as `compat_disabled` (`doctor_warns_about_held_and_invalid_rule_definitions`, `doctor_counts_scoped_rules_without_adding_them_to_the_baseline`, `doctor_respects_compat_claude_false_for_claude_rules`). Platform acceptance remains open.
- [ ] 4.4 Test no rule leakage across Location/Epoch, denied/failed calls, truncation, untrusted sources, stale response and token-count consistency with actual request assembly.
  - A scoped rule delivered in one Epoch is delivered again, once, by the first matching read after compaction starts a new Epoch (`a_scoped_rule_is_delivered_again_in_the_epoch_after_compaction`). Earlier cases already cover denied reads, partial patches, nested checkouts, held/untrusted definitions and suppressed delivery when the Epoch or Location changes while a read settles. A trust revocation between rule discovery and commit suppresses delivery without consuming the rule (`a_rule_revoked_while_its_read_settles_is_neither_delivered_nor_consumed`; unit `a_binding_from_a_revoked_generation_is_not_authorized`). Restart does not leak or re-deliver rules: delivered rules stay consumed in the same Epoch after restart, a read interrupted by restart leaves its rule eligible, and a revocation while the server was down holds the rule after restart (the three `*_restart` tests in `crates/cyber-app/tests/instructions.rs`). Untrusted sources are now also covered at inspection: held, read-denied, invalid and compatibility-disabled definitions are reported as withheld without being opened by inspection or counted in any estimate (see 4.3). Location and Session isolation are now covered in `crates/cyber-app/tests/instructions.rs`: after the Session enters a worktree, the origin's delivery and approval neither suppress nor supply the worktree's copy of the rule (held until that checkout is approved, then delivered once with the worktree as its root), and an origin-only rule never reaches the moved Session through a read of the origin file (`a_rule_delivered_in_one_location_is_not_reused_after_the_session_moves`); a child Session and a fork cut before the delivering read start with no delivered rules, deliver the rule once in their own Epoch and leave the parent's record unchanged (`a_child_or_forked_session_does_not_inherit_parent_rule_delivery`; a full fork copies the Epoch together with the history that carries the reminder, by design). Truncation: with 20 matching rules under a 512-byte budget each reminder records exactly the rules it labels, as a delivered body or a whole entry of its bounded read list, unlisted rules stay eligible and later reads label each once (`a_truncated_rule_reminder_records_only_labelled_bindings`). Token-count consistency: rebuilding the provider-neutral request from the scripted provider's actual body (system parts, messages, tool definitions) gives exactly the `last_prepared_request` estimate, for the request carrying the rule reminder and the next one; the idle `next_request` estimate equals the following request without its new prompt, rounded in two parts (stored projection sized with an empty tool list, plus the last request's schema estimate), so it is within one token of the request sent (`inspection_estimate_matches_the_assembled_request_with_rule_reminders`). These need the real tool host's native read receipts, so they live in the App suite rather than the `cyber-server` harness. No defect was found. Platform acceptance remains open.

## 5. Root structured output

- [x] 5.1 Add validated per-prompt root format admission, bounded schema compilation and durable schema/attempt ownership without changing existing child contracts.
- [x] 5.2 Add `structured_output` finalization, two correction retries, budget/interrupt handling and durable successful/error result settlement.
- [x] 5.3 Add `--output-schema`, API/SDK result fields and text/JSON/stream rendering; preserve unstructured commands and validate option combinations before admission.
- [ ] 5.4 Test valid/invalid schemas, malformed results, retry exhaustion, missing finalizer, compaction, restart/retry identity, budget exits and independent queued prompts with a real server/scripted provider. Local functional cases pass: schema bounds, admission refusal, malformed/invalid submissions, retry exhaustion, missing finalizer, compaction during correction, independent queued schemas, shutdown/resume, interruption/budget outcomes, terminal writer cuts, post-commit acknowledgement loss, steering ownership, and CLI budget/timeout exit 4 and Unix SIGINT exit 130. Current-source platform acceptance remains required; this task is not accepted from local results alone.

## 6. Streaming JSONL input

- [x] 6.1 Define typed version-2 envelopes, correlation, resource bounds, aggregate exit rules and driver-owned interactive request lifetime; document compatibility and unsupported combinations.
- [ ] 6.2 Implement concurrent bounded parsing/admission and exact owned permission/question replies, retained request lookup, interrupt and end/EOF draining without stopping unrelated work. Basic parsing/admission merged in #117. Local continuations retire completed payloads and bind direct model requests to the complete recorded input cohort. Background-descendant and hook-ask ownership are implemented below; foreground-descendant ownership, broader event/result correlation and remaining resource bounds still need implementation and acceptance. Direct input presence is implemented below; full platform acceptance remains required. Direct scoped interruption integration is recorded below. Direct Job/handback retirement is implemented below; this task remains open.
  - Process and subagent Jobs now persist the original dispatched step/call, attempt, operation key and complete input cohort. Direct/detached legacy calls retain unknown provenance. The CLI now uses this proof for bounded direct Job/handback retirement; descendant ownership and scoped cancellation remain open.
  - `POST /jobs/input-state` now aggregates running/unadmitted-handback state at the Store into at most 32 small records. Complete valid cohorts must belong to the supplied Session/input set; mixed, missing or malformed proof grants no inferred ownership. Authenticated HTTP, bounds/read-only, actual background dispatch/handback and large-payload lookup regressions pass. The query now also tracks pending handback steps and the observed Session revision; CLI adoption of those barriers is recorded below. Scoped control integration is recorded below.
  - Handback admission and notifier acceptance now commit together before inference; rejected notifier writes roll back both. Exact retry reconciles historical split admissions. Request/nested-Job input provenance resolves through the earlier matching admission/notification and immutable original Job cohort, including new steering and bounded union. Six provenance tests, 14 Job and 22 request integration tests and 26 actual process/subagent cases pass, including an owned permission after handback and a nested running Job retained by the original input query. CLI retirement and step correlation are implemented below; broader correlation and descendant scoped cancellation remain open.
  - The CLI now adopts bounded input-owned Job state and exact stored-step provenance instead of capturing every newly appearing Session Job. Pending slots survive running Jobs, unadmitted notices, unfinished handback steps and unread durable revisions. A real-binary gated process/handback regression passes, and actual shared-Session controllers prove revision fencing and retirement while another driver's question stays active. An owned replay/live Session connection preserves the durable cursor and supports idle/delta notices through opt-in `live=true`. Root/handback retirement and step-text/failure correlation are implemented locally; input-scoped presence, descendant/hook controls, broader correlation and restart/lost-ack acceptance remain open.
  - At `34033f9d`, runtime-only retained input interruption commits intent before signalling an exact owned Drain, withdraws direct owned requests before waiting for disposal, refuses queued inputs and authenticated late handbacks, and cancels proven direct process Jobs in pages of 64 identities. Exact retries cannot retarget; missing owners/runtime generations report unknown. Mixed/descendant ownership remains unresolved. Four unit and nine runtime regressions, 469 server library tests (one subprocess fixture ignored), 14 Job, 22 request and 84 HTTP cases pass locally. HTTP/SDK/JSONL integration follows below; scoped presence, descendant control and full platform acceptance remain open.
  - Authenticated HTTP and generated SDK controls now expose retained input interruption with a committed receipt revision. JSONL interrupt/SIGINT/fatal-error handling polls the same original selection, fences retirement by consumed revision, bounds network/retained controls and preserves newer inputs/replies without global fallback. Six actual shared-Session controller cases and 18 real binary cases pass, including inference interruption plus later admission, fatal input during inference, and Unix owned background SIGINT exit 130. All 86 HTTP, 469 server library (one subprocess fixture ignored), nine runtime control and 105 CLI unit cases pass; SDK typecheck and 138 tests pass with ten configured skips. Presence, descendant/hook control, complete mixed ownership, resource bounds and full native/platform/restart acceptance remain open.
  - Direct input-client presence now uses separate expiring leases and atomic admission bindings, with observational JSONL event streams. Exact retry cannot rebind, restart cannot recreate live authority, missing scoped metadata cannot fall back to legacy attendance, and expiry/reply/cascade checks preserve other clients' requests. Local verification passes 472 server library (one subprocess fixture ignored), six input-client, ten browser, 22 request, nine interruption, 16 Goal, 88 HTTP, 19 client library, 105 CLI unit and 18 real JSONL cases; SDK typecheck and 139 tests pass with ten configured skips. Full descendant/hook ownership, mixed-cohort behavior, protocol restart/lost-ack and platform acceptance remain open. Native CI prerequisite failure is now explicit and dependent proofs require successful sandbox setup; this grants no acceptance from skipped cases.
  - Descendant request provenance: a permission/question request from a child Session now carries `root_session_id` and `root_input_message_ids` (durably in `permission.asked`/`question.asked`, in pending/routed requests, OpenAPI and the TS SDK) when every link up to the root is a running background subagent Job whose immutable `input_origin` is complete, bounded and identical across that link's running Jobs; a settled or restart-interrupted Job lends nothing, so a later foreground resume of the child is unstamped and a background resume uses the new running Job's cohort (`runtime/requests/descendant_inputs.rs`, at most 32 links and 8 Jobs per link). Scoped attendance (`input_clients::request_attendance`) then uses the root cohort: only the live input client bound to it at the root Location keeps the request waiting, accepts its reply and drives Goal unattended-ask timing; release/expiry settles it unattended without borrowing browser/stream/other-client attendance. Foreground/detached, mixed, malformed, oversized or cyclic lineage stamps nothing and keeps existing attendance. Evidenced by `a_subagent_request_carries_the_parents_input_cohort`, `a_descendant_request_waits_only_for_its_bound_input_client`, `mixed_or_detached_descendant_requests_grant_no_ownership` `descendant_cohort_survives_restart_as_provenance_not_attendance` (three fail without the stamp) and `a_foreground_resume_after_the_background_job_settled_inherits_no_cohort` (fails without the running-Job check, as does the restart case), three unit tests for complete/mixed/malformed/cyclic lineage and settled/resumed Jobs, 479 server library, 666 ci-profile server (lib + input-client, request, child-ownership, HTTP, Job, Goal, interruption, interactive-client, team suites), 12 background-subagent tool and 30 real JSONL cases; SDK typecheck and 139 tests pass with ten configured skips. Open: the JSONL driver does not yet claim or answer descendant requests, foreground subagents (no Job) carry no root cohort, descendant scoped interruption/cancellation, hook presence and full platform acceptance.
  - JSONL descendant/hook ownership: the driver (`exec/driver.rs`, `exec/inputs.rs` `request_cohort`) now claims a permission/question routed to its Session from a child Session when `root_session_id` is its Session and every `root_input_message_ids` identity is one of its pending inputs; the line keeps the run's `session_id` and adds `request_session_id` (the asking child), the retained Ask posts the reply to that child Session, an input stays pending while an Ask it owns waits (on top of the running-Job/handback hold), and EOF/end releases the lease (settling bound descendant asks unattended) then declines the rest. A `PreToolUse` hook `ask` opens an ordinary step-cohort request and is owned like a direct ask. Bug fixed: durable/routed `permission.replied`/`question.replied` carry `request_id`, not `id`, so a request answered elsewhere was never dropped from the driver's asks (regression `a_request_answered_elsewhere_leaves_the_drivers_asks`, fails without the fix). Evidenced by real-binary `a_subagent_permission_is_answered_over_stdin`, `eof_declines_a_subagent_question_and_settles_the_input`, `a_hook_ask_is_owned_by_the_driver` and, against a real `cyber serve`, `a_second_driver_neither_sees_nor_answers_the_first_drivers_subagent_request` (all four fail without the driver change), plus the unit test `a_descendant_request_is_owned_only_through_its_complete_root_cohort`; 34 real JSONL, 17 delegation, 18 lifecycle-hook and 117 CLI unit cases pass under nextest ci. Open: foreground subagents (no Job, no root cohort) are still not claimed, descendant scoped interruption/cancellation, descendant hook receipts in `--include-hook-events`, and full platform acceptance.
- [ ] 6.3 Emit per-input results, optional partial/hook events and atomic last-message output with bounded backpressure and secret-safe errors. `--include-hook-events` is implemented without putting receipts on the Session stream: the driver (`exec/hook_events.rs`) pages the authenticated `GET /sessions/{id}/hook-executions` (50 per page, at most 4 pages per read) after each owned tool call settles, at idle and once before `end`, stopping at a receipt older than the run (less 60 s clock allowance) or at an already reported one while no owned receipt is still running, and emits one `hook_event` line (receipt, hook id, event, call, tool, outcome, decision kind, duration, owning `inputs` `id`/`message_id`) per settled receipt whose `call_id` was called by an owned step, deduplicated by a bounded (1024) seen-set; hook IO and hook-supplied reasons are never included. Evidenced by the real-binary `include_hook_events_reports_owned_tool_hook_outcomes` (a PreToolUse deny on `read` is reported once with its input correlation while the Session-scoped UserPromptSubmit receipt is not; no lines without the flag), `include_hook_events_needs_stream_json_input` and the `unsupported_combinations_exit_2_before_any_session` case, and unit tests for dedupe, running-then-settled receipts, the page bound and run-start stop. Not reported: Session-scoped receipts without a `call_id` (SessionStart, UserPromptSubmit, Stop and similar), receipts of descendant Sessions, and receipts beyond 200 newer ones in a single read. Secret-safe errors: every JSONL line's `error`/`message` field is passed through the built-in `cyber-core` redactor (`exec/report.rs` `secret_safe`) before it is written, so provider error bodies, server/background reconciliation errors and echoed stdin in protocol errors show `[REDACTED:<kind>]` instead of the value; evidenced by the real-binary `jsonl_errors_do_not_echo_secrets` (a provider HTTP 400 body with `TOKEN=` and an AWS key, then a protocol error echoing an `api_key=` type, checked across `error`, `result` and `end`) and the unit test `error_result_and_end_fields_never_carry_a_secret_literally`. Open: user-configured redaction patterns and known config/env secrets are not applied on this path (built-in detectors only), text-mode stderr is unchanged, and the server still records the raw provider message in `session.step.failed`.
- [ ] 6.4 Test multiple inputs, steering/queueing, malformed/oversized input, capacity, stale/cross-request replies, EOF while awaiting input, slow stdout, SIGINT, server restart and lost admission acknowledgement. Done: multiple inputs in order, per-input structured results, structured/plain steering while inference is gated, long-run slot retirement, malformed and oversized input (exit 2), a 33rd pending input refused as a protocol error (exit 2) with every admitted input still settled, a stalled stdout reader that delays but neither loses nor reorders results, unknown request replies, EOF while a request waits, unsupported combinations, with the real binary (`cyber-cli/tests/exec_stream_input.rs`). Stale, wrong-kind and cross-driver replies are refused with a `protocol` error line while the run goes on: a second reply to an already answered permission (`a_second_reply_to_an_answered_request_is_a_protocol_error`), a `question_reply` naming a permission request, which leaves it pending so the correct `permission_reply` still runs the tool once (`a_question_reply_naming_a_permission_does_not_consume_it`), and, against a real `cyber serve`, a second driver attached to the same Session and Location that never sees the first driver's `permission_request` and whose reply to it is refused while the first driver still answers it (`a_second_driver_cannot_reply_to_the_first_drivers_request`). A lost admission acknowledgement is covered against a real `cyber serve` through a proxy that commits each prompt but drops its response: after the client's three same-key retries also lose theirs, the driver resends the same message identity and the input is admitted, answered and sent to the model once (`a_lost_admission_acknowledgement_is_retried_without_a_second_prompt`). When the retried admission loses its response too, the driver reads the Session inbox: the committed input is reported admitted and answered once (`an_admission_whose_retry_is_also_lost_is_confirmed_from_the_inbox`), and an unreadable inbox ends the run with exit 2 instead of a false `refused` (`an_unconfirmable_admission_fails_the_run`). A server killed under an attached driver ends the run with exit 2 (`the server closed the event stream`), settles the admitted input as unanswered without hanging, and the restarted server still holds that input in the Session inbox (`a_server_crash_ends_the_run_explicitly_and_the_restarted_server_keeps_the_input`); the driver does not reconnect across a restart. Descendant and hook-originated requests: a background subagent's permission answered over stdin (`a_subagent_permission_is_answered_over_stdin`), EOF settling a subagent's question and the input (`eof_declines_a_subagent_question_and_settles_the_input`), a `PreToolUse` hook `ask` owned by the driver (`a_hook_ask_is_owned_by_the_driver`) and a second driver that neither sees nor answers the first driver's subagent request (`a_second_driver_neither_sees_nor_answers_the_first_drivers_subagent_request`). Open: foreground-subagent ownership, descendant interruption, and continuing an attached run across a server restart if that is wanted. Current local evidence includes 34 real JSONL cases (including secret-safe error, result and end lines), Unix owned background SIGINT and seven actual shared-Session controller tests; foreground-descendant ownership, complete mixed-cohort behavior, full restart/lost-ack and platform acceptance remain open.

## 7. Review and delivery

- [ ] 7.1 Update guides, roadmap/feature evidence, original task lists and generated API/SDK/CLI artifacts for actual delivered behavior.
- [ ] 7.2 Run affected Rust tests, format/Clippy, SDK typecheck/tests, TUI/real-binary acceptance, strict OpenSpec/lint and relevant native/platform CI. Record skips and unperformed external checks explicitly.
- [ ] 7.3 Use the code-review skill for substantive review, address findings and deliver reviewable PRs. Merge only when authorized; archive after accepted merge and overlap reconciliation.
- [ ] 7.4 Verify all approved first- and second-workstream outcomes against evidence before marking the combined goal complete.


Reserved-before-origin continuation: list/review/confirm/lookup operations now
consume the exact exited worker lifetime proof and atomically pair only that native
acknowledgement with a retained cancellation request. A real service kill before
origin publication preserves later symlink/owner edits and the transcript, supports
retry after restart and admits a subsequent prompt in an unmanaged non-Git Location.
Cancelling an unstarted resolution reports `cancelled_before_origin` without
acknowledging its original failed restore. Origin-before-intent, independent checkout
resources, complete clients, Git, reclamation and the remaining M8 cuts remain open.


Pre-effect origin continuation (2026-11-06): new file-copy workers publish version-2
native source-phase proof before their stable lifetime reservation. A durable
activity admission marker and native true phase precede the first source callback;
missing/legacy phase proof and database/native disagreement refuse cancellation.
The owner cancellation now accepts an exited published origin only with explicit
unadmitted evidence, commits its paired receipt, then publishes bounded journal
cancellation metadata. Explicit preparation acknowledgement reconciles that metadata
without source effects. Original intents and private preparations remain retained.
Local native missing-intent and real-service origin-before-source-admission cuts
are covered; the real-service cut does not assert that intent publication was absent.
Independent checkout resource ownership, Git journaling, cleanup, complete clients
and the remaining M8 cut/platform matrix still prevent acceptance of M2–M8 and
parent tasks 2.4/3.2–3.4. No additional parent task is marked complete.


Independent checkout acquisition continuation: ordinary snapshot and recovery
workers now call a dedicated host port with their already-reserved activity ID.
The production CLI/App adapters retain separate `op_` checkout locks alongside
Session/binary/MCP/LSP claims. Managed hosts without explicit support refuse.
Retained settlement still holds actual descriptors through teardown. Regression
coverage verifies coexistence, duplicate/foreign ID refusal, removal exclusion,
unknown claim retention and preservation of unrelated Session evidence. This is
acquisition ownership, not native reconciliation authority: immutable persisted
checkout route/creation/lifetime binding and interrupted-claim acknowledgement
remain incomplete, so M2–M8 and parent tasks remain open.


Acquired checkout proof continuation: snapshot claims retain native common/activity/
creation directory pins and record exact file/creation identity. Runtime source
callbacks and consuming commits verify the dedicated native proof, including inside
the Store writer through commit/reply. Changed routes/records refuse without source
callbacks or acknowledgement. This does not finish immutable lifetime/resource
publication or unknown-claim reacquisition across early cuts. Those and complete
platform/client acceptance keep M2–M8 and parent tasks open.


Atomic lifetime-bound checkout publication continuation: new managed claims stage
and flush complete ownership records under their native lock before no-replace
publication. Version-2 claims carry exact Session creation/directory and immutable
lifetime digest. Runtime checks the complete returned resource set, including
version, foreign/missing context and duplicates, before native settlement. Read-only
inspection precedes any acknowledgement. Real native pre-publication kill and
collision tests retain evidence and refuse activity reuse. Unbound legacy claims
and Windows durability limits remain distinct. Unknown-claim acknowledgement,
full publication/recovery cuts, Git journals and reclamation remain incomplete;
no parent M2–M8 or 2.4/3.2–3.4 task is accepted here.


Read-only reacquisition and acquired-resource publication continuation (2026-11-07):
core now reacquires an exact exited version-2 checkout claim under its native lock,
retaining original metadata pins without creating, settling or changing evidence.
A distinct proof type exposes no generic settlement. Runtime publishes the actual
complete acquired binding set into the private lifetime source phase before origin
or source dispatch. Publication is quota-checked and immutable; source admission
retains references and refuses missing managed sets. Explicit empty unmanaged sets
remain distinct from absent constructor evidence. Unknown-claim acknowledgement,
partial acquisition/publication cuts, Git journals, reclamation and full native/client
acceptance remain open; no parent task is marked complete.


Consuming acquired-resource proof continuation: preparation review and cancellation
receipts now bind a versioned digest of the exact lifetime and complete acquired
checkout set. The production CLI/App host port reacquires original claims read-only;
recovery retains and revalidates those locks/pins and native reference publication
through the paired Store commit and observed reply. Missing/changed/live/foreign
proof refuses without a consuming receipt. Database-only lookup remains independent
of native evidence; explicit journal metadata acknowledgement verifies the committed
digest. Historical digest-less receipts retain only their existing journal authority.
Native unknown-claim acknowledgement is still required: this continuation does not
settle a checkout claim, delete construction evidence or accept any parent task.


Pre-effect native checkout acknowledgement continuation: observed paired receipts
with complete version-2 resource bindings now publish separate exact native markers
under retained original locks. Original claim bytes and independent unknown claims
are preserved. Read-only candidate checks at the writer reject conflicting existing
journal or checkout receipts; lookup never retries metadata. Explicit acknowledgement
reconciles interrupted journal/checkout publication without source replay. Verified
markers may admit later removal while pinning retained acknowledgement stages;
original creation records are archived without replacement when needed for removal
retry/recreation proof. Complete resource publication precedes checkout admission
settlement. Actual native pre-publication kill/namespace replacement and real Git
removal/recreation cases cover this step. M2–M8 and parent tasks remain open for
partial acquisition/corrupt construction, post-effect/owned Git integration, safe
reclamation, complete clients and full crash/platform acceptance.


Git image boundary continuation: shadow-Git blob lookup now resolves an exact literal
NUL-delimited path, validates mode/object identity and checks the typed SHA-1/SHA-256
blob hash. Malformed/ambiguous output, unsafe paths, unsupported entries and corrupt
blob bytes refuse before replacement. Real unusual-path and corrupted-loose-object
cases preserve the user's index and current source on failure. Historical exclusion
lookup remains mandatory. This is an image-materialization prerequisite for owned
Git journals, not their implementation; M2–M8 and all parent tasks remain open.


Verified Git catalog continuation: changed-path derivation now uses a bounded
Location-scoped catalog whose exclusion tag and every traversed directory tree
match their typed Git object IDs. Metadata subprocess overflow refuses after
killing/awaiting the child. Literal Location selectors preserve Unix backslashes
and glob-shaped names independently of inherited pathspec environment. Real
corrupt tag/root/nested-tree restores refuse without changing source or user index;
scoped restores preserve sibling files. This remains preparation for owned Git
journals; M2–M8 and parent tasks are unchanged and incomplete.


Git/native regular-file handoff continuation: the owned adapter now prepares a
bounded immutable Git object packet before the synchronous filesystem callback.
Native import rechecks leaf hashes and known expected paths, uses the shared CAS/
preimage/ownership engine and writes version-4 intents with original Git IDs and
exact worktree/Location context. Native fixture keep/rollback and request retry
retain that context; legacy copy encoding and fences remain distinct. Ordinary
Git worker reservation/admission is still disabled. Symbolic links, full expected
state scanning (including newly appearing paths), exact runtime Git lifetime/
receipt acknowledgement, retention and complete crash/client/platform acceptance
remain required; M2–M8 and parent tasks remain open.


Native link continuation: version-5 intents now bind backend and typed regular/link
images while preserving earlier file encodings and digest frames. Link observations
pin the link inode, reject aliases/unsupported metadata and read bounded target
bytes without following them. Shared native staging/exchange and exact-preimage
rollback support link/file transitions. Review and generated SDK expose image kind.
Local macOS native positives and five real link process-kill cuts cover inode
classification without progress/replay; these are backend fixtures, not complete
Git runtime acceptance. Ordinary Git admission remains disabled for full expected
state/Git compatibility, lifetime/acknowledgement, retention, clients and complete
crash/platform gates. M2–M8 and parent tasks remain open.


Native inventory continuation: the Git importer now compares eligible Location
leaf names, revalidates directory namespace generations and read-only Git rule
inputs, and rejects new/removed/type-changed state before intent publication.
Current expected images use their own exclusions; historical exclusions still
constrain target/recorded mutations. Native relative paths now preserve Unix
backslashes while rejecting empty/dot/traversal components. Snapshot-bound ignore/
raw-state proof, full Git compatibility, actual Git lifetime/acknowledgement, safe
reclamation and full client/platform acceptance remain open. Admission remains
disabled; no M2–M8 or parent task is accepted by these checks.


Capture-policy continuation: version-2 Git checkpoint tags now retain bounded
capture-time policy fingerprints. Native inventory compares effective ignore
settings, rule hashes and canonical ignored names before intent publication;
legacy records remain readable without native proof. Captures bracket Git tree
creation with policy collections and refuse a changed successful collection.
Application ignore/size settings and private roots are included. Raw working-tree
representation, configuration-resolution races, Git compatibility, actual worker
lifetime/acknowledgement, reclamation and full clients/platform acceptance remain
open. Git admission stays disabled; M2–M8 and parent tasks remain incomplete.


Raw-state continuation: supported captures now bracket Git tree creation with
bounded exact raw file/link reads. Version-3 tags retain canonical and raw graphs
through a verified envelope tree; native preparation uses raw graphs, preserving
original checkpoint identities and exclusion rules. Public Git diff/blob behavior
resolves the canonical graph. Legacy bare/v1/v2 checkpoints remain readable but
cannot supply missing raw native proof. Git runtime ownership/acknowledgement,
preview/path-selection parity for normalized-only changes, configuration races,
remaining Git compatibility, retention and full client/platform acceptance still
block ordinary admission. M2–M8 and parent tasks remain incomplete.


Native preview continuation: managed Git preview is connected to the same raw
planner as restore behind the unchanged owned-Git availability gate. Raw changed-
path selection is connected at the same gate, preserving normalization-only edits.
Rendering uses verified expected images rather than reopening current source, and
scoped Git paths/patch headers stay worktree-relative. Backend fixtures verify
preview leaves no journal/source changes and revalidates outside selected paths.
Complete summaries/client behavior, worker lifetime/acknowledgement, reclamation,
remaining Git compatibility and platform acceptance remain required. Ordinary Git
admission and all M2–M8/parent task acceptance remain unchanged and incomplete.


Git lifetime-port continuation: trusted callers may now reserve backend-independent
version-2 native lifetime metadata in Git Locations. Exact live locks, owner/Session
creation/Location checks and managed-checkout completeness use the shared native
engine. This lifts only the metadata reservation refusal; ordinary owned Git
source availability remains false. Actual Git worker/checkout consuming receipts,
post-effect acknowledgement/reconciliation, reclamation and complete crash/client/
platform acceptance remain required; M2–M8 and parent tasks remain open.


Ordinary consuming-receipt continuation: new restore receipts now bind exact
version-2 lifetime and complete checkout-set digest when native lifetime proof
exists. The paired writer rechecks the retained set and lookup verifies original
native lifetime history. Legacy receipt encoding remains unchanged without the
optional field and cannot generate new checkout acknowledgement evidence. This
adds receipt evidence only; exact post-effect native checkout publication and
reconciliation, actual Git runtime acceptance, reclamation and remaining M2–M8 /
parent obligations remain open. Git source availability stays disabled.


Ordinary checkout acknowledgement continuation: version-2 host acquisition now
retains original unsettled claim bytes instead of applying generic settlement
before source dispatch. The consuming writer verifies candidate markers read-only;
observed durable ordinary-restore receipts publish exact native markers. Explicit
reconciliation reacquires original lifetime/resources and never dispatches source
work. Legacy version-1 acquisition and old receipt behavior remain distinct. Missing
managed journal receipts refuse with retained recovery evidence; successful managed
no-effect completion is not implemented. Recovery-resolution receipt/checkout
integration, actual Git source/backend/runtime and complete crash/platform/client
acceptance, reference-aware cleanup and other M2–M8/parent obligations remain open.
Ordinary Git source admission remains disabled.


Owned no-op and native namespace continuation: both journal backends now retain a
real empty intent for an owned empty plan, using ordinary origin/completion and
receipt acknowledgement without source admission. Unowned no-op behavior stays
unchanged. Receipt validation no longer equates the lifetime Location namespace
with its distinct journal collection. Real CLI regressions cover owned file-copy
no-op completion, retained owner files, receipt lookup and repeated acknowledgement
without source replay/inference. The full managed Git backend/source/checkout path,
recovery-resolution resource acknowledgement, complete native cuts/platforms and
reference-aware cleanup still require acceptance; parent tasks and Git admission
remain unchanged and open.


Original-owner retention continuation: recovery now reads attested original lifetime
metadata, reacquires exact version-2 lifetime and published native checkout resources
before reserving a resolution owner, and retains both owners through work/commit.
Authorization revalidates the original set. Legacy v1/missing lifetimes gain no new
claim proof; unknown versions refuse. Completion drops native result guards before
worker lifetime locks. This is retention, not original/resolution checkout marker
authority: binding both sets into consuming recovery receipts and publishing/
reconciling their exact acknowledgements remain required. M2–M8 and parent tasks
stay open; ordinary Git source admission is unchanged and disabled.


Recovery checkout consuming-receipt continuation: new recovery receipts bind the
exact resolution origin, resolution lifetime/resource digest, and the original
version-2 lifetime/resource digest when available. Both retained claim sets are
checked read-only at the consuming paired writer, which checks both attested
lifetime histories and refuses a missing version-2 proof. Only an observed durable
pair admits exact resolution-then-original native checkout markers. Original
claim bytes stay immutable. Lookup verifies both committed owners, and explicit
metadata reconciliation reacquires both original resource sets rather than
substituting a fresh Session lease or replaying source work. Legacy receipts gain
no new claim authority. This continuation does not close M2–M8 or parent tasks:
partial acquisition, actual managed Git source integration, reference-aware
reclamation and full native/platform/client acceptance remain required.


Git format routing continuation: public capture now declines non-Git fallback when
an ancestor .git marker exists even if libgit2 cannot discover the repository.
Private shadows initialize with the user's exact supported object format and
mismatched existing shadows refuse before index mutation. Actual SHA-256 repository,
encoded-image and linked-worktree/subdirectory coverage replaces the former
manually initialized shadow-only graph fixture. Retained copy history stays readable
by typed IDs through existing namespaces; mixed-backend comparison refuses. This
does not admit managed Git source work or implement cross-backend restore/migration.
These compatibility/acceptance requirements, configuration races, partial
construction and reference-aware cleanup remain open with M2–M8 and parent tasks.


Git configuration locator continuation: supported native policy collection now
binds Git-reported root candidates (including absent files), active origins,
include targets, environment digest, exact file observations and physical directory
context. Selected values come from the bound origin inventory; batched explicit
path/boolean expansion cannot reselect a different value. Native planning rechecks
these inputs without Git subprocesses, using a new fingerprint frame that does not
upgrade old checkpoint proof. Metadata reads/waits have output/deadline limits and
kill/await cleanup. The application-created private reference-layout profile is
revalidated; unsupported topology, remote-URL conditions and named-user expansion
retain unavailable proof. This fixes the reproduced post-preparation locator change
but does not finish complete configuration/ABA/dependency, filter, actual Git
runtime/checkout, migration, crash/platform/client or reclamation acceptance.
Managed Git source admission and all parent tasks remain open and disabled.


Mixed-image journal continuation: native Git preparation can retain copy target or
recorded IDs as typed deferred manifests with a fresh raw Git expected checkpoint.
The synchronous native store loads only its original Location-bound manifests,
verifies BLAKE3 objects, enforces combined image budgets, and applies both backends'
historical exclusions without hiding expected-state checks. A version-6 context
and digest bind per-image source kinds; old versions reject those annotations.
Native preview/restore and keep/rollback, including interrupted replacement after
reopening the namespace, have backend coverage. Runtime-owned Git callbacks,
client planning/routing, complete migration/limit combinations, config/generated
rule-source and filter compatibility, native kills/platforms and reference-aware
cleanup remain required. Managed Git source admission and M2–M8/parent tasks remain
open; backend fixture origins do not constitute runtime/checkout proof.

Generated-rule source continuation: supported native policy now retains a bounded
version-1 generation recipe, original user common-repository exclude observations,
application settings, skipped literal names and exact output digest. Collection
regenerates and compares output, and native planning revalidates retained source,
recipe, output and common/info directory identity. Unchanged generation preserves
checkpoint identity; descriptor-relative output replacement does not follow file
or parent links. A new policy fingerprint frame leaves old evidence unpromoted.
This closes the reproduced post-preparation user-exclude change, including linked
worktrees, but does not prove user-index/large-untracked eligibility, complete Git
configuration/filter compatibility or actual runtime ownership. Managed Git source
admission, M2–M8 and parent tasks remain open. The next client milestone is typed
recovery review/confirm/lookup/acknowledgement on the implemented non-Git API, with
real restart/retry acceptance; it does not waive remaining Git/GC/platform gates.

Typed recovery CLI continuation: `sessions recovery` lists retained journals,
reviews an exact keep/rollback selection, and requires both fingerprint and stable
request ID for confirmation. `sessions recovery-request` performs lookup only;
explicit `--acknowledge` invokes the existing metadata-only reconciliation route.
Responses are typed and checked against the selected Session/journal/request;
missing legacy evidence and unobserved native acknowledgement remain explicit.
CLI reference and operator documentation are updated. Real-service kill/restart,
stale/foreign request, repeat lookup and later owner-edit coverage now exercise
these typed commands. This advances the restore-journal client portion of M7;
preparation/TUI clients, actual managed Git integration, safe cleanup and full
native/platform/client acceptance remain open. No parent task is closed.

Terminal restore-recovery continuation: rewind now exposes C recovery with retained
journal/request selection, explicit keep/rollback review and second-Enter
confirmation, read-only request lookup and separate A acknowledgement metadata
reconciliation. Bounded server-side discovery uses the exact Session creation,
journal and native request-owner identity, so a fresh App can find a reserved
request after losing its reply. Failed discovery cannot authorize a fresh attempt;
legacy proof, pending/unknown outcomes and unrelated fences remain protected.
Actual production-App/native tests interrupt after a proven source replacement,
exercise both keep/rollback, preserve unrelated/later owner edits and the composer,
discard a successful reply, create a fresh TUI App and discover/lookup/reconcile
without inference or transcript movement. This is native cancellation plus a fresh
App, not an OS kill of the TUI process. Updated CLI service-kill acceptance also
checks discovery after server restart and foreign-Session filtering. Preparation
clients, ordinary managed Git source integration, partial construction, reclamation
and complete cut/platform acceptance remain open; M2–M8 and parent tasks are not
closed by this client continuation.

Typed preparation CLI continuation: `sessions recovery-preparation` lists and
reviews exact candidates, then requires a fingerprint and stable request ID for
pre-effect cancellation. `sessions recovery-preparation-request` provides receipt
lookup and explicit native metadata reconciliation. Replies bind selected
Session/activity/lifetime and published-origin context; unsupported review proof
or admitted effects refuse, while historical receipts retain their separate read
semantics. Real CLI pre-origin and published-origin-before-source kill cases now
use these typed commands for cancellation, restart lookup and reconciliation,
including stale/foreign refusal and preservation of later owner symlinks/files.
This advances the preparation CLI portion of M7; preparation TUI, partial
construction, managed Git integration, safe reclamation and complete acceptance
remain open. No parent task is closed.

Preparation terminal continuation: rewind recovery now offers P for preparation
candidates and committed cancellation history, with J returning to journals.
Candidate selection discovers a committed receipt before native-proof review;
confirmation retains a fresh stable request before sending, consumes the review,
and permits lookup rather than Enter replay after reply loss. A is explicit
metadata reconciliation. Session/Location/generation guards preserve composer,
attachments and conversation. Opt-in history keeps default pending listing intact;
exact receipt discovery delegates to the validated original committed pair.
Controller and actual HTTP transport coverage accompanies native CLI service-kill
cuts before/after origin publication, including restarted history/discovery and
foreign-Session refusal. The subsequent real-binary PTY test now covers native
pre-origin and published-origin cancellation with the committed confirmation
response withheld, SIGKILL of the TUI, and fresh-process discovery/lookup/explicit
acknowledgement without another cancellation. It preserves saved draft attachment
payloads and later source symlinks/bytes, messages and inference counts. These are
local macOS arm64 positives; other platform/cut combinations remain unproven.
M7/M8 and their parent tasks stay open alongside partial constructors, managed Git
admission, safe reclamation and complete compatibility/platform evidence.

M6 ordinary-payload continuation: implement receipt-bound Store plan/completion
records, writer-serialized reference checks, retained native namespace/reader/
lifetime proof, and no-follow removal of positively owned ordinary file-copy
stages. Keep intent/lifetime metadata and CAS roots; recheck retention and refuse
unknown entries, aliases, live workers and independent uncertainty. Wire periodic
maintenance and a bounded cap-pressure capture retry. Validate real-App positives,
interrupted plan resume, missing/changed proof, disabled/zero retention and cap
pressure before accepting this path. Managed Git, recovery-resolution/preparation
construction payload cleanup and complete reclamation/platform acceptance remain
required; M6 and all parent tasks stay open.

M6 paired-payload continuation: reclaim original and resolution regular private
slots together after exact keep/rollback Store and native acknowledgement. Bind
both journals, preserve version-1 ordinary plan IDs, retain both lifetimes/phases,
and preflight complete immutable outcome/displaced inventories before deleting.
Exercise the real partial-restore kill fixture for keep and rollback, both live
worker locks, young retention, unknown entries and planned partial progress.
Construction/preparation payloads, managed resources/Git, CAS/metadata reclamation
and the complete native cut/platform matrix remain required; M6 stays open.

M6 prepared-cancellation continuation: bind the immutable prepared intent into
review and the durable cancellation activity, and admit version-3 target cleanup
only with persisted prepared inode identities, exact native cancellation and a
positive non-admitted phase. Retain all intent/cancellation/phase/request metadata;
unknown/incomplete construction and historical missing digest evidence cannot be
promoted into cleanup proof. Exercise an actual service kill with prepared images
while SQLite writer contention prevents source admission, then verify changed
inodes/digests, historical missing proof and durable partial cleanup. Raw partial
construction, managed/Git resources, metadata/CAS and full platform/cut acceptance
remain required; all parent tasks stay open.

User-index handoff continuation: native policy now binds the actual per-worktree
index, absence, routing files and bounded shared-index candidates, with native
read-only revalidation and a distinct fingerprint frame. Regression coverage
includes tracked-to-untracked, creation, identical-byte inode replacement, split
dependency changes and SHA-256 linked-worktree index isolation. A positive verifies
split-index metadata/bytes remain unchanged through preparation and native restore;
oversized observations retain canonical capture without native authority. Size-
decision generation, complete user config/filter compatibility, tracked large
files, runtime/checkout/recovery/cleanup and full crash/platform gates remain open.
Managed Git source admission and all M2–M8/parent task acceptance remain unchanged.

Generated size-exclusion continuation: retain size-excluded names from the same
verified recipe as generated output, include them in a distinct policy frame, and
recheck regular-file type and threshold through no-follow metadata at capture and
native planning revalidation. The reproduced shrink-after-preparation gap now
refuses before journal/source effects. Threshold/deletion/link/parent-link/FIFO
cases and still-oversized/fresh-capture historical-exclusion positives are covered.
Original classification provenance at generation, full configuration/filter/limit
compatibility and runtime/checkout/recovery/cleanup/platform acceptance remain
required; managed Git source admission and parent task acceptance stay disabled.

Classification-provenance continuation: version-2 generated recipes now bind
observed user index/routing, resolved user configuration and rule inputs. A private
classifier uses copied index/shared-index bytes and effective configuration whose
includes were resolved in the original branch/gitdir context; original metadata
stays strict. Private copied files have mode 0600 and post-read verification, with
only disposable shared-index timestamp refresh allowed. Fresh capture/preparation
requires matching provenance and skipped names; legacy/unproven recipes retain
canonical readability without native authority. GIT_CONFIG command redirection is
cleared to prevent internal configuration writes reaching an unrelated file.
Supported capture/preparation/restore and mutation-during-classification positives
and denials are covered. Complete configuration/ABA/filter/limit compatibility,
managed Git runtime/checkout/recovery/cleanup and source-bound platform acceptance
remain open; managed source admission and all parent acceptance stay disabled.

Runtime Git candidate continuation: add a non-default native-git-acceptance build
that takes the same owned restore/preview/changed-path ports and actual runtime
workers, without changing default admission. Git tracked-image reads use the raw
reader/total bound while configured max_file_bytes remains the untracked omission
threshold. Actual App cases cover root/scoped SHA-1 and scoped SHA-256, binary
tracked images above that threshold, links, Store receipts, native acknowledgement,
lookup/reconciliation and later owner edits. Root/scoped SHA-256 coordination now
shares a read-only ancestor-marker lock domain when libgit2 cannot discover it;
this is coordination, never source/permission authority. Add candidate jobs to the
native matrix, keeping their evidence distinct. Default admission, complete managed
resource/storage-root integration, keep/rollback/process-death/runtime cuts,
compatibility/reclamation and full platform acceptance remain open with M2–M8.

Managed default-storage continuation: identify ready registered source subtrees
within application data for observation/reservation while retaining private
snapshot/other-data/explicit-root boundaries. Require the actual worker's complete
native checkout claims before constructing source intents; registration alone
cannot authorize an unowned restore. Release the Session projection mutex before
host checkout-policy admission, retain its captured revision and refuse stale
admission. The non-default App candidate now exercises actual default /worktrees
creation, native resource publication, restore, unchanged user Git metadata and
reopened metadata acknowledgement without source replay. This is an ordinary
managed restore positive, not complete managed keep/rollback/process-death,
construction/reclamation/platform acceptance. Default Git admission remains off;
M2–M8 and all parent tasks remain open.

Managed Git CLI process-death continuation: the explicit native-git-acceptance
binary creates a ready managed linked checkout in default application storage,
observes a genuinely partial namespace during a 1,280-file restore and kills the
registered service. Fresh services review/confirm keep and rollback, consume
non-empty original/resolution checkout bindings and reconcile metadata without
source replay. Verify all bulk file images, an early deletion, unrelated/later
owner files, escaped newline paths, stale/foreign requests, unchanged transcript
and user index/configuration, and no inference during recovery. Reuse verified
blob bytes only within one bounded image handoff while charging/verifying every
placement; SHA-1/SHA-256 tests retain mode, byte-limit and later-corruption refusal.
The four native workflow jobs now select this candidate, but workflow edits are
not execution evidence. These two source-kill cuts do not close the full cut,
resource construction, reclamation or platform matrix; default Git admission and
all parent acceptance remain unchanged.

Resume verification (2026-11-08, macOS ARM): the repeated-blob regression passes
for SHA-1/SHA-256, snapshot all-target Clippy with `-D warnings` passes, and
format/whitespace checks pass. The candidate CLI suite reports 19 passes and one
preparation timeout in
`fully_prepared_pre_effect_cancellation_reclaims_only_persisted_target_inodes`;
that case passes alone (60.54 seconds). Managed Git keep/rollback passes, now
including an explicit assertion for the newline-containing path. This is not a
green complete-suite result. Strict validation with CI-pinned OpenSpec 1.13.2
passes all 117 items; spec lint reports zero errors and 25 warnings. The default
CLI run was stopped before execution; native CI/platform runs remain unperformed.
Fetched main is now `8266db6c`, including merged structured-output PR #116 and
stream-input PR #117. Integrate and audit their remaining acceptance obligations
before continuing sections 5/6; both issues remain open. These observations do
not close parent tasks or establish the preparation timeout's origin.

Main integration audit (2026-11-09): main `8266db6c` is merged at `2fcd745d`,
retaining the recovery branch and all seven uncommitted files. Incoming #116/#117
are now included; task 6.2 is reopened for exact driver ownership and bounded
retained state. Unchanged main passes its six JSONL tests. The integrated branch
initially fails four because optional snapshot warnings are treated as failed
inputs. After matching ordinary exec's nonfatal warning handling and adding a
real-binary regression, all seven JSONL and three structured-output CLI tests
pass. Five structured-result server and six binary-worker ownership tests also
pass, as do formatting/whitespace checks and strict OpenSpec (100 items after
incoming archives; lint zero errors/25 warnings). The focused review and remaining
scope are recorded in docs/goal-delivery.md. Full workspace/SDK/plugin/native CI
were not rerun; the prior preparation timeout and all parent acceptance gates
remain open. No goal implementation PR is open and no issue was closed.

Managed regular-file Git payload reclamation continuation (2026-11-09): ordinary
restores and both recovery owners now reacquire their complete persisted checkout
sets, require the exact already published consuming acknowledgements, synchronize
those retained markers/directories and hold all native pins through private
deletion. Missing markers never create metadata or a reclamation plan. Live
claims, changed registrations/resource digests and foreign dispositions refuse.
The existing plan/receipt formats are retained; data-only validation no longer
mistakes a managed resource set for an unsupported shape. Native phase checks
still require exact admission/origin and resource-digest evidence. Git restore
default admission remains off; typed link payloads, incomplete construction,
CAS/metadata collection and the full native/cut matrix remain open with M2–M8.

The real managed CLI keep/rollback cases now remove an acknowledgement and hold a
live claim to prove precise pre-plan refusals, then interrupt after durable plan
publication and resume reclamation without source replay. The App verifies aged
ordinary managed cleanup, reopened receipt lookup, preserved later owner symlink,
no inference and unchanged user index/configuration. The core read-only marker
requirement and persisted source-phase/digest cases pass. The native workflow
selects the new core requirement on all four platforms; edits are not CI evidence.

Local checks: all 20 candidate CLI branch/recovery tests pass with four parallel
threads; both Git App acceptance tests pass. Snapshot library: 140 passed and six
ignored subprocess fixtures; focused final marker and source-phase tests pass.
All-target Clippy for core/snapshot/server/App/CLI passes with warnings denied,
as do formatting/whitespace and strict OpenSpec (100 items; lint zero errors and
25 warnings). The preparation kill helper now gives staging an independent
bounded deadline; the service-kill probe matches the recorded server instance
rather than any listener at a reused port. An unconstrained parallel run passed
19 cases but stalled before conversation-test prompt admission; that attempt
was terminated after preserving its Store/registration and samples under
/tmp/cyber-goal-first-prompt-stall-20261109. Its isolated rerun passed. That run
is not green and its startup-stall cause remains unassigned. SDK/plugin/full
workspace and external native CI have not been rerun for this checkpoint.

Typed acknowledged-payload continuation (2026-11-09): ordinary and paired cleanup
now admits immutable symlink images through the existing no-follow native inode
pins, retaining the same complete preflight, frozen plan and per-unlink identity,
generation, route and checkout checks. App acceptance passes both managed restore
and SHA-1/SHA-256 root/scoped cases, including dangling journal targets and refusal
of equal-target replacement inodes before a plan exists. No source replay or
receipt/schema change is introduced. Cancelled Git preparation, incomplete
construction, CAS/metadata collection and parent acceptance remain open. Focused
code/security review found no additional issue in this extension; it is not a
review or acceptance of the whole branch.

Native CI run 42684837811 on 3d3471f9 passed the snapshot adapter and exact core
marker test on Linux x64/ARM, then failed managed App worktree creation because
bubblewrap was not installed. The direct Git App worker case passed on both.
The workflow now installs bubblewrap and applies the same Ubuntu user-namespace
prerequisite as ci.yml. This workflow edit is not passing CI evidence; fresh
source-bound platform results are still required.

Typed-payload local checks: snapshot library 140 passed, six ignored subprocess
fixtures; Git App acceptance two passed; exact managed Git real partial-kill
keep/rollback passed (315.50 seconds). The first expanded mixed-image CLI run
failed its 45-second first-exchange wait before reaching cleanup assertions.
The helper now permits 120 seconds for bounded preparation, reports journal stage
and intent state on timeout, and captures a prematurely exited command's output.
The same partial-window and post-kill source assertions remain required. This
retry does not establish the original timeout's precise cause or resolve the
separately preserved first-prompt startup stall. All-target Clippy for
core/snapshot/server/App/CLI passes with warnings denied. Formatting, whitespace,
strict OpenSpec (100 items) and lint (zero errors/25 warnings) pass. The complete
CLI suite was not repeated for this typed-payload extension.

JSONL last-message publication continuation (2026-11-09): exclusively created
same-directory temporary files replace the predictable `.name.tmp` path, with
synchronized bytes and cleanup after failed publication. Destination symlinks
are replaced without writing their targets. Concurrent writers publish one
complete answer. Publication failure preserves per-input results and produces
aggregate exit 1 plus an `end` error when the run would otherwise succeed;
interruption/protocol exit precedence is unchanged. Three unit and all eight
real JSONL regressions pass; CLI all-target Clippy passes with warnings denied.
Strict OpenSpec still passes all 100 items, lint zero errors/25 warnings.
The native workflow now selects these regressions and builds the Linux sandbox
helper after installing bubblewrap. External results for these edits are pending.
Task 6.3 remains open for hook events and bounded output ownership/backpressure;
6.2/6.4 and all parent acceptance obligations remain open.

Pending-input ledger continuation (2026-11-09): the JSONL driver now reserves an
exact message-ID slot before HTTP admission, retains at most 32 pending inputs,
and releases each terminal payload after emission. Promoted answers use stable
message IDs instead of indices into completed history. Refused admissions retire
their reserved slot; bounded aggregate counters retain input totals and failure
after payload retirement. Premature termination emits every pending result even
when an input already has an error. Two unit tests pass, including 4,096 payload
retirements and capacity/reused-slot identity checks. All nine real JSONL tests
pass, including 65 sequential correlated prompts and exact final input count.
Admission ordinals preserve pending terminal result order and the first failure
independently of completion order. The reporter also releases emitted completed
answer/tool/denial payloads while preserving billing and aggregate status. Owned
background tracking retains only its exact fork's handback-admitted bit rather
than every observed prompt ID; unrelated/foreign Session admissions cannot set
that bit. Thirty-one exec unit, 17 real delegation, five forked-command and nine
JSONL tests pass. CLI all-target Clippy passes with warnings denied; strict
OpenSpec passes 100 items and lint reports zero errors/25 warnings. Focused review
found the pending-output ordering regression and corrected it before acceptance
of this change. Request/interrupt ownership, bounded background Job lookup,
bounded output/backpressure, hook events and remaining 6.4 acceptance still remain
required. Tasks 6.2–6.4 stay open. Native CI run 42693044871 is on the earlier
9ac06c40 checkpoint; it is not native acceptance of this ledger extension.

Structured terminal-result continuation (2026-11-09): new runtime regressions
first reproduced absent input outcomes after interruption and budget exhaustion
during correction (five existing tests passed, two regressions failed). Drain
exit now records the matching open root input's outcome before releasing its
entry or publishing idle. It preserves a durable valid finalizer, records the
current submission count on error, and leaves shutdown attempts open for explicit
restart resume. Metadata settlement failures remain execution failures. Pass
execution/error classification is extracted to keep the existing cognitive
complexity limit. Four added cases prove interrupted/budget outcomes and lookup
after restart, schema/attempt preservation through actual overflow compaction,
and shutdown followed by explicit resume without resetting the result attempt.

Final local checks: 94 runtime regressions pass (13 budget, 19 context/compaction,
53 runtime and nine structured-root); three structured-output CLI and nine JSONL
tests pass. Server all-target Clippy passes with warnings denied, as do format,
whitespace and strict OpenSpec (100 items; lint zero errors/25 warnings). An
earlier Clippy run ran out of disk and the broader test command could not create
its log; neither is a passing attempt. Cargo package cleanup removed 23.2 GiB of
rebuildable server artifacts, and the checks above passed after cleanup with
incremental compilation disabled. Finalizer/terminal-outcome writer/restart cuts
and real CLI budget/SIGINT precedence remain open under 5.4. No new public receipt
or API/SDK field was added by this correction.

Native run 42693044871 passed all selected native, managed Git App/CLI and JSONL
steps on Linux x64 and ARM for 9ac06c40. macOS Intel/ARM jobs remain queued at this
audit; they have no passing result. The ledger and structured terminal-result
extensions need their own current-source native acceptance before delivery.

Structured terminal-write recovery continuation (2026-11-09): a new fault fixture
first reproduced two extra provider requests when a validated finalizer was
durable but its terminal append had failed. Start-of-pass recovery now settles
that valid result or the exhausted correction error before model dispatch; new
queued input remains eligible after reconciliation. Three runtime cases prove
valid/exhausted recovery without another request, preservation of later plain
prompt behavior, and old-result settlement before promoting a queued different
schema. The initial exhausted fixture repeated identical invalid calls and hit
the existing doom-loop approval (confirmed by its persisted permission event),
then timed out. Distinct invalid submissions now reach the intended boundary;
the repeated-tool safeguard is unchanged.

Real CLI cases now gate a correction at the provider and verify max-token and
wall-time termination retain exit 4, while Unix SIGINT retains 130 despite the
structured terminal error. The direct unsafe signal call was rejected by the
configured unsafe-code lint and replaced with the existing owned-child `kill
-INT` fixture pattern. Fixture helpers are split by preparation, reconciliation
and new-input checks to satisfy the cognitive complexity limit. These are
resolved intermediate failures, not passing attempts.

Final checks: 111 regressions pass in one server/CLI feature-unified run (97
runtime cases and 14 real CLI cases); combined server/CLI all-target Clippy passes
with warnings denied, as do formatting, whitespace and strict OpenSpec (100
items; lint zero errors/25 warnings). The native workflow selects all exec units
plus the structured-root/CLI-stop cases on its four hosts and retains their log.
This edit is not current-source external acceptance: run 42693044871 remains on
9ac06c40 (Linux x64/ARM passed; macOS jobs queued at the preceding audit). No new
API/SDK field or CLI flag was added. SDK/plugin/full-workspace checks and Windows
execution were not rerun here; the SIGINT case is explicitly Unix-only.

Structured steering and committed-reply continuation (2026-11-09): two new
runtime regressions first failed because a steered promotion discarded the prior
input's open result. Promotion now freezes displaced outcomes and commits each
immediately before its replacement in the same append. A valid durable finalizer
wins; otherwise each displaced structured input gets an explicit superseded
error with its own attempt count, including zero-attempt intermediate schemas.
Child result behavior and unstructured input semantics are unchanged. A shared
pure result constructor also preserves the same terminal rules at Drain exit.

Runtime cases prove multiple steered schemas and replacement by plain input.
Two real JSONL cases gate inference, admit steering concurrently over stdin, and
verify exact per-input outcomes plus aggregate failure for a superseded schema.
Two SQLite WAL-hook cases lose the acknowledgement only after the exact target
terminal commit is visible through an independent read-only connection. They
prove the writer fences, cached terminal state does not advance, and reopening
the physical Store recovers the original result or committed new input without
replaying old inference/tools or duplicating promotion. The native workflow now
watches the fault helper. Manual proof dispatches queue behind an active matrix
rather than cancelling it; obsolete pull-request heads still cancel normally.

Checks: 115 broad runtime/CLI regressions pass, followed by both added WAL-ack
cases. The final targeted run passes all 16 structured-root, five output-schema
CLI and 11 JSONL tests; combined server/CLI all-target Clippy passes with warnings
denied. Formatting, whitespace and strict OpenSpec pass (100 items; lint zero
errors/25 warnings). Focused code/security review confirms state stays unchanged
until the atomic promotion append succeeds and no request/reply or interruption
authority is inferred from these result records. No new API/SDK fields were
added. Current-source native/platform acceptance and global SDK/plugin/full
workspace checks remain open. This does not complete tasks 6.2–6.4 or M2–M8.


Cancelled fully prepared Git payload continuation (2026-11-09): cancellation
cleanup now accepts fully persisted Git target images and typed symlinks through
the same version-3 intent-bound plan, positive non-admitted phase, complete checkout
set and exact already-published acknowledgement checks as other cleanup classes.
No source replay, receipt field, plan format or default Git admission is added.
Incomplete construction without persisted identities, CAS/metadata collection and
the remaining native/cut/client gates remain open with M6 and its parent tasks.

Both real CLI prepared-cancellation cases pass on local macOS ARM (235.48 seconds):
file-copy and managed Git. The managed case kills the real service after prepared
identities are published while SQLite writer contention prevents source admission.
It verifies non-empty persisted checkout digest evidence, missing-marker and live
claim refusal before a plan, equal-target symlink and regular-file replacement
refusal, dangling private link ownership, immutable Store intent binding, unknown
entry refusal, resumed cleanup after a durable plan and synthetic planned unlink,
retained cancellation lookup after restart, unchanged owner files/transcript/Git
index/configuration and no cleanup inference. The Git fixture uses smaller images
under the unchanged handoff limit; its first oversized attempt was rejected before
cleanup, and an empty-resource assertion inherited from the file-copy fixture was
corrected to verify the actual persisted bindings. Neither failed attempt counts
as acceptance. The final helper refactor and persisted-set checks are in the passing
binary. Snapshot library: 140 passed, six intentionally ignored subprocess fixtures.
All-target snapshot/CLI Clippy with warnings denied, formatting/whitespace and
strict OpenSpec pass (100 items; lint zero errors/25 warnings). The compiler still
reports macOS sandbox deprecation and linker unwind-table warnings. Focused code
and security review found no additional issue in this extension; this is not a
review of the full recovery branch. SDK/generated-artifact, default CLI, full
workspace and current-source external platform acceptance were not rerun here.
The native workflow now selects both managed Git process-death cases serially;
the workflow edit is not passing platform evidence.

Native run 42716226637 at 03b4526a completed successfully on Linux x64/ARM and
macOS ARM/Intel. It predates steering and this cancellation extension. Run
42728260672 at 1be03642 covers steering and post-commit acknowledgement loss;
its Linux x64 job has passed, with the remaining jobs pending at this checkpoint.
Neither run accepts the new cancelled-Git cleanup source. Queue its own native
proof and retain each run's actual tested source identity.


Fallback collection retention continuation (2026-11-09): six new regressions
initially fail because disabled capture still prunes manifests, zero retention
still prunes temporary/orphan bytes, orphan aging is fixed at seven days, corrupt
or missing Location evidence is discovered after deletion, and policy revocation
is not rechecked. Collection now preflights the complete namespace-bound manifest
inventory and known regular object/temporary entries before manifest unlinks,
retains disabled/zero-retention bytes, applies configured retention to orphan and
temporary aging, and refuses a changed configuration before each unlink. Foreign
Location manifests, temporary aliases and unknown object entries refuse before
any manifest deletion; already empty namespaces are no-ops. The native collector
is isolated in copy/collection.rs and still uses the existing serialized Store
reference callback. No native worker/journal fence is relaxed and no API, SDK or
receipt schema changes. Missing surviving Location evidence protects orphan bytes;
durable collection/retirement evidence, complete metadata/CAS reclamation and
partial construction remain required by M6 rather than being inferred from names.

Final local verification: 28 fallback integration tests and 16 App reclamation
cases pass, including a real Store-writer disabled/zero-policy/reenabled-collection
case and existing reference-writer exclusion, cap-pressure, unknown-owner, native
process-death, restart and receipt regressions. The additional empty-namespace
unit and quoted unknown-entry diagnostic unit pass. A focused App unknown-payload
rerun passes after the diagnostic change, and the new CLI panic-preservation probe
passes. Earlier in this turn, before the final preflight refinements, the complete
snapshot suite passed 188 tests with seven intentional subprocess/diagnostic skips;
that earlier run is not claimed as final-source full-suite acceptance. Final
all-target snapshot/App/CLI Clippy and formatting/whitespace pass. Strict OpenSpec
passes 100 items; spec lint reports zero errors and 25 warnings. Focused code and
security review found no additional issue in these changes after the preflight
fixes; the macOS construction failure and complete branch review remain open. Native platform, SDK/generated
artifact and full-workspace delivery acceptance remain open.

Source-bound native failure: run 42728260672 at 1be03642 passes Linux x64/ARM,
but macOS ARM fails the managed Git process-death recovery case at cleanup-plan
preparation with "Unknown recovery payload or construction entry". Its adapter,
core marker and App Git acceptance steps passed; JSONL and structured steps were
skipped after the failure. The log does not identify the entry, so its type and
cause are not established and the fence must not be bypassed. Paired cleanup now
quotes the first bounded unexpected leaf in its refusal, with a unit case proving
newlines are escaped and bytes remain intact. CLI fixtures now retain their own
private roots on panic after service stop, with a catch-unwind regression; native
CI uploads those failed scripted-fixture directories including hidden Git metadata.
Independent JSONL/structured checks run unless cancelled even after an earlier
recovery failure, without relaxing the overall job gate. The native workflow also
selects the complete App reclamation suite. These edits provide diagnostics and
coverage, not a fix or passing acceptance for the macOS construction failure.

Owned intent-update continuation (2026-11-10): runtime-owned prepared journals
now publish version 7 with an immutable binding to two preallocated native intent
inodes and the original copy/Git image format. Progress, completion and native
acknowledgement overwrite/sync the spare, exchange the two names and sync the
directory; they allocate no anonymous source-phase temporary intent. Reopen and
updates reject missing, replaced, aliased or invalid bindings. Ordinary, paired
and prepared-cancellation payload cleanup retain both slots, recognize the spare
only through its binding and recheck both metadata generations before unlinks.
Legacy digest framing and unknown construction fences remain unchanged.

Local evidence: five intent-slot cases pass, including an actual subprocess kill
during a torn spare write and physical Store reopen; its ignored child is invoked
by the parent proof. The complete snapshot candidate run passes 197 cases with
eight intentional subprocess/diagnostic skips, before the final malformed-version
writer guard; the slot cases pass again with that guard. Eighteen App reclamation
and two App Git runtime cases pass with both metadata pins. Three real managed Git
CLI cases pass before the extra spare-generation pin; the full final serial CLI
run is pending. Four non-Git CLI cases pass after correcting the old version-3
assertions. The broad four-thread CLI run records six failures: those three
version assertions, two pre-origin observation deadlines and one typed storage-busy
TUI acknowledgement. The three preparation/TUI cases pass serially in 124.39s;
the parallel failure causes remain unestablished, and no safety fence or deadline
was relaxed. Six structured CLI cases pass, including a separate timeout with a
verified in-flight correction; short deadlines may legitimately expire before
main inference. All-target snapshot/App/CLI Clippy passes after extracting the
copy-intent header assertions. Focused code/security review finds no additional
issue in this increment; full-branch review and delivery acceptance remain open.

Native run 42733865471 passes all four Linux/macOS jobs at e2b3ba18. It predates
these changes and accepts only that checkpoint. The workflow now selects the
preparation/TUI client cases serially in addition to managed Git recovery; fresh
source-bound platform evidence remains required. Partial construction, complete
reference/CAS/metadata retirement, default Git admission and M2–M8/parent task
acceptance remain open. No public API or SDK schema changes in this increment.

Final intent-slot CLI evidence: all 22 serial branch/recovery cases pass at
2ec1b1c5 in 837.69s, including both managed Git process-death cases and terminal
lost-reply reconciliation. SDK generation consistency, typecheck and 136 tests
pass at that checkpoint, with ten configured external skips. Native run
42753722365 passes both Linux jobs and macOS ARM; Intel remains running at this
checkpoint. These results do not accept later source changes.

Collection-provenance continuation (2026-11-10): before any unlink, fallback
collection now publishes/synchronizes an immutable bounded Location/root/native-
namespace record derived from complete valid manifests and pins its exact native
identity/generation through deletion. Restart after the last manifest disappears
can establish fresh retention without guessing from orphan names. Missing legacy
provenance, corrupt/foreign records, aliases, changed metadata, unavailable
configuration and disabled/zero retention protect remaining bytes. Every protected
copy reference must resolve to a valid manifest in the global private inventory
before collection; a dangling root cannot silently become an empty object set.
Valid other-Location roots and unrelated Git IDs retain compatibility. Namespace
metadata and unknown unpublished construction files count toward quota; none gain
cleanup authority from their names. Journal/worker fences remain unchanged.

Local verification: the snapshot candidate suite passes 205 cases with nine
intentional subprocess/diagnostic skips before the final unavailable-policy and
Git-reference compatibility refinements. Final targeted runs cover all 31 fallback
cases and seven collection unit cases, including the actual process-kill/physical-
reopen proof and its invoked ignored child. Eighteen App reclamation cases pass,
including serialized reference-writer exclusion. The final refinements are confined
to collection and covered by its targeted runs. Focused code/security review caught
the missing-reference and null-configuration cases; both now have regression
coverage. Full metadata/CAS retirement, partial construction and complete M2–M8
acceptance remain open. Public API/SDK schemas are unchanged; full workspace and
source-bound native/platform acceptance remain required.

Final collection-provenance checks: all-target snapshot/App/CLI Clippy with warnings
denied, formatting and whitespace pass. Strict OpenSpec passes 100 items; spec lint
reports zero errors and 25 warnings. Native macOS sandbox deprecation and linker
unwind-table diagnostics were observed during builds. No new public/generated
contract changed; the earlier SDK evidence remains scoped to its tested checkpoint.

Native scheduling correction: run 42753722365 at 2ec1b1c5 finishes both Linux jobs
and macOS ARM successfully. Intel passes snapshot/core/App, managed Git, preparation/
TUI and JSONL checks, then reaches the 45-minute job limit while compiling final
structured-result binaries; its matrix result is cancelled, not accepted. The job
allowance is now 60 minutes without changing any test deadline/assertion. Collection
source 124c11b9 has its own run 42759174937 in progress and is not accepted from the
prior source's results.

Metadata-retirement writer continuation (2026-11-10): reclamation sequences 2/3
now retain a bounded semantic-role native identity/generation/content-hash plan
and its exact completion, separately from payload completion at sequence 1.
Owner/plan mismatch, duplicate roles or inode identities, supplied paths, incomplete
payloads and inconsistent bytes refuse. The App's additive maintenance port commits
the immutable plan before its guarded callback, serializes it against reference
writers, re-attests receipt/native fences/references, and returns zero without
calling an observed completed callback again. Legacy trait implementations default
to unavailable without invoking the callback. A migration marker makes older Store
catalogs read-only; physical Store reopen and historical reprojection retain exact
phase distinctions without conferring native I/O authority.

This is a writer prerequisite, not native retirement acceptance. No native adapter
calls the port yet; current cleanup still retains all journal/lifetime/phase
metadata. Implement native per-leaf planning/deletion/resumption and retired-receipt
lookup/reconciliation, then complete managed checkout/partial-construction and
crash/platform acceptance. M6 and all parent tasks remain open. HTTP/OpenAPI/SDK
schemas are unchanged; the Rust maintenance interface and internal event protocol
have the additive retirement state/port described above.

The real App unavailable-policy regression initially deletes 57,344 bytes under
null configuration. The common ordinary/paired/cancellation retention guard now
refuses unavailable policy, and an actual two-payload case loses configuration after
its first unlink, preserves the second and resumes only under restored policy.
Final local evidence: 27 App reclamation/writer cases and 14 migration cases pass,
with an additional exact-inventory re-attestation and retry-counter rerun after
review refinements. The metadata callback tests deliberately prove Store sequencing
without deleting native metadata. All-target Store/server/snapshot/App/CLI Clippy,
formatting/whitespace, OpenAPI artifact and SDK generation checks pass; strict
OpenSpec passes 100 items and spec lint reports zero errors/25 warnings. macOS
sandbox deprecation and linker diagnostics were observed. Full workspace, SDK
runtime suites, native metadata deletion and new-source platform checks remain open.

Review refinement: the metadata registry now bounds vector length and counts JSON
encoding into a 16 KiB sink before cloning/deserializing; scalar bounds also precede
typed encoding. Final oversized-owner/vector, duplicate-identity, missing/foreign
role and supplied-path cases pass, as does final server/App Clippy. This adds no
native deletion authority. Focused code/security review finds no further issue in
this writer increment; native producer/reconciliation and full branch review remain
required before delivery acceptance.

Collection source 124c11b9 completes native run 42759174937 successfully on Linux
x64/ARM and macOS ARM/Intel. This accepts that source, not the subsequent metadata
writer protocol. Run 42759434324 at 50e00372 retains the longer workflow allowance
and is queued at this checkpoint; it also predates these writer changes.

Native metadata-retirement continuation (2026-11-10): the Linux/macOS adapter now
captures semantic-role identities, generations, BLAKE3 hashes and exact byte totals
for acknowledged journals with empty complete checkout inventories. It retains the
Store namespace lock, Location mutation/reader lease and actual exited lifetime
locks, verifies current and retained routes, known-only journal inventory, pins and
policy before each unlink, and synchronizes exact leaf/directory removal. Existing
immutable plans resume missing leaves without reconstructing or replaying source.
Unknown entries, replacement inodes, aliases, live owners and changed policy refuse.
The Store writer remains held through the callback; completed receipts answer native
acknowledgement from retained Store history, while pending plans freeze rewrites.

Periodic maintenance discovers at most 32 payload-completed receipts from the Store
per stable cursor page, with 64 KiB per-receipt bounds and Location-scoped cap
fallback. Thus missing active intents do not hide interrupted cleanup, and deferred
managed candidates do not permanently starve later pages. Discovery is not native
authority. A successful metadata cap fallback permits one fresh capture attempt.

Local evidence: all 37 App reclamation cases pass, including real ordinary native
retirement, actual HTTP acknowledgement after removal, before/after callback cuts,
replacement/unknown/alias/live-lock refusal, policy loss after the first unlink,
stable discovery pagination, cap-triggered capture and actual subprocess SIGKILL
followed by physical Store reopen and production periodic resumption. Later source
symlinks/unreadable owner bytes, transcript and inference counts are preserved.
The child fixture is an inert top-level test unless explicitly invoked by its parent;
its process-death assertion actually executes in the parent case. Affected server,
snapshot and App all-target Clippy passes with warnings denied; native macOS C
sandbox deprecation diagnostics are observed without unchanged-main attribution.
Managed claim retirement, zero-payload/partial construction, old/deleted Location
maintenance, full receipt-kind cuts and current-source platform acceptance remain
required. M6 and parent tasks remain open; this is not issue or delivery completion.

Prior-source native run 42759434324 at 50e00372 finishes both Linux jobs and macOS
ARM successfully; Intel fails the managed Git prepared-cancellation test because
its preparation origin was not observed (branches_cli.rs:1912). The failed private
fixture is retained/uploaded at .tmphzzjxr; the fixture-preservation probe's earlier
caught panic is expected and passes. Independent preparation/TUI, JSONL and
structured checks pass on Intel. This is a real unresolved acceptance failure,
not a passing matrix or evidence that a longer job allowance corrected the case.
Run 42767559419 at 5be366b7 is queued at this audit and predates native retirement.

Final local checkpoint verification: native-git-acceptance snapshot package passes
153 library cases (8 subprocess fixtures intentionally ignored at top level), 31
file-copy cases and 22 Git cases (1 subprocess fixture ignored at top level); the
Windows-only and doc binaries select zero here. Server snapshot selection passes
53 cases. OpenAPI artifact, SDK generation consistency, formatting and whitespace
checks pass. CI-pinned OpenSpec 1.13.2 validates all 100 items; spec lint reports zero
errors/25 warnings. Code-review-skill review of the native descriptor/route checks,
semantic-only plan, writer-held callback, refusal/resumption and actual tests finds
no additional defect in the tested ordinary path. Full receipt-kind acceptance
has a test gap: native preparation/paired retirement needs direct cases, and managed
retirement remains unsupported. These are required before accepting M6/delivery.
Full workspace, SDK runtime, current-source CLI/TUI and cross-platform acceptance
were not rerun for this checkpoint. No PR, merge or parent completion is claimed.

Intel cancellation investigation (2026-11-10): downloaded the retained failed
fixture from native run 42759434324, artifact 13741826409. Its original archive
remains untouched. A separate read-only SQLite inspection finds no published
snapshot preparation origin; server logs show the preceding rewind preview took
40,253 ms. This narrows the failure to the pre-origin request path but does not
prove the final cause or acceptance. The confirmation client previously discarded
its output when the observation assertion failed; its stdout/stderr now remain
inside the retained fixture, with exit status and observed-origin timing in logs.
The existing 45-second origin, 120-second staging and native pre-effect assertions
are unchanged.

Raw graph publication was starting one hash-object process per path even for
identical immutable captured bytes. The new isolated real-Git regression initially
fails with 65 calls for two contents. An exact-byte borrowed map now writes each
distinct blob once per capture while preserving every path/mode/tree record and
Git hash verification. The cache never crosses captures or source observations.
The regression passes for SHA-1 and SHA-256, binary bytes, literal newline paths,
executable/regular/link entries, equal bytes across different modes, repeated
capture and changed content (2/4/7 cumulative writes). It does not replace source
identity/policy capture checks or per-placement handoff quota accounting.

Final local raw-publication verification: the real managed Git prepared-cancellation
case passes in 130.51 seconds with origin observation at 14.5746 seconds. All original
45/120-second deadlines and actual source-admission/cancellation/payload assertions
remain. The feature snapshot package passes 154 library, 31 file-copy and 22 Git
cases; nine library subprocess fixtures and one Git subprocess fixture are ignored
at top level. The new process-accounting child actually runs both formats through
its parent. Windows-only/doc binaries select zero on this host. Snapshot/CLI all-target Clippy passes with warnings denied
following a test-helper split (the initial helper exceeded 15); no lint allowance
was added. Formatting, whitespace, strict OpenSpec (100 items) and spec lint
(0 errors/25 warnings) pass. Native C sandbox deprecations and a linker unwind-size
warning were observed without assigning their origin. Code/security review finds
no additional issue in this exact-byte, operation-local optimization: no source
observation, path/mode placement, hash check or quota proof is removed. Native CI
path selection now also covers App snapshot support fixtures and nested Store
reclamation protocol files. Full branch review, workspace/SDK runtime, other
CLI/TUI scenarios and fresh source-bound platform acceptance remain required;
this does not complete the Intel acceptance failure, M6 or issue #93.

Native receipt-kind retirement continuation (2026-11-10): existing actual CLI
preparation and partial-source-kill keep/rollback fixtures now invoke metadata
retirement only after observed payload completion. Prepared cancellation retains
its immutable five-file plan; paired recovery retains eight semantic leaves for
two exact owners. Tests require journal removal, no-follow absence of each lifetime
and source-phase path, observed completion, zero-effect repeat retirement, and the
original receipt through restarted real CLI acknowledgement. Existing later-owner
symlink/bytes, transcript, model-request and source-result assertions remain.
The native matrix now selects these two full cases in independent steps even after
another step fails, retains separate logs and preserves failed fixture directories.

The new managed-preparation refusal assertion initially fails on the error text:
empty-set digest validation returned a generic incomplete-resource error before
the adapter's intended unsupported-managed diagnostic. Its local fixture remains
at .tmpskmwRM. The adapter now explicitly refuses managed ownership before that
empty-set calculation; complete receipt verification and every native/Store fence
remain mandatory. The regression requires the specific unavailable result, no
metadata plan, and retained intent/cancellation files. This grants no managed
retirement authority. Managed claims, zero-payload and partial construction,
old/deleted Location maintenance and complete crash/platform acceptance remain
required before M6 or its parent tasks can close.

Final receipt-kind checks pass: all three exact CLI cases run serially and pass
with final no-follow worker-file absence assertions and the explicit managed
refusal. The cases cover prepared cancellation, managed Git cancellation (retained
unsupported metadata) and both actual partial-kill keep/rollback actions. The
corrected run reports 220.47 seconds, including an early fixture pause to let the
previous run finish; that elapsed time is not a performance benchmark. Its native
origin observations occur after resumption under the original deadline. The prior
run's two positives and one diagnostic mismatch are retained as failures rather
than counted as a green suite. Snapshot/CLI all-target Clippy with warnings denied,
formatting/whitespace, strict OpenSpec (100 items), spec lint (0 errors/25 warnings)
and Ruby YAML parsing pass. Code-review-skill review finds no additional issue in
the typed receipt-based assertions or the refusal-only guard correction. No new
HTTP/generated contract changes. Full workspace, SDK runtime, other CLI/TUI cases,
all receipt-kind crash cuts and new-source platform acceptance remain unperformed
here and required by their parent tasks. No M6, issue, PR or delivery completion is
claimed. The preserved original pre-main-integration stash remains intact.

Managed retirement implementation continuation: extend the approved M6 protocol
with a frozen complete checkout inventory, exact native claim/acknowledgement
observations and persistent repository-lock identity. Retain all native locks
through the final metadata call; remove only verified acknowledged leaves under
the Store writer's durable plan. Resumption may observe missing exact leaves only
after that plan, and same-owner construction/foreign/aliased/live evidence refuses.
Shared creation and unrelated activity records remain retained. Implement the core
native guard, extended writer frame/catalog fence, actual snapshot adapter and
ordinary/preparation/paired managed positives plus refusal and process-cut cases.
This is ongoing M6 implementation, not acceptance or a new parent completion.

Managed checkout metadata retirement (2026-11-10): core now exposes a native guard
separate from its serializable observations. It derives fixed claim/acknowledgement
names, retains unique no-follow file locks and persistent repository locks, checks
native routes/creation bytes and every surviving generation before each unlink,
removes acknowledgement before original claim and preserves unrelated activity and
shared creation records. Same-owner unknown constructors refuse. Missing leaves
are progress only for the trusted adapter's already committed exact plan; ordinary
acquisition/acknowledgement is never synthesized during cleanup. Metadata opens
are nonblocking before regular-file validation, including a bounded FIFO regression.

The internal metadata event's version-2 frame retains complete ordered checkout
bindings with per-owner consumed resource/receipt digests and exact leaf metadata.
It bounds resources to 8,192, each native leaf to one MiB, aggregate checkout bytes
to 64 MiB and encoding to 32 MiB. Legacy version-1 frames stay empty-inventory and
16 KiB bounded. Shape/uniqueness and exact consumption are validated at the Store
writer; no data frame itself grants native effects. Migration marker
20261110010000_snapshot_checkout_retirement makes older catalogs read-only and
preserves existing evidence across physical reopen. Historical absolute-path
shape validation is portable and data-only; native acquisition separately requires
host-native absolute routes.

The actual snapshot adapter now prepares those guards from retained lifetimes,
complete resource publication and existing exact consuming acknowledgements,
commits the immutable Store plan, performs guarded claim and private metadata
retirement under the writer, and resumes from frozen bindings after phase/intent
removal. Repository locks remain through its last filesystem call. Shared
namespace/record entries are synchronized before planning, and exact parent
directories are also synchronized when resumption merely observes missing names.
Review tightened capture ordering: native identities/hashes/generations are frozen
before typed original context is re-attested, then fresh pins reject later changes.
Changed intent/admission and original-claim context regressions preserve bytes and
refuse before planning. The default owned-Git admission gate remains off.

Actual local positives cover managed ordinary restore in the default data layout,
managed prepared cancellation, and both keep/rollback after real partial source
replacement kills. They verify claim/acknowledgement disappearance, exact completion,
no-op repeats, protected index/config/creation bytes and receipt-only lookup/
acknowledgement after restarting. Later source edits/links, transcript and inference
counts remain unchanged. Native guard tests cover both per-leaf cuts through reopen,
multiple consumed owners sharing one repository lock, live/busy exclusion,
replacement/changed/aliased/context/constructor refusal, FIFO nonblocking and data
bounds. These primitive reopen cuts are not managed Store/process-death acceptance.
The prior unrelated probe panic is caught/expected; two test-helper failures were
retained as failures (.tmpvlw4ci and .tmptdEG3s), then corrected to avoid recreating
retired journals or killing an already stopped service. App/core failed fixture
paths are now retained for native CI alongside the existing CLI evidence.

Final local managed-retirement verification: 22 selected core worktree tests pass
(two existing subprocess fixtures ignored at top level), including nine new
retirement/portable-path cases; all 14 migration cases pass. Both real App Git
runtime cases and all 38 App reclamation cases pass. The final three managed CLI
cases pass in 292.72 seconds, including the caught fixture-preservation probe and
both keep/rollback source-kill actions. A later sample attempt finds the CLI process
already terminal; the verified filesystem wait is not treated as a timeout or
restarted. The snapshot feature package passes 154 library, 31 file-copy and 22 Git
cases (nine library/one Git subprocess fixtures intentionally ignored at top level;
Windows-only/doc binaries select zero here). All-target core/Store/server/snapshot/
App/CLI Clippy passes with warnings denied, as do format/whitespace, SDK generation
consistency, strict OpenSpec (100 items), spec lint (0 errors/25 warnings), and Ruby
workflow YAML parsing/guard-step selection. macOS C sandbox deprecations and linker
unwind-size diagnostics are observed without assigning their origin. Build-only
CLI artifacts were cleaned with cargo clean after verifying no CLI test/service
was live; original source/evidence and the pre-main-integration stash are preserved.

Code-review-skill summary for this checkpoint: architecture follows the existing
native core / runtime receipt / App Store-writer / snapshot adapter boundaries.
Native plans are data only; publication uses existing consumption markers and no
source replay or capability activation. Review identified and corrected capture
ordering and missing-parent-sync gaps; context/bounds/refusal tests pass. Critical:
0. Major: 1 required acceptance gap (managed metadata-deletion process/crash matrix
and current-source platforms). Minor/nits: 0. Verdict: REQUEST CHANGES before full
recovery delivery, while this scoped checkpoint is reviewable. Next: actual managed
metadata process cuts/retention-loss/reader cases, zero-payload and partial
construction, old/deleted Location maintenance and complete native acceptance.
Full workspace, SDK runtime and other CLI/TUI scenarios were not rerun here.
M6, parent tasks and issue #93 remain open; no PR, merge or delivery is claimed.

The final OpenAPI artifact consistency test also passes (one exact HTTP case).
No generated HTTP/SDK schema changes are required by this internal protocol.

Managed metadata process-death continuation (2026-11-10): an actual App creates a
managed checkout in default application data, captures/restores through the real
owned worker, observes payload completion and starts metadata retirement. A
configuration callback observes the real acknowledgement unlink, or the following
original-claim unlink, and parks before the next native deletion. The parent
verifies readiness, sends SIGKILL and reaps the child. No native source/deletion
hook or Store event is replaced. Failed control/child fixtures remain retained.
The original 30-second restore confirmation bound is unchanged; a separate new
90-second parent deadline covers the complete fixture setup and selected cut.

After each kill, a physically reopened Store shows version-2 metadata planning
without completion, the exact missing checkout leaves and the still-retained
intent. Production periodic maintenance resumes the frozen complete bindings,
finishes the same plan and returns zero on repeat. Protected Git index/config and
shared creation bytes, Session event sequence and a later source symlink to an
unreadable owner file stay unchanged. A fresh App returns the same receipt through
explicit acknowledgement after metadata is gone. These are ordinary managed
restore cuts at acknowledgement/claim removal, not the full managed preparation/
paired/constructor/removed-Location crash matrix. The child entry point is inert
in ordinary test discovery and actually executes twice through its parent.

The existing managed App fixture setup and tracked restore are shared with the
new subprocess case without changing its source/ownership assertions. Native CI
already runs the complete Git App binary; its path filter now explicitly includes
the new support file. This advances M6/M8 acceptance without closing either task.

Final local cut verification passes all four Git App entries in 48.73 seconds,
including the inert child and parent exercising two actual SIGKILLs. The focused
new parent also passes independently in 28.83 seconds. App all-target Clippy with
warnings denied passes after splitting cut-state assertions from resumption (the
initial combined helper measured 18/15; no allowance was added). Formatting,
whitespace, strict OpenSpec (100 items) and spec lint (0 errors/25 warnings) pass.
Code-review-skill review checks the exact observed cut, unconditional child
kill/reap, retained failure roots, physical Store reopen, immutable plan equality,
production maintenance resumption and data-only acknowledgement assertions; no
additional issue is found in this test continuation. Native C sandbox and linker
unwind diagnostics were observed without assigning their origin. No production
behavior, wire/SDK contract, default Git admission or parent completion changes.
Full workspace/SDK runtime and fresh-source cross-platform proof were not performed
locally; the native matrix must execute this source before accepting M8.

Ordinary zero-payload continuation: remove the positive displaced-byte eligibility
requirement and let an exact empty native inventory run through payload planning,
the serialized native verification callback and durable completion. Empty displaced
files remain actual leaves that must be verified and unlinked despite reporting
zero bytes. Unchanged restores require their positively non-admitted source phase;
restores with changes still require admission. The same immutable receipt, intent,
checkout, lifetime, namespace, reference and retention checks apply. Remove the
now-unused admitted-only phase wrapper and keep its resource/digest unit tests on
the explicit admission verifier. Paired/preparation zero-inventory paths remain
separate; this does not claim their completion.

Three actual App cases (restored creation, empty displaced files and unchanged
restore) failed before the fix because no cleanup plan was committed, then pass
with periodic metadata retirement and data-only HTTP receipt acknowledgement.
Each also refuses unavailable/disabled/zero retention, unknown construction, a
live lifetime lock and contradictory source admission before committing a plan.
A fourth regression interrupts the zero-leaf callback before and after native
verification and resumes the exact durable plan before periodic retirement.
Final focused selection passes four tests in 25.24 seconds.
The full reclamation selection before the final assertion-helper extraction and
fourth test addition passes 41 cases in 251.50 seconds; the final focused run
verifies the extracted helpers and added interruption case. Native macOS C sandbox
deprecation and linker unwind diagnostics were observed without assigning origin.
Full workspace/SDK runtime and fresh-source platform checks were not run locally.
Lifetime tests pass
seven cases with one explicit subprocess fixture ignored. App/snapshot all-target
Clippy with warnings denied passes after splitting cleanup and acknowledgement
assertions (the original combined helper measured 24/15; no allowance added).
Strict OpenSpec passes 100 items and spec lint reports 0 errors/25 warnings.

Focused code-review-skill review finds no outstanding issue in the changed native
admission and fixture paths: empty plans still execute full pre/post verification
and synchronization, missing expected images still require an exact frozen plan,
and periodic retirement cannot precede completion. Architecture follows the
existing native adapter/Store-policy port; no new HTTP, SDK or schema surface is
introduced. Cross-platform acceptance, managed/Git zero-payload cases, construction,
old/removed Location and complete crash/client acceptance remain required. M6/M8,
all parent tasks and issue #93 remain open.

Native run 42774849027 at b42f5fca now has four successful platform jobs, including
macOS Intel. Later managed-retirement and real-cut checkpoints are running/queued;
that success does not accept later source. Fetching main finds 8266db6c already
integrated, with no new upstream commits to merge.

Paired/preparation zero-payload continuation: both native adapters now submit a
verified empty inventory to the existing Store planning/completion protocol instead
of returning before the callback. Paired cleanup synchronizes every owner journal
even for an empty or already removed inventory. No receipt, event or API schema
changes; all owner, intent, acknowledgement, source-phase and reference fences
remain in force. This extends the approved M6 protocol rather than accepting M6.

Four new real CLI regressions cover file-copy and managed Git deletion-only
preparation cancellation and kept creation after native completion but before
Store commit. Preparation uses the existing native kill helper. The completed-
source cut observes the first creation, holds the actual SQLite writer, observes
the complete native intent and empty private payload inventory, then kills the
service. Its client is always killed/reaped on scope exit. The new fixtures use
256 leaves to keep the cut observable; existing 1,280-leaf partial-cut fixtures and
their deadlines/assertions stay unchanged. Each case refuses unknown construction,
interrupts after committing an empty plan, resumes that exact plan, retires the
expected managed claim/acknowledgement count and all consumed worker/journal leaves,
and repeats retirement with zero effect. A later source symlink to unreadable owner
bytes, Git metadata, transcript and inference count survive cleanup and explicit
receipt acknowledgement after service restart. All four reached the empty-plan
shortcut before the production fix and failed there. The final native-feature
selection passes four cases in 47.43 seconds, including the complete managed
checkout-count assertions. Earlier fixture assumptions about omitted false fields
and partial-only recovery reviews were corrected before this regression result;
failed roots remain preserved.

Native run 42784209997 at 991b1519 reports Git App setup failures on both Linux
platforms: retained fixture paths exceed the sandbox proxy Unix socket address
limit. The log from Linux x64 is retained locally for inspection. CI now uses a
short `/tmp/c<workflow-run-number>` fixture root and uploads that same root,
including hidden files and nested managed process-cut fixtures. A new independent
step runs all four zero-payload CLI cases and retains its log. YAML parsing and
fixture/artifact root consistency pass. Production networking and sandbox policy
are unchanged; fresh-source CI must verify this routing correction.

Default and native-feature CLI/snapshot all-target Clippy with warnings denied pass. Strict
OpenSpec passes 100 items and spec lint reports 0 errors/25 warnings. The first
green build attempt failed linking because the disk was full; only regenerable
Rust incremental compiler cache was cleared before a successful retry, preserving
source and native failure fixtures. Native C deprecation and linker unwind
diagnostics were observed without assigning their origin. Focused code-review-skill
review checks the empty callback's full pre/post verification, per-owner directory
synchronization, exact frozen plan resume and managed claim cardinality; no
outstanding issue is found in this continuation. This focused review approves the
continuation with zero findings; whole-branch review remains required for delivery.
The existing managed Git selection
passes three tests in 162.81 seconds: the deliberately caught fixture-preservation
probe, real partial-kill keep/rollback, and prepared cancellation with file/link
cleanup. The two file-copy zero-payload cases also pass in the default build in
16.87 seconds. Main is fetched again with no upstream commits missing from this
branch. Full workspace/SDK runtime and current-source platform
acceptance remain required. Constructor, old/removed Location and remaining native
cut/client acceptance stay open with M6/M8 and all parent tasks.

Location-route acceptance continuation: the existing producer still requires the
original live directory identity. New tests use two actual Locations and Sessions
in one App/serialized Store to cover missing, replacement and private-root alias
routes, both before payload reclamation and with a committed native metadata plan.
The unavailable owner retains exact journal/worker bytes, plan and event sequences;
the other Location completes payload and metadata retirement during that same GC
pass. Returning the original directory resumes cleanup without changing later
source edits, Session/native-owner sequences or inference count, and explicit HTTP
acknowledgement returns the retained receipt. This is six route/phase combinations.

A seventh case uses the actual configuration callback after native active-intent
unlink to rename the source directory. Native verification refuses before removing
the spare or worker metadata, leaving sequence 2 and the exact stored plan. After
the directory returns, Store discovery finds the owner without its intent and
periodic maintenance completes that identical plan. No native delete or source
effect hook is replaced. The initial race assertion mistakenly expected spare-first
ordering; observation now follows the producer's actual intent-first order. Its
failed partial-retirement fixture was preserved and inspected.

Shared real-restore setup is extracted into a reusable `Restore` fixture so the
second Location uses the same App and writer rather than a fabricated receipt or
second Store. Reclamation App fixtures now preserve their root on panic. CI runs
this App suite independently of earlier failures under the same short retained
fixture root as Git/CLI proofs; its source filter explicitly includes the new
support module. No production API, wire schema, generated SDK or default Git
admission changes.

The full default reclamation selection before adding the final race passes 44
entries in 289.68 seconds. The final three Location tests, including all seven
scenarios and native-owner sequence assertions, pass in 36.44 seconds by default
and 37.03 seconds with native-git-acceptance under a short fixture root. Default
and native-feature App all-target Clippy with warnings denied pass after separating orchestration
and resumption (the initial combined helper measured 24/15; no allowance added).
Focused code-review-skill review covers original-identity refusal, unrelated
Location progress, pending-plan preservation, post-unlink route revalidation,
exact-plan resumption, source/event invariants and failure evidence retention;
no outstanding finding is identified in this test continuation. Whole-branch
review, full workspace/SDK runtime, managed removed-Location cases, constructor
cuts and fresh-source platform acceptance remain required.

Formatting, whitespace, workflow YAML and independent-step/retained-root checks
pass. Strict OpenSpec validates all 100 items; spec lint reports 0 errors and 25
warnings. Native C deprecation and linker unwind diagnostics were observed without
assigning origin. Fetching main again finds no upstream commits missing from the
branch. The native workflow must execute the new source before accepting these
cases as cross-platform evidence.

These tests verify the current live-root route. Positive maintenance after
permanent root removal still needs a separately validated detached authority and
coordination path; the current checks were not relaxed or replaced, and this
remaining capability has not been marked complete. M6/M8, parent tasks and issue
#93 remain open.


Direct JSONL request ownership continuation (2026-11-10): assistant-step events
now capture the complete active-path input cohort before inference. Runtime append
and replay validate nonempty proof against branch-aware state. Private step
snapshots preserve it across continuations and compaction without extending the
public snapshot schema. Pending requests expose optional `input_message_ids`
from the exact recorded step/tool call, never from the latest global promotion.
Legacy or oversized evidence remains unknown; proof is bounded to 32 identities
of at most 256 bytes, with no partial cohort accepted. The driver requires every
identity to belong to its pending inputs before collecting/replying/closing a
direct request. External correlation labels confer no authority. OpenAPI and
TypeScript types are regenerated.

Actual shared-App/Session controller tests cover permission and question requests,
foreign reply refusal, close while the first owner remains attached, normal owner
reply and a closed driver's own later request. Removing the guard reproduces the
foreign-request collection failure; the failed fixture is retained. Separate real
binary tests verify request proof and stdin permission/question replies. Server
regressions cover co-promoted inputs, later steering, actual compaction, inactive
branch promotions and contradictory replay. Runtime append preflights step proof;
raw Store append is not claimed to validate SessionState.

Verification passes: 3 provenance unit tests; 443 server library tests with one
explicit subprocess-fixture test ignored; 22 request tests; branch checkout (1),
context compaction (19) and structured-root (16) integration tests; 33 exec unit
tests and 12 JSONL binary tests. Server/CLI all-target Clippy passes with warnings
denied. OpenAPI exact check and SDK generated-file check pass; SDK typecheck
and tests pass (136 passed, 10 configured external skips). These are local checks,
not full-workspace or current-source cross-platform acceptance. The native workflow
now independently runs provenance/request regressions alongside JSONL tests and
retains failed shared-App fixtures under the short artifact root.

Focused code-review-skill review finds no outstanding finding in this direct
request increment. It follows the existing runtime/Store/CLI boundaries; SQL uses
bound parameters, unknown proof grants no driver ownership, replay preserves
legacy readability, and generated API changes are additive. The new producer,
fold, lookup and driver guard have unit/integration coverage. Whole-branch review
and current-source native acceptance remain required. Session-wide interruption
and interactive leases, descendant/hook requests, input-scoped background jobs,
output backpressure and broader event/result correlation remain implementation
gaps. No claim is made that releasing a Session-wide lease cannot affect another
owner without its own attached presence. Tasks 6.2–6.4 and issue #93 remain open.

Final checkpoint checks: formatting, diff whitespace, workflow YAML and the
independent JSONL step/retained-root assertions pass. The CI-pinned OpenSpec
1.13.2 validates 100/100 items; cross-spec lint reports 0 errors and 25 warnings.
Other cached OpenSpec versions were not used for acceptance. The SDK generation
check is clean. No parent task is checked off by this increment.

Native CI follow-up: run 42790242409 at af0d10ed passes macOS ARM/Intel
but fails both Linux jobs during the managed metadata child fixture setup, before
its observed cut. The log reports SUN_LEN at the nested control/fixture root.
The child now inherits the workflow's short TMPDIR instead of nesting beneath
the control directory. It still retains its own failure root and writes the actual
root into fixture.json for reopening; the workflow uploads both roots. The actual
managed metadata process-death selection passes locally after this correction
(two entries, including the inert child entry and parent exercising both cuts).
Formatting and diff checks pass. Linker unwind and native C deprecation warnings
were observed without assigning origin. Linux execution remains required.

Instruction budget continuation (2026-11-10): the approved additive
`instruction_limits` object now validates positive integer file/total limits
(defaults 32768/131072, file <= total), rejects nonobjects/unknown members, and
uses normal member layering, profiles and CLI overrides. Project definitions
(including profile members) use the existing sensitive-definition trust digest;
held definitions cannot change trusted global limits. `instructions` array
concatenation is preserved. No managed-policy namespace is added or widened.

The typed limits flow through ToolHost, BuiltinHost and AppHost into baseline and
nested loading and doctor. Nested text honors a configured total below its 32 KiB
ceiling. Configuration failure withholds instruction reads without hiding date
or environment; first-epoch initialization keeps input retryable with no model
call, and repair resumes it. Diagnostic names now identify the actual setting.
The maximum-integer read limit saturates its lookahead instead of overflowing.

Review/regressions caught and resolved a missing AppHost forwarding method,
Serde accepting an empty array for a defaulted struct, missing project trust
gating and maximum-limit overflow. The real-App regression inspects actual
provider baseline and nested text, including UTF-8/file/total cuts. Verification:
88 configuration tests; 444 server library tests with one explicit subprocess
fixture ignored; 20 context/compaction tests; 9 instruction App tests; the AppHost
forwarding check; 2 host configuration tests; and the doctor regression pass. The
focused maximum-limit read test passes after the arithmetic fix. All-target
Clippy for core/server/tools/App/CLI passes with warnings denied after moving the
doctor test module to the end of the file; no allowance is added. Native C and
linker unwind warnings were observed without assigning origin.

Formatting, diff checks, SDK generation consistency and workflow YAML/independent
step/log checks pass. CI-pinned OpenSpec 1.13.2 validates 100/100 items; lint reports
0 errors/25 warnings. No HTTP/CLI envelope or generated artifact changed; SDK
runtime tests and full workspace tests were not rerun for this continuation. The
native workflow now selects and retains these instruction/context regressions.
Current-source native/platform acceptance remains required.

Focused code-review-skill review finds no unresolved finding in this settings
wiring. Full rendered assembly accounting (labels/notices/omitted inventories),
trusted path-rule discovery/globs, successful-operation/Epoch/digest-bound
reminders, changed/revoked definitions, effective context inspection and the TUI
flow remain required. Task 4.1 and all other parent tasks stay open; this does not
accept rules/context, the recovery branch or issue #93.

Rendered instruction assembly continuation (2026-11-10): `core/instructions`
now budgets complete file labels, bodies, separators and truncation notices.
Nested limits also include the system-reminder wrapper and bounded lists of whole
file labels. Tiny budgets withhold files instead of emitting partial labels or
notices. Typed bounded bodies retain their actual limiting budget and omitted
byte count, so UTF-8 cuts and per-file/aggregate notices remain truthful. Other
Context Sources and the provider/agent prefix have separate accounting.

Reminder candidates carry the exact typed files they label. Durable settlement
IDs and InstructionsLoaded paths come from the successfully finalized candidate,
not the entire planned inventory or untrusted rendered text. In-group deduplication
occurs after finalization, so a failed attachment cannot reserve another call's
files. Bounded authorized rediscovery of labelled importers keeps unlisted children
eligible; a cut final import line is never interpreted as a different filename.
Read gates, confinement, regular-file checks, cycle/depth checks and bounded input
reads remain on rediscovery. Source-graph exhaustion/deep graph/restart acceptance
still belongs to the remaining discovery and provenance work.

Verification passes: 449 server library tests (one explicit subprocess-fixture
entry ignored), 20 context/compaction tests, 10 real-App instruction tests and the
doctor regression. The App tests inspect actual provider text against the stored
source and enforce complete reminder byte limits. The new bounded-list App case
compares durable Epoch identities with complete labels in two actual tool results,
verifying later progress without consuming unlisted imports or repeating prior
identities. Unit coverage checks UTF-8/notice boundaries across small budgets,
bounded pointer prefixes, importer retry and truncated-path refusal. An earlier
assertion expecting the full 32 KiB body plus metadata was replaced by complete
rendered-size checks and positive content/identity checks.

Affected server/App/CLI all-target Clippy with warnings denied, formatting, diff,
SDK generation consistency and workflow YAML/selection checks pass. Strict
OpenSpec 1.13.2 validates 100/100 items; lint reports 0 errors/25 warnings. Native C
deprecation and linker unwind warnings were observed without assigning origin.
Main was fetched with no missing upstream commits. The native workflow includes
assembly/reminder units and the new support-module filter, alongside real App
acceptance and retained logs. SDK runtime/full workspace and fresh-source platform
acceptance have not run for this increment. No API/event envelope changed.

Focused code-review-skill review checks complete-boundary rendering, typed
delivery authority, post-finalization consumption, permission/confine checks on
rediscovery and partial-line refusal. Full rules/context review remains open for
trusted definition identity, source-graph exhaustion, operation/Epoch/restart and
revocation binding, effective inspection and the TUI flow. Task 4.1 and parent
acceptance remain incomplete; no goal or delivery completion is claimed.

Trusted rule discovery/read continuation (2026-11-10): global `rules/` and
project `.claude/rules/*.md` / `.cyber/rules/*.md` now use the fenced native
definition reader, sorted directory precedence and validated project-relative
globs. Unscoped bodies join the baseline; matching scoped bodies enter successful
built-in read results. Claude opt-out and project-config opt-out are respected.
Frontmatter is not rendered or used to configure permissions. Imports retain
confinement/read/depth/cycle checks and recognize rule targets, so importing a
scoped rule cannot turn it into unconditional instruction text.

Exact bounded rule-file bytes enter the existing checkout trust digest. Loading
requires an approved expected digest and the original Location's read gate before
project definition capture, followed by fenced byte/binding verification. Nested
definitions resolve the operation directory's reviewed inventory in the same
checkout; a foreign nested checkout cannot contribute rules. Changed bytes remain
held until renewed review. No #92 trust-key policy or approval-file format changes.
Trust inventory paths are canonicalized at the declared checkout root, preserving
ordinary caller aliases while retaining no-follow checks below it. Explicit global
root aliases cannot bypass scoped-import classification.

Read delivery now persists optional typed `rule_reminders` (root, definition path,
byte digest and approval generation), separate from legacy canonical instruction
paths. The Epoch tracks rule identities separately; historical events default to
no rule bindings. Actual App Store-event inspection and replay verify this
separation. Directory/import duplicates are captured/charged only once per
discovery, preventing a valid near-limit graph from spuriously exhausting its
definition budget. Full operation/config-generation/Epoch binding, edit/patch
and partial-effect delivery, revocation withdrawal, graph-exhaustion and restart
acceptance remain required under 4.2–4.4. No parent task is checked off.

Verification passes: five selected core library tests (four rule tests and the
existing untrusted-rule table check); all 89 configuration tests; 454 server
library tests with one explicit subprocess-fixture entry ignored; all 15 real-App
instruction tests; 20 context/compaction tests; AppHost forwarding; nine path-skill
compatibility tests; and the doctor regression. App tests cover withheld/approved
rules, baseline/read separation, once-per-Epoch identity, renewed review after
edits, nested/foreign roots, read-denied definitions, Claude opt-out, imports,
bounded provider text and typed durable bindings. Native link/hardlink refusal,
invalid/null/empty/escaping glob refusal and inventory bounds have core coverage.
OpenAPI's exact consistency test and SDK generation check pass; no HTTP schema or
generated SDK change is required. Full workspace/SDK runtime and fresh-source
platform acceptance have not run for this continuation.

Focused code-review-skill review resolved empty-array/null fallback risks, alias
classification, duplicate budget charging and mixing opaque rule IDs into the
legacy path field. Architecture follows the existing core fenced reader and
runtime ToolHost authorization port. Full rule/context review remains open for
exact native operation/config/Epoch revalidation and provenance, withdrawal after
revocation, graph/restart boundaries and effective inspection/TUI. Native C and
linker unwind warnings were observed without assigning origin. The focused native
workflow now includes core rule and trust tests plus skill compatibility checks.
Task 4.1 and all parent acceptance/delivery obligations remain open.

Final local gates: core/server/tools/App/CLI all-target Clippy with warnings denied,
formatting, diff checks and workflow YAML/source-selection checks pass. Strict
OpenSpec 1.13.2 validates 100/100 items; cross-spec lint reports 0 errors/25
warnings. The worktree must execute on fresh-source CI before platform acceptance.


Nested read context fence continuation (2026-11-10): capture the active Location
and Context Epoch before running the built-in read, reject absent/stale contexts,
and recheck before discovery and again under the settlement lock. Resolve current
instruction settings on the blocking discovery thread after successful execution;
retain no pre-effect permission/trust callbacks. Normal grouped settlement and
exact `/approve` replay use the same final fence. Suppression leaves successful
output intact and records no reminder identity, so a later operation can deliver.
Regressions cover replaced/missing/stale Epochs, moved Locations, and a gated
successful read whose configuration becomes unavailable before settlement, then
restored configuration and successful delivery on a later read. This does not
complete 4.2: native operation receipts, exact configuration-generation fencing
through commit, edit/patch and successful partial effects, withdrawal/revocation,
physical restart and effective inspection remain required. No parent task closes.

Checkpoint verification: server library 455 passed / one ignored nested lock
probe; context/compaction integration 21 passed; production App instruction
integration 15 passed; HTTP override integration filter two passed. Server/App
all-target Clippy with `-D warnings`, formatting/diff checks, generated SDK
consistency and CI-pinned strict OpenSpec (100 items) passed. Spec lint reports
zero errors and 25 warnings; native C/linker warnings were observed without an
origin claim. This increment did not rerun the entire workspace, SDK runtime or
platform matrix. Checkpoint review using the code-review skill found no new
findings within these changes: existing runtime/host boundaries and atomic
settlement are retained, unavailable configuration holds reminders, both writer
paths revalidate the context, and regressions cover suppression and retry.
Whole-goal review and source-bound native acceptance remain open.


Native Location observation continuation (2026-11-10): add a metadata-only
DirectorySnapshot using the existing no-follow native source reader's directory
chain and identities. RootBinding retains the physical Location and project root,
checks the declared Location alias still names that directory, and revalidates
all native ancestors and the nearest project boundary. Unix retains handles;
Windows retains full native IDs without holding directories open through the
operation, preserving rename support. Runtime read targets retain the binding;
normal and exact-override settlement check it before discovery/settings, after
settings/discovery and under the settlement lock. No new file-operation or source
permission is granted. Root/ancestor replacement, removal, symlink substitution,
alias retargeting and changed nested checkout boundaries withhold reminders.
Ordinary edits preserve the binding. A gated runtime read regression verifies a
replacement checkout cannot supply nested guidance for the old call, while a
fresh operation can load it without the old call consuming delivery. The existing
rule/override and budget behavior remains covered by production App regressions.
Checkpoint review found and fixed the declared-alias retargeting case; its native
symlink regression now covers both alias retargeting and physical-path link
substitution. These are point-in-time native observations, not proof of the file
handle used by the effect. Durable rule identities still use logical root paths;
actual per-file/native-operation receipts, root incarnation in durable provenance,
edit/patch/partial-success delivery, exact configuration-generation fencing,
revocation withdrawal, restart and inspection remain open. No parent task closes.

Next producer integration points from code inspection: the successful read in
`cyber-tools/src/tools/fs.rs`, `guarded_write` after its write, and each completed
`write_one` in `tools/patch.rs`. Patch completion paths already distinguish files
applied before failure. Collect actual authorized per-file effects there, including
hook-rewritten inputs, rather than deriving delivery from pre-hook arguments or
the overall tool status. RootBinding alone must not be treated as that receipt.

Final local verification after the alias fix: eight filtered Core tests passed
(seven rules tests plus the existing untrusted-rule table test), server library
455 passed / one ignored subprocess lock probe, context/compaction 22 passed,
production App instruction integration 15 passed, and Core/Server/App all-target
Clippy with `-D warnings` passed. Formatting/diff checks, workflow YAML parsing,
generated SDK consistency and CI-pinned strict OpenSpec (100 items) passed;
spec lint reports zero errors and 25 warnings. Native C/linker warnings were
observed without assigning their origin. Main was fetched and remains fully
integrated at `8266db6c`. SDK runtime, entire workspace and Windows/native platform
acceptance were not rerun locally for this increment. Workflow source filters
include the shared native snapshot helper; its existing rules/context/App steps
exercise the new cases. Current-source native execution and whole-goal review
remain required before delivery; no goal/task acceptance is inferred from these
local checks.


Native read receipt continuation (2026-11-10): built-in file reads carry the
receipt of their actual open handle through a typed ToolHost/App execution port.
The pre-authorization physical target must still match at open. An alias with a
separate non-allow/taint/classifier decision cannot attest the canonical target.
Actual workspace reads preserve their size and hard-link behavior; strict rule
and memory definition readers keep existing link restrictions. Read fallback,
listings, denied/failed results and unsupported receipts cannot activate scoped
rules. The capture uses actual post-hook input, not the provider's original path.
The receipt revalidates native root, file and ancestor IDs and declared aliases
before/after discovery and under settlement. A successful `rule_read` record
binds native evidence, Location/Epoch, dispatched attempt and operation key to
the rule reminders in the same Store event. New rule IDs incorporate the native
root; historical path-only IDs remain compatible. Replay refuses to consume a
new identity on a mismatched dispatch, Location, Epoch or root. The native
metadata describes the past read and grants no future filesystem authority.

Review fixed receipt reuse across host retries, separated workspace hard-link
observation from strict definition/storage readers, required UTF-8 receipt paths,
and fenced the authorization-time physical target. Actual App hook-rewrite
acceptance verifies only the database rule reaches the provider after a web read
is rewritten, while the original call input remains in the transcript. Store
replay checks Location/Epoch/attempt/key mismatch, and Core tests exercise actual
handle bytes, alias retargeting and large/hard-linked targets. This is read
provenance; edit/patch/partial-effect receipts, exact config generation through
commit, rule withdrawal, full restart and inspection still prevent completion
of 4.1–4.4. No parent task closes.

GitHub native run [42890123485](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/42890123485)
passed at `c9e0c373`. The later `fa2c9dfb` and `25e7e89f` runs were cancelled;
those results do not establish native acceptance of their changes or the new
receipt code. Main is fully integrated at `8266db6c` after fetch.

Local receipt verification: 11 filtered Core tests, nine import snapshot tests,
455 server library tests (one ignored subprocess probe), 22 context tests, 16 App
instruction tests (the command-hook rewrite fixture is Unix), and the AppHost
forwarding check passed. Core/Server/Tools/App all-target Clippy with `-D warnings`,
formatting/diff, workflow YAML, generated SDK consistency and pinned strict
OpenSpec (100 items) passed; spec lint reports zero errors and 25 warnings.
Native C/linker warnings were observed without an origin claim. The shared native
reader's import tests are included in the platform workflow. Windows, whole
workspace, SDK runtime and current-source native matrix acceptance remain unrun
locally for this increment. Checkpoint review found no remaining findings in the
read changes after the fixes above; full-goal review remains open.

Existing-file edit continuation. The guarded edit captures the authorized
physical path and verifies the handle that completed the write against the
observed file. The settlement stores the edit receipt under `rule_edit` with the
native root and file identities, dispatched attempt, Location and Context Epoch.
Replay checks those fields against the edit dispatch. The App regression covers
a denied read followed by a successful edit in the same Epoch. The Core test
rejects a writer handle for an unrelated file. An edit through a separately named
alias can still produce its normal tool result without a scoped rule receipt.
Multi-file patch, creation/removal receipts, partial patch settlements, exact
configuration generation and revocation/restart acceptance remain open.

Local edit verification passed 12 filtered Core tests, 17 App instruction tests,
and all-target Clippy for Core, Tools, Server and App. Formatting, SDK consistency
and strict OpenSpec checks passed. Windows and platform acceptance remain pending.

Direct patch continuation. A patch now retains up to 32 verified completed file
effects and loads their rules under one discovery and reminder budget. Updates
and exclusive creations bind the completed writer handle. A deletion records
the pinned native parent and removed name after a successful relative unlink and
verified absence. This evidence describes the namespace operation. The patch
keeps its error status after a later failure. Only completed targets can deliver
rules and each labelled binding retains its matching path. Historical read and
edit settlement fields still decode. Shell-routed patches, exact configuration
generation, revocation and restart acceptance remain open.

The App regression covers updates, creation under a new parent and deletion,
plus a later precondition failure that prevents the remaining effects. It checks
the actual tool output, recorded paths, error status and replay identity. Replay
does not mark a rule delivered when its matching receipt names another target.
The Core test confirms a replaced parent prevents the observed removal.

Local patch verification passed 13 filtered Core tests, 18 App instruction tests,
455 Server library tests (one ignored subprocess probe), and all-target Clippy
for Core, Tools, Server and App. Strict OpenSpec validated all 100 items. Spec
lint reported zero errors and 25 warnings. Formatting and generated SDK
consistency passed. Windows, full workspace and SDK runtime suites were not
rerun locally. Current-source native acceptance remains pending.

Intercepted shell patch continuation. The built-in patch path records the same
authorized receipts for Bash and PowerShell interceptions. Shell settlement
retains the original dispatch name and the interception metadata. Replay
requires that marker in addition to the attempt, Location, root, operation
target and Context Epoch. A regular external command supplies no built-in
patch receipts. App regressions cover complete and partial Bash interceptions
and reject replay after removing their interception metadata. The shell suite
checks that an edit denial and external program execution cannot supply
receipts. The external program and login shell cases check routing and receipts
because the external program can legitimately change the fixture file.

Local verification passed 8 shell patch tests, 18 App instruction tests, 455
Server library tests (one ignored subprocess probe), and all-target Clippy for
Tools, Server and App. Formatting, workflow YAML, strict OpenSpec (100 items)
and SDK consistency passed. Spec lint reported zero errors and 25 warnings.
The PowerShell receipt assertion is Windows-specific and did not run locally.
Full workspace, SDK runtime and current-source platform acceptance remain open.
