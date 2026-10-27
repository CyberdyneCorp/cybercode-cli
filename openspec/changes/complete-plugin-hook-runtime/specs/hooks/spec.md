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

#### Scenario: Durable file observation ownership
- **WHEN** a new watcher generation claims a Location whose metadata and feedback allowance were already recorded
- **THEN** old workers cannot append new observations, the acknowledged metadata and allowance survive, and replay does not rerun possible handler effects
- **AND** a failed observation commit does not consume metadata changes or dispatch allowance
- **AND** renewing a feedback allowance requires a separate, single-use committed user acknowledgement owned by a live Session at that Location

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


#### Scenario: Owner storage nested under a Location
- **WHEN** owner data, configuration, state, cache or temporary roots lie below an observed Location
- **THEN** owner-supplied private roots and their canonical aliases are excluded before traversing those subtrees, from both initial metadata and subsequent deltas
- **AND** ordinary project observations and committed user-input renewal continue through the real application host
