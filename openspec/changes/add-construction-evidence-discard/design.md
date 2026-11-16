## Context

Status: **approved by the owner 2026-11-15** (open questions answered below). No implementation exists yet; nothing here is enabled.

Construction evidence is any private stage a constructor flushed but did not publish:
checkout claim stages (`preparing_<len>_<activity>_…`), acknowledgement stages
(`ack_preparing_<len>_<activity>_…`) and file-copy cancellation-record stages (`tmp_*`
beside `preparation.json`). The approved managed-recovery protocol keeps each stage
byte-identical, never adopts it, and makes reuse, metadata retirement and worktree
removal refuse with `same-owner unknown constructor`. Recovery clients list stages by a
read-only name inventory. Since the 2026-11-15 decision, a Session whose Location was
removed lists its retained journals read-only as orphaned.

## Goals / Non-goals

Goals: an explicit owner-only command that discards exactly one reviewed stage (or the
reviewed set of one activity), proves nothing can still publish it, leaves an audit
receipt, and is idempotent under retries and lost replies.

Non-goals: automatic cleanup; discarding restore journals, displaced payload images,
published claims, acknowledgements or cancellation records; recovering (adopting) a
stage; any authority for model tools, subagents, peers, relays or remote browsers.

## Decisions

### Who may run it

Only the authenticated local owner: the owner credential of the local server, from
the CLI, the SDK with that credential or the TUI. Requests carrying delegated
input-client leases, interactive browser leases, peer/relay identities or any
model-tool origin are refused (`403`). It is never offered as a model tool.

### Review, then confirm

1. Review (`.../discard/review`, read-only): the owner names the Session and either one
   stage name or an activity ID. The server resolves the stage(s) without opening them
   for writing and returns, per stage: name, kind (claim, acknowledgement, cancellation
   record), owner activity and Session, native identity (device, inode), size,
   modification time, a SHA-256 of the bytes (read through a no-follow handle, bounded
   to 1 MiB per stage; larger stages are refused), the fences that currently cite it,
   and the Location state (`present` or `removed`). It returns a confirmation
   fingerprint over all of that plus the Session revision.
2. Confirm (`.../discard/confirm`) takes the fingerprint and an owner-chosen
   `request_id`. A changed fingerprint is `409 stale` and nothing is touched.

### Fencing at confirm

Under the Session lock and the workspace mutation lock, in this order:

- The Session is idle, has no running Drain, no admitted snapshot worker, and no
  unresolved preparation other than a cancelled one for that activity.
- The owner activity's lifetime lock is acquired exclusively and non-blocking. A live
  holder (any process still able to publish) refuses with `409 busy`.
- No stable claim, `.snapshot-ack` or `preparation.json` exists for the activity that
  was published from, or could be published from, this stage; a published record means
  the stage is not orphaned and ordinary recovery applies.
- Each stage still has the reviewed native identity, size, mtime and digest, is a
  regular file (never a link, FIFO, socket or directory) inside the expected private
  directory, and its name parses to the reviewed owner activity.
- For a removed Location: the Location path is `NotFound`, the managed registration
  for that worktree is gone or marked removed, and the stage's recorded owner Session
  is the reviewed Session.

### Effect and audit receipt

The command first commits a durable `session.snapshot.construction_discarded.1` intent
(request ID, Session, activity, per-stage name/kind/identity/digest/size, Location
state, actor = local owner, time) and then, per stage, renames it with no-replace into
a private `discarded/<request_id>/` directory in the same namespace, flushes, and
unlinks it. The receipt is completed with the unlinked identities. A crash between
steps resumes the same frozen plan on the next confirm or lookup with the same
`request_id`; a stage whose identity no longer matches is reported, never deleted.
Lookup (`GET .../discard/requests/{request_id}`) returns the receipt without acting. The
receipt is never removed by retention and appears in the Session's event history and in
`cyber doctor`.

### What still refuses

- Restore journals, payload images and every published record: not construction
  evidence; use keep/rollback recovery and reclamation.
- Malformed stage names with no derivable owner: listed, refused in this revision.
- Acknowledgement stages whose owner claim is still unacknowledged by exact receipt
  reconciliation, until that reconciliation has run once.
- Any stage while its activity's lifetime lock is held or the Session is busy.
- Remote, delegated or model-originated callers.
- Worktree removal and metadata retirement keep refusing until the evidence is gone;
  discard does not remove the worktree itself.

## Risks / Trade-offs

Deleting fenced evidence can hide a crash that was still recoverable; the exclusive
lifetime lock, published-record check and digest-bound review are what make that
deletion safe. Bounding stage size keeps review cheap but leaves very large stages for
a later decision.

## Owner decisions (2026-11-15)

- **Scope:** one stage per confirmation. Each stage is reviewed, confirmed and
  receipted on its own; there is no per-activity bulk discard in this revision.
- **Surfaces:** CLI (`cyber sessions recovery discard`) and the authenticated
  HTTP/SDK route only. The TUI recovery panel lists the evidence and shows the exact
  CLI command; a TUI action is deferred to a later revision.
- **Failed unlink:** periodic maintenance retries unlinking `discarded/<request_id>/`
  indefinitely. It is never dropped silently: `doctor` and recovery listings report it
  as `discard pending`, and its bytes count toward the snapshot cap until removed.
