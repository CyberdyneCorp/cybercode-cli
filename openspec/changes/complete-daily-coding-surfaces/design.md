## Context and baseline

Planning baseline: `097b6278`. `cyber-server/src/runtime/rewind.rs` already commits staged revert through durable events and its comment describes preserving a Session branch. `cyber-cli/src/commands/sessions.rs` still reserves `rewind`. `cyber-snapshot/src/lib.rs::shadow` returns no snapshot for non-git Locations. `runtime/structured.rs` provides child result validation, but Session creation currently refuses `output_schema` without a parent. Exec reads ordinary stdin as prompt text.

Canonical requirements also need reconciliation before coding: Three-phase revert says commit deletes later messages while Rewind targets and Session branches preserve them. Session structured output and exec structured-final wording differ on retries. This change defines one user-visible behavior and keeps existing child compatibility.

## Decisions

### 1. One rewind engine and preserved branches

All entry points call the server-owned revert engine under idle/checkout mutation ownership. `cyber sessions rewind <session> --to <message> --target code|conversation|both` uses a preview/confirmation flow for code changes; non-interactive invocation requires an explicit reviewed confirmation option. Freeze message, branch, Location and snapshot identity for the preview and revalidate before applying. A changed baseline requires a new preview; do not silently overwrite edits. Show excluded files, unavailable snapshots and known external/unknown effects.

Conversation rewind changes the active path and preserves later messages as an inactive branch; unresolved native/tool recovery stays independent of transcript visibility. Reconcile the obsolete destructive Three-phase revert wording in canonical and overlapping deltas. Code-only rewind keeps the conversation. Conversation-only rewind remains available when snapshots are disabled/unavailable. Stage/clear/commit retain the existing baseline and conflict checks, including restart. `/undo` stages the preceding user boundary and restores its text in the composer; `/redo` advances the staged boundary or clears it, scoped to the same Session/Location. Busy Sessions require an explicit interrupt and acknowledged idle state before a fresh preview. Switching context invalidates stale menus and callbacks.

### 2. Bounded non-git snapshots

Generalize the existing Snapshots host interface to select shadow Git or a content-addressed file-copy backend. Non-git objects use BLAKE3, private atomic durable writes and a Location identity-bound manifest. Size/mtime can narrow candidates but never establish byte identity: hash before trusting a reused snapshot, including same-size/same-mtime edits. Enumerate only allowed regular paths without following directory symlinks; apply configured ignores and per-file limits, exclude owner/private state and report every skipped class. Preserve file type/executable metadata without storing unsupported device objects.

Use the canonical 200 MiB per-Location fallback limit and seven-day default retention. An exhausted limit disables code rewind with a visible reason while conversation rewind continues. Reference accounting protects staged baselines, inactive branches and unresolved restore evidence; zero/disabled settings do not delete required recovery data. Restore performs the same three-way conflict analysis, mutation lease, recheck and backup/journal protocol as Git snapshots. Crash recovery never infers that a partly applied restore succeeded or blindly reapplies it. Test path replacement, symlinks/hardlinks, external edits and interrupted object/manifest/restore commits natively. Native storage limitations produce truthful unavailable states rather than unsafe fallback.

### 3. Rule discovery and inspectable effective context

Load `.cyber/rules/*.md` and enabled `.claude/rules/*.md` at the existing directory precedence. Rules without `paths` join the baseline. Relative `paths` globs activate after a successful authorized read/edit/patch of a matching path; denied/failed operations do not consume once-per-Epoch delivery. Successful partial file settlements retain correct reminder ownership. Bind project root, definition digest, trust/config generation, successful operation identity and Context Epoch. Keep existing directory instruction ordering and `AGENTS.override.md` replacement semantics; do not use a project rule to widen tool/permission authority.

Rule definitions use the existing fenced native source reader (no linked/hard-linked/special sources), with full definition bytes represented by SHA-256 in the existing checkout trust inventory. The reviewed manifest remains private configuration metadata. Project loading requires a trusted expected digest, same checkout root and the Location's read gate before definition capture; captured bytes and retained native bindings are verified before rendering. The operation directory's ancestry selects nested definitions using existing checkout-digest approval semantics, without changing #92's pending trust-key policies. Global owner rules use `<config>/rules`. Directory text order is ordinary instruction files, sorted Claude rules when enabled, then sorted Cyber rules, with root directories before more specific ones. Imports retain existing confinement/read/depth/cycle checks and recognize scoped rule targets instead of loading their frontmatter as unconditional text.

