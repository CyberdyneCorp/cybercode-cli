# Setup ownership and explicit initialization

Status: **proposed for review, not implemented**. This expands the approved
Setup portion of task 5.1. The full plugins/hooks, daily-coding and binary-editor
goal remains unchanged. Native plugin support decisions remain separate.

## Existing implementation and gap

At `79b4171f`, `Runtime::create_session_admitted` checks
`location_has_sessions(directory)` before creating a root Session. That query
matches any Session at the exact stored directory, including children, and the
read and creation are separate operations. `Inner::setup` then dispatches after
creation and admission settle. It deliberately does not replay after a crash.
There is no explicit init entry point or durable installation/attempt marker.
Concurrent first creation can observe the same empty history. Existing history
alone does not identify individual attempted, failed or unknown Setup effects.

## Decisions requiring review

1. **Installation scope:** create one durable installation epoch per owner Store.
   Retain it across process restarts and binary upgrades, including a reinstall
   that retains the Store. A new Store starts a new epoch. Upgrades do not
   automatically re-run potentially mutating setup; explicit `cyber --init`
   requests another attempt. This gives "after install" a concrete Store scope.
2. **Location key:** retain the exact stored Session directory key used today.
   Different alias spellings can have separate Setup markers. Do not silently
   change Setup to physical-root deduplication while migrating old attempts.
   FileChanged's canonical watcher ownership is a separate contract.

These choices need approval before marker/schema/init implementation. A choice
to use version epochs or canonical deduplication requires a revised migration
and concurrency design, not a hidden implementation substitution.

## Durable admission

Create the installation epoch once through the existing single writer. Automatic
markers are keyed by `(installation_epoch, stored_directory)`. Projectors perform
no filesystem I/O. Retained path strings obey existing Session validation.

Extend the checked-append facility for Session creation: decide automatic
eligibility under its writer transaction, then append `session.created.1` and an
owned Setup attempt event atomically across their separate aggregates. The
current `Store::append_checked` accepts only one aggregate; it cannot already
perform this operation. The Setup event and
marker belong to the installation/Location owner, not the Session aggregate that
normal Session deletion purges. The marker projection has a unique
installation/directory key and no cascading Session ownership. Concurrent creators
still create their Sessions successfully; only the winner records a dispatchable
automatic attempt.
The Store extension must bound the aggregate/event groups, retain registered
event validation, expected-sequence checks and fault injection, and run all
projectors in the same writer transaction. A late conflict or projector error
rolls back every group's events, sequence rows and projection effects. Publish
notifications only after the combined commit. Do not emulate this with two
separate acknowledged appends or call a public Store mutation from inside its
writer callback; `append_in_tx` is currently private to `cyber-store`.

A rolled-back creation records neither Session nor marker. A child Session
cannot claim the automatic marker. Child-only history does not consume it.

An attempt records its event ID, installation epoch, Session, stored directory,
trigger (`first_session` or `init`) and bounded request ID. The marker means the
attempt was admitted, not that every handler succeeded. Existing durable hook
receipts report completed, failed, skipped and unknown effects. Handler receipt
identity includes the source attempt and effective handler identity so duplicate
admission cannot launch the same effect again.

Record bounded attempt settlement separately from marker admission, including
outcome counts and whether catalog dispatch actually completed. No receipt rows
alone do not prove that zero handlers were selected: the process may have died
before resolving its catalog. Only an acknowledged settled-empty attempt can be
reported as an empty completed init. A bounded live-invocation registry identifies
currently running attempts; an admitted, unsettled attempt without that owner is
unknown. Transport retry inspects that state and never restarts dispatch.

For an existing Store without markers, surviving root creation events at the same
stored directory (including roots later moved or archived) are positive evidence
for consuming a legacy automatic marker without dispatching another effect.
Deleted roots need the separate legacy review below: their creation events are
not retained. Decide migrations in the writer transaction, not a filesystem scan
or startup replay. A historical attempt may already have mutated external state.
Do not infer successful execution from history, replay uncertain effects, or
blanket-mark legacy handlers completed. Explicit init remains a new user-owned
attempt rather than a replay of old execution.

Dispatch only after commit, outside Session/writer locks and shutdown admission
held by creation. Recheck the live Session/directory and existing hook policy
before handler admission. Session moves/archive, shutdown and ownership loss
cannot turn an old attempt into a newly authorized effect. A crash after marker
commit may skip an unlaunched effect; it must not automatically replay it.

## Session deletion and legacy-history review

Source inspection at `dd0c9279` establishes a limitation in the preceding draft.
`runtime/deletion.rs::Runtime::delete` removes the Session/descendant aggregates'
non-team events, their sequence rows and Session projections. Consequently a
query of `session.created.1` cannot recover every previously used Location after
normal deletion. Replacing the current eligibility query with only
`parent_id IS NULL` could turn child-only surviving history into a fresh Setup
even when an earlier root and its potentially mutating attempt were deleted.
The surviving child can belong to another root's subtree; deletion removes the
deleted root's own descendants.
No such query change has been implemented.

