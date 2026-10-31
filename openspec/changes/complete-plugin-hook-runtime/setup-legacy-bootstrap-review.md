# Legacy Setup bootstrap: proposed owner resolution

Status: **proposal requiring S1 review, not approved or implemented**. This
supplies a concrete compatibility choice for the deletion gap in
[setup-design.md](setup-design.md#session-deletion-and-legacy-history-review).
It does not authorize schema, marker or init implementation. The full original
Setup contract remains required until a complete revised decision is accepted.

## Proposed decision

Retain the proposed Store-lifetime installation epoch and exact stored directory
key. Do not infer that Setup never ran from missing Session rows or receipts.
For an unproven legacy Location, require a fresh owner decision before dispatching
another potentially mutating automatic attempt. Permit an explicit new attempt
or an explicit decision to consume automatic eligibility without dispatch.

This is a compatibility revision for ambiguous legacy history. It is not the
claim that universal refusal satisfies automatic Setup. New Stores retain normal
automatic first-root behavior, and all other Setup requirements remain required.
The changed behavior for legacy root creation needs explicit review together
with the epoch/key choices; accepting those two choices alone is insufficient.

## Classification owned by the Store

| Authoritative state | Proposed result |
| --- | --- |
| Store proven newly initialized with its epoch, with no legacy history to migrate | Normal atomic first-root Session/automatic-attempt admission. |
| Existing marker or admitted attempt | Retain its identity and actual status; normal creation cannot dispatch it again. |
| Retained root-creation evidence at the exact stored directory | Consume legacy automatic eligibility without another effect; record the evidence, not fabricated successful handlers. |
| Valid retained, non-synthetic Setup receipt facts at that key | Retain positive use and known per-handler outcomes; consume automatic eligibility without replay. An individual receipt does not prove a whole attempt completed. |
| Existing legacy Store with no complete applicable proof, including deleted roots, child-only surviving history or an exhausted bounded search | Classify history as unproven. Missing facts cannot make the Location fresh. Apply the owner-resolution flow below. |

Freshness needs Store initialization provenance captured under its ownership
lock before a legacy migration can erase that distinction. An empty Session/event
table, absence of receipt projections, filesystem timestamps, reinstall, process
restart or an alias spelling is not a freshness witness. Restoring/copying an
existing Store retains its epoch and provenance. Normal Session deletion cannot
create a new installation. The exact initialization/migration implementation is
an S2 prerequisite and must preserve the existing atomic schema initialization.

Read positive legacy evidence in bounded pages and with a finite search budget.
Budget exhaustion can yield only unproven history, never a proof of absence.
Do not inspect deleted prompt contents or use provider usage as a history oracle.
Bind retained evidence to its actual schema, source ID and directory key; do not
silently canonicalize aliases or import synthetic test receipts as authority.

## Proposed owner flow

Ordinary root creation for an unproven legacy Location returns a structured
`SetupHistoryUnprovenError` before creating the Session, admitting input, running
a model Turn or dispatching Setup. It reports the epoch, validated directory key
and a bounded resolution revision. It must not silently create the root while
pretending automatic Setup completed or while hiding that Setup was suppressed.
Child creation still cannot consume automatic eligibility or claim root authority.

The owner has three explicit choices:

1. **Run a new Setup attempt.** Use the proposed `cyber --init` owner operation,
   with a new request ID. This authorizes a new attempt, not replay or settlement
   of an old unknown invocation. Preserve all earlier uncertainty. Display that
   prior execution cannot be ruled out; do not claim the Location was never used.
2. **Continue without Setup.** Proposed CLI spelling:
   `cyber --init --skip-legacy-setup --cwd <directory>`. This resolves only an
   unproven legacy claim, creates the requested root and consumes automatic
   eligibility with an explicit skipped outcome. It dispatches no Setup handler
   and emits no successful Setup observation. It is not a general bypass for
   fresh-Store automatic Setup. A later deliberate init needs a new request ID.
3. **Cancel.** Create no Session, attempt or consumed marker. A later request
   still needs resolution if history remains unproven.

Extend the already proposed `POST /api/v1/setup` request with an optional
`legacy_resolution` object containing the observed resolution revision and action
`new_attempt` or `skip_automatic`. Its creation-input validation and 128-byte
request-ID bound remain unchanged. The CLI performs the owner preflight needed
to obtain the revision; a stale resolution is refused without effects. Explicit
init for an otherwise ordinary Location retains its existing new-attempt meaning.

An interactive client may present these choices after the structured refusal.
An unattended ordinary creation cannot choose on the user's behalf. Explicit
init/skip requests still obey existing non-interactive, trust, Mode, pure/managed
policy and permission rules; they do not manufacture permission approvals.
Use ordinary owner API authority. Prompt text, hook rewrites, plugin callbacks and
`session/prompt` cannot supply a migration decision or assert that history is fresh.

## Atomicity, ownership and retries

Resolve the installation/directory claim, create the root, admit the new attempt
or explicit skip, and bind request deduplication in the bounded multi-aggregate
writer transaction required by S2. Recheck the epoch, exact key, resolution
revision, existing marker and current owned context in that transaction.
Competing legacy resolutions cannot both claim the same unresolved revision.
A stale loser must not silently turn into another fresh attempt or retry with
a new request ID. Normal concurrent creators resume the ordinary creation rules
only after the legacy claim has been resolved.

Dispatch after acknowledged commit and outside ownership locks. Existing live
attempts, native owners and uncertain cleanup still need their normal settlement
or reconciliation; a migration choice cannot bypass them. A fresh explicit
intent never rewrites an older unknown outcome to completed. A lost commit
acknowledgement follows the Store uncertainty fence and cannot trigger retry.

Keep marker, resolution, attempt and request records independent of Session
deletion. An exact retry returns the original result and deletion status; it
cannot recreate a deleted Session or dispatch again. Changed-input reuse is
refused. Reprojection uses self-contained owner facts and preserves consumed,
skipped and unknown states without requiring a surviving Session projection.
The request digest includes the epoch, exact key, resolution action/revision and
validated root-creation inputs. Check exact deduplication before rejecting a
consumed revision, so an operation's own successful resolution cannot make its
transport retry look stale. A different action or revision under the same request
ID is changed-input reuse, not another chance to dispatch.

## Review and implementation consequences

S1 must accept or replace this compatibility policy, the Store-lifetime epoch
and the exact directory key together. No S1 checkbox is closed by this proposal.
If accepted, revise the unconditional concurrent-root-creation description and
the Setup delta scenarios to make the unproven-legacy exception explicit before
S2 code. No policy is selected merely by recording or mechanically merging this
review document. If declined, retain the original full requirement and return a
different complete migration decision rather than silently choosing a fallback.

Required acceptance cases include all classification rows; deleted roots with
another parent's surviving child; partial and synthetic receipts; budget
exhaustion; concurrent init/skip/root creation; stale revisions and changed inputs;
policy refusal; empty versus unresolved catalogs; lost commit/handler outcomes;
process restart and projection rebuild; deletion before an exact retry; and no
model Turn, extra permission, duplicate effect or false successful observation.
