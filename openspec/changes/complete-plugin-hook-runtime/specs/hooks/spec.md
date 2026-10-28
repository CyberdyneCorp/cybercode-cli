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