The independent `hook.started.1` aggregates can retain positive Setup receipt
facts, while the `hook_execution` projection cascades with Session deletion.
Retained, non-synthetic Setup receipt facts may therefore establish an earlier
attempt; they do not establish successful settlement or a complete set of prior
Locations. The usage ledger retains only provider/auxiliary-call facts, so a
Session that ran no model call supplies no complete setup-history substitute.
The computer-deletion tombstone records an ID/time rather than a Location.
Absence from any of these sources is not proof that Setup never ran.

Future installation markers, attempt admission/settlement and request deduplication
must survive Session deletion in their own durable aggregates/projections. Their
records retain bounded owner facts needed for this contract, not deleted prompt
history or handler IO. Checked live admission validates the real root Session
under the writer transaction; replay validates self-contained marker/attempt
facts without requiring a Session projection that may legitimately be gone.
Rebuilding projections must preserve an admitted or unknown attempt rather than
making it eligible again. Deletion cannot clear a consumed automatic marker or
allow an old init request ID to create another Session.

Legacy bootstrap remains an explicit review item before S2. A migration can
consume positive historical evidence, but cannot manufacture the missing
pre-marker history of deleted roots. The complete migration/support decision
must explain how unproven legacy Locations obtain fresh user authority without
replaying uncertain effects. An explicit init is one available user-owned action;
no policy that silently reruns Setup, silently resets installation identity or
universally refuses automatic Setup is selected by this document. Approval of
the two epoch/key choices alone does not settle this newly identified legacy
bootstrap requirement. The full original Setup contract remains required until
a revised compatibility decision is reviewed.

The [owner-resolution proposal](setup-legacy-bootstrap-review.md) supplies a
concrete candidate for that review, including a deliberate legacy compatibility
exception and its CLI/API, concurrency and retry consequences. It is not accepted
by recording the proposal; S1 and the original full contract remain open.

Required adversarial cases include root/attempt deletion before restart, a
previously deleted root with another parent's child surviving at that Location,
an empty settled attempt,
unknown execution, projection rebuild after Session deletion, and an exact init
retry whose original Session was deleted. Each must retain the original attempt
identity and outcome/uncertainty; none may manufacture another mutating dispatch.

## Explicit init surface

`cyber --init --cwd <directory>` creates a root Session and requests a Setup
attempt without opening the TUI, admitting a model prompt or running a Turn.
Coalesce explicit init with any otherwise automatic first-Session attempt so a
single invocation cannot dispatch both. Normal profile/model/Mode/pure/trust
configuration still applies. Refuse ambiguous combinations with another
subcommand or a supplied model prompt before creating a Session.

Add `POST /api/v1/setup` with a nonempty request ID of at most 128 bytes and
bounded root-Session creation inputs for daemon/attached CLI use. This owner
operation creates the Session and admits explicit init together; the CLI must
not first call ordinary Session creation and accidentally run automatic Setup
before requesting init. Parent/child-only creation fields are refused.

The operation uses ordinary owner API authority; it is not a tool or a
plugin/hook callback operation. Prompt text, a hook rewrite and `session/prompt`
cannot manufacture it. Persist the request ID and validated creation-input digest
under a unique installation/request key. Exact transport retries return the
original Session/attempt/outcomes without creating another Session or
dispatching again. A deliberate later init uses a new request ID. Reusing one
with different Location or creation inputs is refused.

Expose the created Session/attempt IDs, deduplication and existing per-handler receipt
outcomes in JSON; text output describes the recorded attempt. Failed/unknown
handler effects must not be reported as all-success. Explicit init exits nonzero
for failed/unknown execution; observational deny cannot undo Session creation.
Policy/trust skips are reported distinctly. No eligible handlers is a valid
empty attempt. Automatic Setup failure never fabricates rollback of a committed
Session. Existing trust, pure/managed policy, cancellation and protection rules
remain authoritative; init grants no additional permissions.

## Implementation and acceptance tasks

- [ ] S1 Review both epoch/key decisions and the legacy-deletion bootstrap requirement above, including the [owner-resolution candidate](setup-legacy-bootstrap-review.md); record the accepted complete migration/support decision before implementation.
- [ ] S2 Add bounded events, marker projection/migration and writer-atomic root
  Session admission across separate durable owners; cover concurrent creation,
  children, legacy/deleted history and rebuild after Session deletion with actual
  Store transactions. Extend checked append across the bounded owner aggregates;
  test late CAS/validation/projector failures, complete rollback, ordering and
  absence of uncommitted notifications through the actual writer.
- [ ] S3 Bind Setup receipts to the admitted attempt; test duplicate, moved,
  archived and forged/synthetic admission, empty-settlement versus lost-owner
  ambiguity, failed/unknown outcomes, cancellation
  and crash points before/after marker and handler acknowledgement.
- [ ] S4 Add the owner operation and top-level init CLI/API/SDK/schema artifacts;
  test transport retries, request-binding refusal, no model Turn/TUI and
  explicit/automatic coalescing and retry after Session deletion through the real
  App and CLI.
- [ ] S5 Verify unchanged hook trust/pure/managed rules and outcome reporting on
  Linux/macOS/Windows, including native command hooks and HTTP handlers where
  configured. Record unavailable checks explicitly.
- [ ] S6 Update public docs/roadmap and parent task evidence; run relevant checks
  and substantive code/security review before presenting implementation PRs.

No task is completed by this design document or structural validation alone.
