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