Definition discovery bounds are 128 project files, 4096 examined entries per rules directory, 1 MiB per fenced definition and 4 MiB per inventory/loader; frontmatter is capped at 16 KiB and 64 portable relative globs of at most 256 bytes. Invalid/null/empty `paths` and unknown authority-bearing keys are errors, never baseline fallbacks. Rule identity includes root, definition path, exact bytes and approval generation. Successful delivery stores optional typed `rule_reminders` separately from the legacy canonical `instruction_reminders`; historical events default to no rule bindings. Exact native operation/config-generation/Epoch binding and withdrawal/restart behavior remain required under 4.2–4.4.

Nested read discovery resolves instruction settings after a successful effect. Its captured Session Location and Context Epoch must still be active before discovery and again under the settlement lock. An absent/stale/replaced Epoch or changed Location suppresses the reminder without recording delivery; normal group settlement and exact `/approve` replay share this check. The captured read also retains the native directory identities from the volume root through the physical Location, verifies the project-root boundary, and rechecks those bindings before/after discovery and under settlement. Unix retains directory handles to prevent inode reuse; Windows releases handles after capture and verifies full native IDs, so the observation does not freeze editor/worktree renames. Ordinary file content edits do not invalidate the root. Replacement/removal, changed ancestors, linked substitution, a retargeted declared Location alias or a changed project boundary withhold the reminder. These point-in-time context/root checks do not prove which file handle performed the effect: actual per-file operation receipts (including partial writes), exact configuration-generation fencing through commit and revocation withdrawal remain required.

Built-in reads now read through an observed file handle after authorization and any input rewrite, and hand the resulting receipt to the runtime through the host execution boundary. Workspace read targets preserve the normal size and hard-link behavior; rule-definition and memory-storage source readers keep their stricter checks. The receipt retains actual native file/ancestor identities, a physical path and the captured project root. Unsupported observations keep the read result but grant no scoped-rule delivery. Rechecking a receipt reads metadata, not file contents. The ordinary legacy host path cannot activate scoped definitions. Successful labelled delivery stores `rule_read` atomically with `rule_reminders`: actual target/root IDs, captured Location/Epoch, operation key and dispatched attempt. Version-2 rule IDs incorporate the native root; old IDs and legacy records replay unchanged. The recorded proof describes an earlier operation and never grants new filesystem authority. Replay checks the dispatch and context before folding new identities. An existing-file edit compares the completed writer handle with the observed native file and records `rule_edit` under the same dispatch and Context Epoch checks. A failed read leaves its scoped rule eligible for that edit. Direct patches collect up to 32 completed native effects in execution order. Each delivered rule records its matching physical target and the settlement stores the effect receipts under `rule_patch`. The shared discovery and rendering budgets apply to the whole patch. Creation uses the completed exclusive writer and bound parent. Removal executes relative to the observed parent directory and attests the successful removal of its name and the resulting absence. A later patch failure preserves the error status and retains the completed effects. Bash and PowerShell interceptions reuse the patch receipts and replay requires the interception marker for shell calls. Exact configuration generation and revocation/restart acceptance remain open.

Represent rules as typed Context Sources so inspection and model assembly use the same effective data. `GET /api/v1/sessions/{id}/context` and `/context` report Epoch, source identity/origin, estimated tokens largest first, source status, pending changes, totals and utilization with unknown denominators explicitly unknown. Inspection is read-only: it must not start inference, launch integrations or load an untrusted/denied file just to count it. Bound response sizes and omit secret-bearing source content by default. Complete the existing 32 KiB per-file and 128 KiB aggregate instruction-budget semantics through the proposed additive `instruction_limits: { max_file_bytes: 32768, max_total_bytes: 131072 }` configuration. Preserve the `instructions` array type. Update existing target wording that expresses this as `instructions.max_bytes`, since that property cannot coexist on an array. The new setting and its override/managed-policy behavior are part of artifact review. The setting follows normal per-member configuration precedence, including selected profiles and command-line overrides. Project definitions, including those inside profiles, use the existing sensitive-definition trust gate because they can increase host text reads. It does not add a managed-policy namespace or allow project/CLI writes to `policy`; unsupported managed keys keep their existing refusal behavior. Runtime and doctor require the resolved limits before reading instruction files. Nested reminders retain their 32 KiB ceiling and honor a lower configured total. The complete rendered baseline and nested reminder fit their total budgets, including labels, notices, separators, wrappers and bounded file lists. Delivery records use the exact typed set of labelled files, never parsed content. Unlisted imports remain discoverable through bounded authorized re-observation of their parent, and in-group deduplication happens only after successful finalization. Truncated import lines grant no path authority. Path-rule definition/operation/Epoch identity and effective inspection remain required.

