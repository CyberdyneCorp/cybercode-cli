## ADDED Requirements

### Requirement: Durable provider reservation lifecycle
(P2) For reserved enforcement, the server SHALL atomically persist a request-identified reservation in every applicable budget scope before provider dispatch, including auxiliary calls and descendants. Admission SHALL compare recorded usage plus outstanding reservations against each limit using conservative bounded estimates. Unbounded or unpriced USD requests SHALL fail with `BudgetUnpricedError`. Settlement SHALL reconcile actual usage and unused allocation once by request identity; interruption after possible dispatch without authoritative usage SHALL retain an uncertain allocation across restart. Only authoritative reconciliation or an explicit audited owner release SHALL clear that uncertainty. Existing soft enforcement SHALL remain unchanged.

#### Scenario: Concurrent requests compete for the final allocation
- **WHEN** two provider requests individually fit a reserved scope but their combined estimates exceed its remaining allowance
- **THEN** the atomic ledger admits only a request whose allocation fits and the refused request makes no provider call

#### Scenario: Crash after possible dispatch
- **WHEN** the server restarts after a reserved request may have reached the provider but no final usage was recorded
- **THEN** its allocation remains uncertain and unavailable for new dispatch until explicitly reconciled or released

#### Scenario: Duplicate settlement
- **WHEN** a request's usage callback or durable recovery is processed twice
- **THEN** recorded spend and released allocation change only once

#### Scenario: Auxiliary call cannot evade a run budget
- **WHEN** a workflow descendant's title, evaluator, advisor or compaction request would exceed an applicable reserved limit
- **THEN** admission is refused before network dispatch and the report identifies that budget and purpose
