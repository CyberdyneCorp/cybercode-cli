## ADDED Requirements

### Requirement: Pinned real-repository pilot
(P0) The system SHALL support real-repository evaluation tasks with upstream repository identity, immutable base revision, verified source tree, license notices, locked setup, task prompt and independent grading provenance. The initial pilot SHALL include four tasks across two repositories, including at least two Rust tasks, with three trials per task and strategy. Reference fixes and later Git objects SHALL remain outside candidate-visible roots. Live execution SHALL require explicit invocation after setup and disk/toolchain checks; it SHALL NOT run in ordinary PR CI.

#### Scenario: Invalid source setup
- **WHEN** the materialized source or required locked environment does not match its recorded identity
- **THEN** the task SHALL fail setup before model dispatch
- **AND** the report SHALL distinguish that infrastructure failure from a graded candidate failure

#### Scenario: Independent grader controls
- **WHEN** a task enters the frozen pilot
- **THEN** its untouched base SHALL fail the regression and its reference fix SHALL pass independent and relevant upstream checks
- **AND** deleting candidate-visible tests SHALL NOT manufacture an independent pass

### Requirement: Separate verified solve evaluation
(P2) The evaluation runner SHALL retain single-agent execution as its default strategy and offer explicitly selected bundled solve execution. The initial solve pilot SHALL use two independent candidates and an independent read-only judge, three runs per task. The independent grader SHALL inspect the exact selected source after host verification and judge selection, without giving hidden grader feedback to solvers or judge. Missing recommendations SHALL NOT count as task success. Lifecycle completion, candidate success and selected-patch success SHALL be separately reported.

#### Scenario: Workflow completes without a recommendation
- **WHEN** a solve workflow completes but recommends no candidate
- **THEN** its task outcome SHALL NOT be a pass
- **AND** candidate and verification outcomes SHALL remain visible without replacing the selected-patch outcome

#### Scenario: Repeated live pilot reports
- **WHEN** single-agent and solve trials run against a frozen task set
- **THEN** reports SHALL retain every attempt, infrastructure failure, strategy, source identity, requested/resolved model metadata, declared budgets, token/pricing uncertainty and timing
- **AND** reports SHALL keep previous published benchmark results unchanged
- **AND** the four-task pilot SHALL NOT claim completion of the twenty-task-per-adapter stable-release gate or elimination of intermittent failures
