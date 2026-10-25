## ADDED Requirements

### Requirement: Integration batch manifest
(P2) The server SHALL accept owner-selected contribution batches with `intg_` IDs and immutable attempts pinning canonical repository, base/target commit, full contribution commits, optional verified hub attribution, dependencies/order, owner-selected verification commands and budget. Default limits SHALL be 20 contributions per batch and 100 pending batches per server. A new source/base/check definition SHALL require a new attempt. Hub-linked attribution SHALL require current authorization and SHALL NOT fall back to an unverified local claim during an Auth outage.

#### Scenario: Branch moves after submission
- **WHEN** a contribution branch changes after its full commit was pinned
- **THEN** the attempt continues using the pinned commit and reports the branch change without silently replacing the contribution

#### Scenario: Invalid dependency graph
- **WHEN** a batch contains a dependency cycle or a contribution from an unrelated repository history
- **THEN** admission fails before preparing an execution worktree

### Requirement: Combined source verification
(P2) The server SHALL compose contributions in deterministic dependency order in an isolated managed worktree, preserving the owner's checkout/target. It SHALL execute the owner-selected checks against the final combined source even if individual checks passed, bind evidence to complete source fingerprints and command/environment identities, and detect source mutation after checks. Contributor reports or changed repository check configuration SHALL NOT replace the admitted host verification contract.

#### Scenario: Individually passing patches fail together
- **WHEN** two accepted contributions pass independently but their combined tree fails a declared integration check
- **THEN** the combined attempt fails verification and cannot be presented as ready based on the individual passes

#### Scenario: Textual merge conflict
- **WHEN** composition finds conflicting files
- **THEN** the attempt records conflict paths and stops before certifying a combined patch, without modifying the owner's working tree

### Requirement: Independent integration review and report
(P2) The server SHALL request review from a separate read-only reviewer and present included contributions, exact combined/base commits, conflict/check outcomes, bounded evidence/logs, review recommendation/limitations and cost uncertainty. Verification authority SHALL derive from server-owned records, never reviewer text or remote task reports. An unavailable reviewer SHALL be explicit and SHALL NOT count as approval.

#### Scenario: Fabricated green report
- **WHEN** a contributor submits JSON claiming successful checks without matching host records
- **THEN** the report retains that claim as unverified attribution and does not grant verified status

### Requirement: Integration recovery and explicit publication
(P2) Integration queue actions SHALL be durable, revision-fenced and idempotent. Target advancement SHALL mark a verified attempt stale; uncertain native operations SHALL remain unknown until reconciled. Publishing a prepared branch/PR SHALL require explicit owner review of destination and exact attempt, persist remote effect identity and reconcile uncertain acknowledgements before retry. The queue SHALL NOT force-push, automatically merge or repeat an uncertain check/publication.

#### Scenario: Target advances before publication
- **WHEN** the target tip differs from the attempt's pinned tip
- **THEN** publication refuses the stale evidence and requires an explicitly rebuilt/reverified attempt

#### Scenario: Lost PR creation acknowledgement
- **WHEN** publication may have succeeded but its response was lost
- **THEN** recovery reconciles the exact repository/head/attempt before any new PR creation
