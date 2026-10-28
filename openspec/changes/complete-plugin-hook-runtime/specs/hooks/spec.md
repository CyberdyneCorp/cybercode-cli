## ADDED Requirements

### Requirement: Committed missing event producers
(P1) `DirectoryAdded` and `CwdChanged` SHALL originate from acknowledged effective lifecycle transitions with the existing scoped envelope and stable event identity. Existing observational `Setup` SHALL retain first-root-Session behavior with explicit init and reviewed durable marker semantics. `WorktreeCreate` SHALL retain its pre-effect, permission-following veto contract on supported creation paths, including isolated children. `FileChanged` SHALL observe successful tool/formatter writes and bounded shell/background/external file changes, reconcile overflow and exclude protected roots. Observations SHALL NOT claim permission or writer identity. Hook-originated writes SHALL NOT form unbounded self-trigger loops. Durable dispatch records SHALL distinguish completed, failed and unknown handler effects; restart SHALL NOT blindly replay an uncertain mutating handler.

#### Scenario: Child creation veto
- **WHEN** an authorized isolated-child creation reaches a blocking `WorktreeCreate` handler
- **THEN** its existing pre-effect veto runs before Git mutation and no checkout or readiness success is fabricated

#### Scenario: External edit and hook feedback
- **WHEN** an external editor changes a scoped file and its hook writes a follow-up file
- **THEN** bounded observations identify the changed paths without unbounded recursive hook execution, and overflow is reported for reconciliation

#### Scenario: Crash after possible handler mutation
- **WHEN** a command hook may have changed external state before its outcome was recorded
- **THEN** recovery retains its unknown status without replaying the command solely to obtain a receipt

#### Scenario: Public child input recreates through a real delegating Session envelope

- **WHEN** a user continues an isolated child whose checkout was removed and recreation reaches `WorktreeCreate`
- **THEN** the hook runs at the delegating Session's actual Location after the child's applicable worktree permission checks, without fabricating a child envelope at that Location
- **AND** a veto prevents checkout creation, rebinding and new input/model admission, while a retained checkout emits no new creation event
- **AND** no hook dispatch holds the child lifecycle mutex or runtime shutdown admission, and current deny rules are checked again before Git effects

#### Scenario: Committed Location observation
- **WHEN** a Session successfully enters or exits a worktree, returns while idle, or is rebound to a different effective path
- **THEN** `CwdChanged` uses the resulting Session Location and includes the previous/destination directories and durable transition ID, sequence and kind, after ownership locks settle
- **AND** a refused or unknown transition, same-path rebind, no-op exit or reducer replay does not emit a successful path observation
- **AND** observational decisions or unavailable handlers do not veto the already committed transition or change its reported result
#### Scenario: Durable file observation ownership
- **WHEN** a new watcher generation claims a Location whose metadata and feedback allowance were already recorded
- **THEN** old workers cannot append new observations, the acknowledged metadata and allowance survive, and replay does not rerun possible handler effects
- **AND** a failed observation commit does not consume metadata changes or dispatch allowance
- **AND** renewing a feedback allowance requires a separate, single-use committed user acknowledgement owned by a live Session at that Location

#### Scenario: Durable Session scope provenance
- **WHEN** an observation owner records a live Session's current scope
- **THEN** a separate idempotent record binds its Session directory to the owner directory, physical-root identity and current generation without advancing metadata state or feedback credit
- **AND** the single writer rejects wrong aggregate, Session-directory, root and generation bindings
- **AND** scope metadata alone grants neither filesystem access nor alias admission; alias delivery additionally requires live pinned native proof

#### Scenario: Canonical observation owner with Session aliases
- **WHEN** live Sessions use different spellings or symlinks of one canonical Location, including when an alias joins first
- **THEN** one canonical journal and watcher serve their separately scoped callbacks while preserving their raw Session envelopes
- **AND** configuration, path matching and native command cwd use the checked canonical root, and new handler admission and launch require its retained native lease
- **AND** retargeting an alias, dropping its watch owner or superseding its generation cannot authorize new old-root effects through a stale descriptor or a legacy fallback

#### Scenario: User acknowledgement follows its admitted physical Location
- **WHEN** user input is admitted and the Session's alias is subsequently retargeted
- **THEN** native feedback renewal checks the physical Location captured with the real input as well as its current live scope and historical Session directory
- **AND** an unused old-root acknowledgement cannot renew another physical root solely because the raw Session path is unchanged
- **AND** a committed acknowledgement can be consumed only once across observation Locations

