# Additional directory admission and DirectoryAdded

Status: proposal for review; no directory admission or producer implementation.
This expands task 5.1's prerequisite. It does not complete the native plugin,
Setup, daily-coding or binary-editor work, or approve their separate decisions.

## Existing contract and inspected gap

The canonical `permissions-modes` requirement "Additional working directories"
requires repeatable `--add-dir`, `/add-dir` and
`POST /api/v1/sessions/:id/directories`. An addition must cover the Session's
ordinary external-directory checks, sandbox writable roots and instruction
Context Sources, persist `session.directory.added.1`, emit DirectoryAdded and
apply at the next Safe Boundary. Protected paths remain protected.

At source `43158f31`, these components do not share such an admission:

- `CreateSession`, `SessionInfo`, `TurnContext` and `Invocation` have a Location
  and rules, but no committed additional-root snapshot or revision.
- `Ctx::check_external_paths` in `cyber-tools/src/host.rs` compares paths with the
  Location. File browsing has a separate check in `host/files.rs`. Updating one
  path would leave attachments, browsing or ordinary tools inconsistent.
- `sandboxing.rs` composes Location/configuration roots separately; adding a
  permission rule alone cannot authorize the OS sandbox. Its isolated-solve
  restriction deliberately clears additional configuration write roots.
- `ContextInputs` observes the Location's instructions. Directory instructions
  need their own provenance-bearing Context Source integration.
- `runtime/drain.rs::boundary` owns Safe Boundary ordering. The existing
  observational lifecycle hook captures Session scope under its lock and runs
  handlers after releasing the lock.

The CLI attachment refusal already suggests `--add-dir`, but that suggestion is
not evidence that the flag, owner admission or effective directory exists. A
standalone hook emitter would therefore report a transition that did not happen.

## Proposed owner and durable model

Use one Session-owned directory service for all three public entry points. It
accepts user/owner intent, validates a bounded request ID and path, and refuses
archived Sessions and requests outside the caller's Session authority. A prompt,
model tool or hook decision cannot manufacture this owner operation. CLI creation
inputs must use that same service rather than changing global configuration.

Proposed bounds are 64 effective additional roots and 64 pending additions per
Session, 4,096 UTF-8 bytes per path and 128 bytes per request ID. These are choices
for review before schema implementation, not newly active limits.

Record pending intent separately from an effective addition. Exact request
retries inspect the original operation; reusing an ID with different input is
refused. A request for an already effective root is an idempotent no-op rather
than another grant or hook. Preserve the submitted spelling for display, while
the effective grant binds the resolved root and its native identity. This is
separate from Setup's proposed exact-stored-directory marker key.

Projectors remain deterministic and perform no filesystem IO. Prepare and pin
the directory outside writer/Session locks, then validate Session/configuration
revision and ownership at admission. Retargeting or replacing a root cannot
silently transfer an old grant to another physical directory. After restart,
rebuild native proof before releasing a new tool dispatch; a durable path string
alone is not that proof. Record unavailable scope instead of treating uncertainty
as an acknowledged effective addition.

## Safe Boundary publication

Prepare one immutable Session scope revision covering ordinary file decisions,
native sandbox roots and instruction-source inputs. Current Turns and tool
batches retain their captured revision. A pending addition cannot widen an
already running call or change another Session's scope.

At the next Safe Boundary, revalidate the prepared root and current Session,
commit the effective `session.directory.added.1` transition, and synchronously
publish the prepared revision before permitting another tool dispatch. Publication
must have no fallible preparation or asynchronous gap after the commit. If this
cannot fit the current owner API, revise the transaction design before coding;
do not publish independent mutable permission/context/sandbox lists.

Capture DirectoryAdded from the committed event identity and resulting Session
scope. Dispatch after releasing Session/writer locks, using existing durable
handler receipts and observational outcome rules. A failed or merely pending
request emits no successful addition. Duplicate delivery and unknown mutating
handler outcomes do not launch the same effect again after restart.

For an idle Session, an owner boundary may settle the addition without starting
a model Turn. If instruction preparation or native root validation fails, report
the pending/refused result explicitly; an API acknowledgement must distinguish
queued intent from an effective addition.

## Permission and context invariants

- The grant satisfies the ordinary external-directory scope check only. Read,
  edit, Mode, managed denial, trust, taint, hidden-path and protected-owner checks
  retain their authority. Plan Mode is not made writable by adding a root.
- Browsing, attachments, in-process editors and sandboxed shell/background tools
  consume the same captured effective revision. Every target resolves against
  actual authorized roots; a child symlink cannot borrow its parent's grant to
  escape into an unrelated tree.
- Preserve the isolated-solve boundary and parent/child permission ceilings.
  Specify and review any child/fork propagation explicitly; copying a path list
  or another Session's event is not admission authority.
- Load additional directory instructions through bounded, permission-checked
  Context Sources, with actual file provenance and existing InstructionsLoaded
  semantics. DirectoryAdded does not replace instruction loading or move the
  Session's Location/cwd/project identity.
- This operation does not widen plugin manifest capabilities, publish arbitrary
  native observer roots or mutate Location/global configuration.

## Implementation and acceptance tasks

- [ ] D1 Review this prerequisite, bounds, identity/deduplication, idle-boundary
  and child/fork semantics; add the permissions-modes/CLI/API deltas before code.
- [ ] D2 Add bounded pending/effective events and projections, request binding
  and writer-atomic revision checks. Test duplicate/conflicting/concurrent
  requests, archived Sessions, replay and transaction rollback with a real Store.
- [ ] D3 Integrate pinned native root preparation and one Safe Boundary snapshot
  into all ordinary file, attachment, sandbox and context paths. Test grants
  during held tool calls, root replacement/retarget, protected aliases, restart,
  failed instruction loading and preservation of Mode/managed/child ceilings.
- [ ] D4 Add CLI, slash/TUI and owner HTTP inputs plus generated schema/SDK
  artifacts. Exercise repeatable flags and actual owner requests, with no model
  Turn or manufactured user presence for idle publication.
- [ ] D5 Emit committed DirectoryAdded with stable source identity; verify scope,
  order, idempotency and unknown-handler no-replay through actual HTTP/command
  handlers. A control omitting effective publication must fail a named behavior
  assertion rather than succeeding because a hook was emitted.
- [ ] D6 Run native Linux/macOS/Windows checks and substantive security review;
  reconcile public docs and task 5.1 only after complete acceptance.

The approved goal is unchanged. This proposal exposes a required prerequisite;
it neither substitutes a test-only hook for the feature nor closes a parent task.