### 4. Root structured output is per admitted prompt

Expose canonical per-prompt `format: { type: "json_schema", schema }` on the ordinary root admission path. Normalize into the shared validator without requiring a fake child Session or changing child `return_result`/SchemaMismatch behavior. Validate schema before admission and apply a proposed 64 KiB encoded schema bound, with a maximum nesting depth of 64, no network reference resolution, explicit refusal of external references, and bounded local-reference compilation through the shared validator. Root prompts get a dedicated `structured_output` finalizer. The tool result is validated and stored on the matching assistant/input attempt; partial JSON or free text is not a successful structured answer.

Use one initial attempt plus at most two validation corrections for root prompts, bounded by the remaining ordinary budgets. Three invalid submissions or termination without a valid finalizer return `StructuredOutputError`; explicit interruption stays interrupted. Context compaction/restart/resume cannot reuse another input's result or reset the retry count to obtain unbounded inference. Store schema identity, attempt and terminal result so lost client acknowledgements recover the original outcome. `--output-schema <file>` maps to this contract; JSON output puts the validated object in `result`, text prints it as formatted JSON, stream output emits one matching result. Maintain existing unstructured output and child consumers.

### 5. Persistent JSONL input owns its admissions

Implement `--input-format stream-json` as a bounded asynchronous reader and protocol dispatcher; ordinary stdin remains a single text prompt. Proposed initial limits are 1 MiB per line and 32 pending prompt admissions. Capacity exhaustion or malformed/unknown envelopes produces an explicit protocol error without silently dropping a user prompt or treating it as shell/text. Input parsing never waits for model completion, so a subsequent interrupt or permission/question reply can be handled. Use normal typed durable admission and retained client request IDs rather than posting unidentified prompts.

Support the existing canonical user/text-or-parts/delivery, interrupt, permission_reply, question_reply and end frames. Require exact live request ownership for replies; the streaming transport does not fabricate browser/TUI presence and does not approve anything automatically. The stream's negotiated request/reply behavior may keep an owned ask pending only while that driver is live. On EOF/end stop accepting frames, drain acknowledged pending work in defined order, and terminate after all owned inputs settle; disconnect cannot leave asks waiting forever. Never cancel unrelated work in a reused Session. SIGINT follows owned interruption and exit 130. Lost acknowledgement and restart use request lookup; uncertain native outcomes retain their ordinary fences.

Request ownership must come from durable server evidence. Add optional
`input_message_ids` to pending permission/question requests. Capture the complete
active input cohort in each assistant-step event before inference, validate it at
runtime append and replay, then bind requests to that exact step and tool call.
Never infer ownership from globally latest promotion history: branch checkout can
make those promotions inactive. Active-path snapshots retain the cohort through
tool continuations and compaction; later steering cannot reassign an older call.
Bound proof to 32 identities
of at most 256 bytes each; missing, malformed or oversized evidence grants no
driver ownership. Historical steps/requests without this field remain readable with
unknown provenance. Direct request reply/close guards require every cohort identity
to be a pending input of this driver and the exact Session. Scoped interruption,
input-scoped presence, descendant ownership and other event/result correlation
remain separate implementation obligations.

Process and subagent Job reservations capture optional `input_origin` metadata from the original parent step/call while its dispatched attempt and operation key match the durable tool-call record. The complete input cohort comes from the recorded step, including across later steering and handbacks. Historical or detached Jobs expose unknown provenance through an absent field. The JSONL driver still requires integration with this evidence before background adoption, result retirement and cancellation can be accepted.