#### Scenario: Duplicate or stale file observation delivery
- **WHEN** an observation is delivered twice to a Session or its watcher generation was replaced before handler admission
- **THEN** the same handler is not run again after a consumed or unknown receipt, and a stale generation cannot admit a new handler
- **AND** handler admission checks the real source record, eligible path and current Session Location under the single writer, without exposing private provenance in public receipts


#### Scenario: Tool and formatter observation hints
- **WHEN** an authorized built-in file mutation and its formatter settlement finish
- **THEN** bounded, coalesced hints cause metadata reconciliation even without language services, without claiming writer identity or bypassing scope checks
- **AND** invalid paths or coalescing overflow request reconciliation without retaining an unbounded set of names

#### Scenario: Replacement root identity
- **WHEN** an observed Location path is replaced by a different repository root
- **THEN** its fresh watcher generation resolves current project identity rather than using the previous root's cached hook envelope
- **AND** an older in-flight identity resolution cannot repopulate the current cache


#### Scenario: Scoped hard-link observations
- **WHEN** all reported aliases of a file are accounted for among permitted entries of a bounded scan
- **THEN** its in-Location aliases remain observable
- **AND** a file with unaccounted, excluded or inconsistent aliases is omitted from new metadata observations, with partial coverage reported and prior stamps retained without a synthetic deletion

#### Scenario: Observer cancellation and restart
- **WHEN** shutdown cancels a launched file observer callback and the Session is subsequently reopened
- **THEN** the local callback owner settles before the observer is joined, its durable receipt remains available, and the old observation is not replayed solely to obtain another result
- **AND** the reopened Location can receive a fresh observation with its preserved feedback accounting

#### Scenario: Cancellation during metadata reconciliation
- **WHEN** owner cancellation arrives while a blocking metadata traversal is in progress
- **THEN** cancellation is forwarded to the traversal and the owner waits for the in-flight traversal step to settle instead of detaching the blocking worker
- **AND** a cancelled scan cannot supply a new journal baseline or delta

#### Scenario: Persistent native registration limit
- **WHEN** a directory-registration bound leaves native coverage incomplete across successive scans
- **THEN** partial coverage remains visible and bounded periodic reconciliation continues without manufacturing a fresh notification-loss signal on every actor tick
- **AND** new callback queue or backend loss still requests immediate reconciliation

#### Scenario: Wide metadata traversal
- **WHEN** a shallow tree has many sibling directories within the metadata entry bound
- **THEN** retained traversal descriptors follow the depth bound rather than the tree width, and reconciliation can inspect the tail without permanent descriptor exhaustion


#### Scenario: Directory moves during metadata traversal
- **WHEN** a traversed directory leaves its recorded parent or is replaced before scan finalization, including an entry-limited scan
- **THEN** new metadata from that directory subtree is omitted and coverage is incomplete, retaining prior stamps without synthetic deletion
- **AND** omitted paths cannot count as permitted hard-link aliases for files remaining in the Location

#### Scenario: Metadata exceeds the journal byte budget
- **WHEN** valid scoped entries fit traversal/count limits but their encoded inventory exceeds the journal budget
- **THEN** the owner admits bounded metadata with visible partial coverage, preserves omitted old stamps without synthetic deletion and keeps admitted existing-file updates live
- **AND** confirmed absence can reclaim capacity, shrinking updates precede growth, and every intermediate checkpoint and delta obeys its encoded byte limit

#### Scenario: Native burst during a held observation effect
- **WHEN** external writes exceed native hint budgets while a launched FileChanged callback is held
- **THEN** bounded reconciliation records the changed tail even after handler feedback allowance is exhausted, with coverage uncertainty and paused delivery visible
- **AND** callback delivery remains within the durable allowance and shutdown joins the observer

#### Scenario: Idle Session returns from an entered worktree
- **WHEN** idle management acknowledges a Session's return to its original Location
- **THEN** host-owned observation membership refreshes after the durable transition and outside the Session lock without requiring another model Turn
- **AND** future observations follow the returned Location while the previous worktree loses eligibility

#### Scenario: Archived observation membership resumes
- **WHEN** a Session sharing a Location is archived while another member remains active
- **THEN** the active member continues observing and the archived member receives no new file effects
- **AND** explicit user input after unarchiving restores future observation without replaying effects recorded during its absence

