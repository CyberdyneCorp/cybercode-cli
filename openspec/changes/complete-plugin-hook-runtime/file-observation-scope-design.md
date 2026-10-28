# File observation Session scope integration

Status: implementation increment within approved producer task 5.2. Durable scope
records and exact-directory dispatch are being connected first. Native pinned
alias admission, renewal and per-handler checks remain unimplemented; this
increment does not claim working alias delivery or change Session path semantics.

## Problem and required behavior

The observer registry shares a canonical root, but the journal and current
receipt/renewal checks use the first Session's raw directory string. A second
Session spelling the same physical root differently cannot receive the shared
observation. Restarting with a different first spelling can also select a
separate journal and feedback allowance. Making path comparisons permissive
would lose durable provenance; querying the filesystem from projectors would
make replay depend on later filesystem state.

Keep one canonical Location journal and finite feedback allowance. Preserve raw
Session directories and their existing hook identities. Admit each Session only
when its current path names the live owner's pinned physical root. Retargeted
aliases, archived/moved Sessions and stale generations lose eligibility.

## Durable provenance

`file.watch.scope.1` records Session ID, current raw Session directory, observation
owner directory, physical-root identity and generation. Its aggregate is keyed
by Session ID, with only the latest record needed for current-scope lookup. A
return to an earlier Location appends a new current record instead of reviving
an old alias binding. Expected-sequence append serializes competing bindings;
idempotent admission does not add another record.

Scope records are separate from metadata checkpoints/deltas. They do not spend
or renew feedback credit, advance the metadata CAS, supply file contents or
create replayable hook work. The single writer validates the current live
Session directory, source watch generation/root and aggregate identity. It does
not perform filesystem IO. Scope metadata alone grants neither alias admission
nor execution authority; a native owner supplies physical proof outside replay.

## Remaining native integration

1. Add a pinned-watch claim accepting an owned clone of the already opened root
   directory handle. Derive physical identity with one shared pure hash helper;
   verify that the current canonical root path still names that handle. Keep
   the existing exact-directory port for callers without native proof.
2. Retain the pinned root in FileWatch. Register only weak generation leases in
   the runtime, bounded by the active canonical Location limit. Superseded or
   dropped leases cannot keep native resources alive or authorize an alias.
3. Before emitting an alias scope record, compare both the canonical owner path
   and current raw Session path with the pinned handle. Perform IO outside
   registry, Session and journal-writer locks. Recheck the Session row under the
   single writer when appending its scope record.
4. Key new production metadata generations by canonical Location. The current
   Session scope record binds raw hook identity to that generation without
   changing stored Session paths, weakening ordinary hook identity checks or
   accepting arbitrary path-equivalence claims.
5. Before each new handler admission, require the current generation's live
   native lease for alias delivery and recheck physical root membership. The
   writer still checks the real eligible observation, current Session scope,
   path and stable receipt identity atomically. No live lease means no alias
   authority, even if a scope metadata record exists.
6. Forward bounded membership refreshes to scope preparation. Invalidate a
   Session's project-envelope cache when its latest admitted physical scope
   changes, using the existing epoch to prevent stale in-flight refills. Do not
   flush the cache on every event or silently follow a retargeted alias.
7. Before recording user renewal, revalidate native scope and prepare the current
   scope record. The writer must still prove that the single-use committed user
   acknowledgement belongs to both the current raw Session Location and its
   historical Location at admission. A scope record cannot manufacture user
   presence, change its source or move an old acknowledgement between Locations.

## Required acceptance

- Canonical and differently spelled/symlinked Sessions receive their own scoped
  events from one watcher generation, including when an alias joins first.
- Restarting through another spelling retains metadata and exhausted credit;
  it cannot create another allowance without real user input.
- Alias retargeting, Session movement/archive, root replacement and stale leases
  reject new old-root admission. Project identity changes with the admitted
  physical root, and another Location's handler cannot borrow its observations.
- Scope records cannot bypass native proof, counterfeit a user acknowledgement,
  alter metadata/credit or evade source/generation/Session checks.
- Scope preparation, physical checks and cache invalidation do not hold owner
  locks across filesystem work, callback execution or task joins. Launched
  effects keep existing cancellation/Unknown/no-replay behavior.
- Run actual Store/runtime and real application tests on Linux, macOS and
  Windows, with explicit supported alias mechanisms and skips if unavailable.