Job completion admits its canonical handback input and `job.notified` record in
one Session commit before waking inference. An exact-retry path also reconciles a
historical admission without its notification record. Request/Job provenance for
a handback-driven call resolves its direct step inputs through the immutable
`job.started` origin only when the corresponding handback admission and matching
notification precede that step. A public prompt's source label or handback-shaped
ID alone grants no ancestry. Merge the complete original cohorts with any new
steering input, deduplicate in order and refuse inferred ownership when the
result is unknown or exceeds the 32-identity/256-byte limits. No history-dependent
latest-promotion heuristic or unbounded in-memory Job ancestry map is introduced.

`POST /jobs/input-state` accepts one Session and 1–32 unique input identities,
each at most 256 UTF-8 bytes. It returns one small running/unadmitted-handback
summary per supplied identity, aggregating only Jobs whose complete nonempty,
bounded durable cohort belongs to the supplied set. Missing, malformed, duplicate,
oversized or mixed-cohort Job provenance does not grant ownership. The query
aggregates at the Store and returns no Job payloads or accumulated history pages.
The observation now includes `handback_pending` while the latest step driven by
an admitted notice has unfinished model/tool work, and `session_seq` from
the same Store snapshot. This remains a read-only observation; it does not finish
an input or cancel work. `GET /sessions/{id}/steps/{message_id}/input-origin`
resolves the bounded original cohort of an exact stored step through the same
durable handback evidence, without requiring a tool call or returning prompt text.

The JSONL driver retains answered input slots while owned Jobs run, notices await
admission, handback steps remain pending, or the query revision is ahead of the
consumed durable stream. Its owned answer boundary must also follow the latest
handback admission (`handback_seq`); an earlier ready answer cannot cover a later
notice. Every callback of a tool-call step must settle before that handback's work
is considered observed complete. Step ownership gates answer text/failure correlation.
Ready slots retire independently of another driver's active Turn. Replace the
all-new-Session-Jobs baseline with these exact bounded observations. The owned
Session stream replays from the captured revision and opts into live idle/delta
notices with `live=true`; live frames do not advance its durable SSE cursor. The
default Session stream remains compatible. Missing/gapped ownership evidence
reports retained work requiring reconciliation rather than fabricating a result.
Scoped cancellation/presence, descendant asks and complete restart/lost-ack
acceptance remain separate obligations.

Scoped interruption uses an exact Session, 1–32 unique original input IDs and a
stable interruption request ID. The runtime retains the selected cohort and a
receipt before signalling any actor. Retrying that request returns the original
receipt; it never captures a later Drain. Reusing its ID for a different cohort
refuses. A receipt left pending by a different runtime generation reports unknown
and requires reconciliation instead of signalling replacement work.

Capture queued refusals and the active Drain under the Session state lock. Cancel
that Drain only when its complete recorded cohort, including authenticated Job
handback ancestry, belongs to the selection. Mixed or unavailable ownership never
authorizes cancellation of the active Drain. Wait on the captured Drain's retained
owner disposal, independently of later Session execution. Runtime-owned cleanup
survives requester disposal and retains the shutdown lease until receipt settlement.
Cancel proven direct process Jobs in bounded pages after the Job-publication
barrier; suppress their completion notices. Do not infer descendant or native
activity ownership from Session membership. Unresolved work reports unknown and
preserves its ordinary recovery fences. Resume remaining queued inputs without
waiting for an unrelated replacement Drain. A control receipt acknowledges only
its proven runtime scope, never native-effect acknowledgement or rollback.

Expose scoped control through authenticated POST
`/sessions/{id}/interrupt-inputs` and retained GET
`/sessions/{id}/interrupt-inputs/{request_id}`, with generated SDK methods. Receipt
lookup derives `session_seq` from the actual committed receipt event instead of
trusting a serialized revision. The JSONL driver polls retained controls in its
existing event loop, bounds each control HTTP request to five seconds and retains
uncertain attempts for up to 30 seconds before reporting reconciliation. It waits
through the acknowledged receipt revision before retiring selected inputs, and
preserves newer inputs and their reply authority. Each additional outstanding
control must select a previously uncovered pending input, so the 32-input capacity
also bounds retained overlapping controls. A lost POST response looks up the same
request; an absent original receipt can resubmit that exact identity/cohort. Missing
scope routes, conflicting receipts and unknown outcomes never fall back to global
Session interruption. SIGINT retains aggregate exit 130 and fatal protocol errors
retain exit 2 while ordinary interrupt frames leave stdin open for later input.

