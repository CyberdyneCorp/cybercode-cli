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


#### Scenario: Owner storage nested under a Location
- **WHEN** owner data, configuration, state, cache or temporary roots lie below an observed Location
- **THEN** owner-supplied private roots and their canonical aliases are excluded before traversing those subtrees, from both initial metadata and subsequent deltas
- **AND** ordinary project observations and committed user-input renewal continue through the real application host
