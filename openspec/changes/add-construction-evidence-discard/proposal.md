## Why

Status: **awaiting owner review** (owner decision 2026-11-15: OpenSpec first, no code).

`complete-daily-coding-surfaces` deliberately gives half-built checkout construction
evidence no cleanup authority. A process killed after a checkout-claim or
acknowledgement stage is flushed and before its no-replace publication leaves a
`preparing_*` or `ack_preparing_*` stage that is listed (preparation listing, `sessions
recovery` text, TUI **C**/**P**) and that fences reuse, metadata retirement and managed
worktree removal as `same-owner unknown constructor` evidence. Nothing can remove it,
so such a managed worktree can never be removed, and evidence tied to a Session whose
Location was removed (now listed read-only as orphaned) stays forever. The owner needs
an explicit, audited way to discard exactly that evidence without giving any automatic
path, model tool or remote client cleanup authority.

## What Changes

- `snapshots-checkpoints` gains "Owner-only discard of construction evidence": a
  two-step review/confirm command that discards one exact, unpublished construction
  stage (claim, acknowledgement or cancellation-record stage) of one Session after
  fencing its owner activity, and records a durable audit receipt.
- The same command covers the construction stages and stale Location metadata left for
  a Session whose Location directory was removed, but never deletes restore journals,
  payload images or published claims/acknowledgements.
- Surfaces: `POST /api/v1/sessions/{id}/rewind/constructions/discard/review`,
  `.../discard/confirm`, `GET .../discard/requests/{request_id}`; SDK methods;
  `cyber sessions recovery-discard`; the TUI recovery panel (**C**/**P**) under an
  explicit key with a typed confirmation.
- No change to automatic maintenance, reclamation or worktree removal: they keep
  refusing while any construction evidence remains.

## Impact

`crates/cyber-core` (worktree activity stages), `crates/cyber-snapshot` (cancellation
record stages), `crates/cyber-server` (runtime command, events, HTTP/OpenAPI),
`sdk/`, `crates/cyber-cli`, `crates/cyber-tui`, `docs/rewind.md`. A new durable event
`session.snapshot.construction_discarded.1`. Security-relevant: it is the first path
allowed to delete fenced recovery evidence, so it needs owner approval before any code.