Input-scoped presence uses a separate 45-second input-client lease, with the
existing monotonic expiry and Session/Location/capacity fences. It grants no
Session-wide or native computer presence. A JSONL prompt carries `input_client`
with its lease ID (or an explicit null client when attachment failed). Admission
validates the exact live input lease and commits an immutable client-binding event
with the input; that binding participates in exact retry identity. Retrying an
already admitted input returns its original receipt even after lease expiry and
cannot rebind it to another client. Bindings survive restart, while live authority
does not. Missing bindings retain legacy behavior; malformed or mixed scoped
bindings do not borrow another driver's, browser's or stream's attendance.

Direct requests and synchronous asker attendance resolve complete durable input
provenance, including authenticated Job handback ancestry, to that immutable binding.
Only one live input client bound to the whole cohort can keep its request waiting.
Release/expiry settles requests that lost their own approver without touching other
clients' requests. Scoped reply selection shares the attendance lock; expiry cannot
lend authority to a late reply. Reply-type mismatch preserves the original waiter.
Reject/always cascades select the same input client, and stop replies signal only
an exact Drain whose complete current cohort belongs to that request. Binding reads
require the adjacent original admission and matching digest; a scoped admission
with missing binding evidence stays unknown rather than borrowing legacy presence. Input leases neither authorize native control nor keep unrelated
legacy requests attended. The JSONL replay/live connection explicitly opts out of
Session-wide presence; ordinary event-stream defaults and browser/TUI attendance
remain compatible. Child/hook routing still requires its separate complete ownership
integration and does not gain authority from this direct-input work.

Emit schema_version 2, one result per admitted input with stable correlation, optional partial text/reasoning and hook events, and atomic `--output-last-message`. Session-level usage remains separate from per-input results and descendant usage. Redact errors and bound output buffering; define aggregate exits as 130 for explicit SIGINT, 2 for a fatal protocol error, otherwise the first nonzero input outcome in admission order (0 only when all outcomes succeed). A failing input does not discard already acknowledged later input; fatal protocol errors stop new admission and settle or explicitly cancel only owned work. Cover these precedence rules in protocol tests. Do not silently change text-mode output or the existing review/workflow/Goal command contracts; unsupported option combinations fail before creating work.

## Delivery and acceptance

Implement in order: revert spec reconciliation and CLI/TUI; non-git snapshots; path rules/context inspection; root structured output; JSONL driver. Each increment includes API/SDK/CLI generation, regressions, docs and meaningful production-path acceptance. Use actual temporary Git and non-git files, scripted provider/server process tests, TUI TestBackend plus real captures, crash/race tests, and native checks for filesystem behavior. Perform code/security review before delivery. Existing stale task headings are reconciled only against demonstrated clauses. On 2026-11-03 the user prioritized #93 after #105, superseding the original Plugins/hooks scheduling prerequisite. Start with the approved rewind slice while preserving unresolved cross-workstream dependencies; this scheduling change does not accept incomplete plugin enforcement or select pending policies. The workstream remains incomplete until all approved scope is implemented and the required checks have recorded outcomes.

### CLI/server preview increment

The first increment adds `/sessions/{id}/rewind/preview` and `/rewind/confirm`.
Preview snapshots current eligible files without changing source bytes or staging
a revert. Its bounded response includes exact Session/directory/sequence/boundary,
active tip, target/recorded/current snapshot identities, current exclusions, file
patches, retained tool statuses and an external/unknown-effect disclosure. A SHA-256
fingerprint binds the server-derived response; it is not an authentication token.
Confirmation recomputes the response, verifies fresh idle/scope/admission state and
holds the Session projection through restore and the stage/commit event batch.

The snapshot port requires a reviewed-restore implementation. Git rechecks the
current tree after acquiring its workspace mutation lock, then uses the existing
conflict-aware restore with fresh admission checks before file writes. Unsupported
custom hosts refuse code preview; conversation-only remains possible. A failed
native acknowledgement or event commit after file effects retains the native
activity fence and backup evidence. The existing one-step `/rewind` and explicit
revert stage/clear/commit routes retain their compatibility behavior; they do not
consume the new fingerprint. Full TUI/interrupt, crash/restart and non-Git backend
acceptance remain subsequent tasks. This increment does not claim arbitrary
external-writer atomicity or a new recoverable restore-journal implementation.