#### Scenario: Owner storage nested under a Location
- **WHEN** owner data, configuration, state, cache or temporary roots lie below an observed Location
- **THEN** owner-supplied private roots and their canonical aliases are excluded before traversing those subtrees, from both initial metadata and subsequent deltas
- **AND** ordinary project observations and committed user-input renewal continue through the real application host

#### Scenario: A live owner's private storage root moves inside its Location
- **WHEN** a previously pinned owner storage directory is renamed within an observed Location
- **THEN** its physical identity remains excluded before traversal despite the changed pathname, including replacement between pathname metadata and directory open
- **AND** private file identities remain excluded through retained aliases, while ordinary project observations continue
- **AND** private identity capture is bounded, retains live ownership and reports observation unavailable rather than evicting an owned identity or resuming after uncertain capture

#### Scenario: Observation preserves owner database locking
- **WHEN** the owner captures private database or sidecar identities, refreshes them, or checks a Location replaced by a regular file
- **THEN** observation SHALL NOT open and close an independent Unix regular-file descriptor that can release the owner's existing POSIX database locks
- **AND** retained private-file history SHALL remain bounded and exclude moved aliases without silently evicting ownership when its budget is exhausted

#### Scenario: Shared watcher membership ends during a callback
- **GIVEN** two Sessions share an observer and one HTTP handler is still awaiting a response
- **WHEN** either Session's observer membership ends
- **THEN** that membership's owned delivery SHALL be cancelled and settled without stopping the other member, and no later handler SHALL start from a captured obsolete membership
- **AND** readmission SHALL use a fresh membership and SHALL NOT revive delivery from the older batch

#### Scenario: Known private identity replaces a public name
- **WHEN** a pinned private file or directory replaces a previously recorded public name inside the Location
- **THEN** the owner retires the previous public preimages without dispatching a deletion callback or spending feedback allowance
- **AND** the scanner records private entry/subtree exclusions separately from ambiguous aliases and publication-budget omissions, so unrelated public observations remain eligible

### Requirement: Setup attempt ownership and explicit initialization
(P1) Setup SHALL admit durable installation/Location-bound attempts before effects, serialize automatic first-root-Session claims with creation, and expose explicit initialization through the owner surface without a model Turn. Marker and migration semantics SHALL follow the reviewed Setup ownership design. Completed, failed, skipped and unknown outcomes SHALL remain distinguishable; neither an automatic marker nor a retry SHALL manufacture successful execution or replay an uncertain effect.

#### Scenario: Concurrent first root Sessions
- **WHEN** two root Sessions are created concurrently at one uninitialized installation/Location key
- **THEN** both valid Sessions can commit but only one automatic Setup attempt is admitted atomically with its creation
- **AND** child-only history does not consume a root Session's automatic attempt

#### Scenario: Creation rolls back
- **WHEN** root Session creation fails before its transaction commits
- **THEN** no automatic Setup marker or dispatchable attempt survives that creation

#### Scenario: Legacy root history
- **WHEN** an upgraded Store has prior root-Session history at a Location but no Setup marker
- **THEN** automatic migration consumes that legacy Location without replaying Setup or claiming its historical handlers completed
- **AND** the owner can deliberately request explicit init

#### Scenario: Explicit init on a new Location
- **WHEN** the owner invokes cyber --init for a new Location
- **THEN** exactly one init attempt is admitted with a root Session and no model Turn or TUI starts
- **AND** an otherwise automatic first-Session attempt is coalesced with that request

#### Scenario: Init transport retry
- **WHEN** the owner retries the same init request ID with the same validated Location and root-Session creation inputs
- **THEN** the original Session, attempt and outcomes are returned without creating another Session or launching handlers again
- **AND** a changed binding is refused and a deliberate later init requires a new request ID

#### Scenario: Marker commit followed by crash
- **WHEN** a process stops after a Setup marker commits but before a handler acknowledgement
- **THEN** recovery preserves the admitted attempt and any recorded unknown outcome without automatically replaying the handler
- **AND** marker presence is not reported as proof of successful effects

#### Scenario: Init preserves hook authority
- **WHEN** explicit init resolves configured hooks
- **THEN** ordinary project trust, pure/managed policy, protected paths and cancellation remain authoritative, and the operation grants no new permissions
- **AND** a prompt, hook rewrite or plugin callback cannot synthesize the owner init operation

#### Scenario: Lost owner before catalog resolution
- **WHEN** an admitted init attempt has no terminal dispatch settlement and no live invocation owner
- **THEN** missing handler receipts are not treated as an empty successful attempt and its outcome remains unknown
- **AND** an exact request retry does not restart dispatch
