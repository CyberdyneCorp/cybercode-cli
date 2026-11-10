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