The CLI defaults to preview, with explicit `--confirm <fingerprint>` for applying
all targets. There is no automatic confirmation or busy-session interruption.
A changed preview requires review again, including when used by a script. An existing legacy staged revert must be explicitly committed or cleared before using the new flow; it is never silently finalized by another reviewed selection.

### TUI staged rewind increment

The TUI uses the reviewed preview/confirmation routes for final rewind and staged
undo/redo. `RewindSelection.action` is additive and defaults to `commit`; `stage`
keeps the original baseline across boundary movement, and `clear` requires the
exact retained boundary and target. Action and prior stage are bound into the
preview fingerprint. Targets cannot silently change while a stage is retained.
Code staging durably retains the union of paths it actually changed and the original baseline. It keeps the selected agent snapshot as its recorded image, without adopting unrelated owner files from a post-restore tree. Reviewed clear restores only those retained paths, including after event replay. Legacy operations cannot silently drop this ownership record.
Unsupported snapshot hosts refuse code staging; conversation-only remains usable.

`/rewind` and idle double-Esc open the boundary/target menu. `/undo` selects the
preceding user boundary and `/redo` advances it or clears the final stage; both
show a fresh review before confirmation. Busy Sessions require an explicit
interrupt choice and a server read acknowledging idle before preview. The panel
shows conflicts/current exclusions and external/unknown-effect limits. Replies
carry exact Session, directory and UI generation. A Session switch, new selection
or closed panel invalidates late composer changes. Native Stop and permission/
question prompts keep their existing priority.

Only acknowledged matching operations replace composer text, with an additional
check that the current draft has not changed while the request waited. Original
plain text drafts are retained per Session/Location in a bounded local in-memory
map; clearing restores the original when safe, otherwise the newer draft remains
and the original can be copied explicitly. Undo/redo refuses an unsent draft with
attachment chips to preserve those inputs. Historical prompt restoration copies
text, without manufacturing media attachment authority. After a restart without
a retained original draft, clear preserves the current composer. No uncertain
operation is replayed automatically.


### Non-Git file-copy implementation increment

The first production backend supports admitted Linux/macOS filesystems and keeps
Git selection and disabled snapshots compatible. Objects/manifests/journals use
`<data>/snapshot/global/<Location identity>/`, adding a Location namespace to the
canonical fallback layout because non-Git projects share the `global` project ID.
The identity hashes canonical path plus native device/inode/creation time, avoiding
mtime/size authority and inode-reuse adoption. Bounded descriptor-relative walks
never follow directory symlinks; configured ignores and private roots take priority.
Mode changes are explicit in review. Unsupported native metadata, directory
identity or cross-filesystem replacement refuses visibly; Windows fallback remains
unavailable rather than using unchecked writes.

Versioned manifests retain excluded classes. Current/target/recorded exclusions
enter the reviewed fingerprint; excluded paths are unknown, not deletions. A bounded
private store accounts for logical payloads, records and displaced preimages.
Native restore prepares stages and durably records its intent before namespace
mutation; admission, contents and directory routes are refreshed per file.
Unknown/interrupted intents block reuse and keep actual displaced preimages.
`GET /sessions/{id}/rewind/recovery` is read-only owner inspection, uses durable
Session Location metadata without waiting on a held live projection, and never
settles native evidence. Its `filesystem_complete` flag describes the filesystem
record only. Live inspection takes no mutation lease and reads atomically published
records; missing progress never implies no effect.

GC uses the application's bounded complete durable `copy:` reference set. It
preserves all referenced manifests, including inactive branches and stages, and
refuses pruning without that view. Any namespace with journals is retained until
a future explicit managed-resolution protocol can prove acknowledgement; this
increment does not invent a keep/rollback policy or clear native fences. That
resolution and journal reclamation, Git restore journaling, full native acceptance
and complete historical Git exclusions remain open under tasks 2.4/3.2–3.4.
Snapshot warnings are optional capture diagnostics, not failed conversation results.


### Managed recovery ownership protocol (review pending)

[Managed recovery](managed-recovery.md) makes keep/rollback, exact worker/native
acknowledgement, lost-reply reconciliation and two-phase cleanup concrete. Its
filesystem-only classification and legacy-journal migration boundary require review
before changing recovery admission. Historical Git exclusion implementation and the
already approved rules/context, structured-output and JSONL work are independent.
