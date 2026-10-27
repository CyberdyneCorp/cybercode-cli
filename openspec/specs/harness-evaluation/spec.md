# harness-evaluation Specification

## Purpose
Measures coding quality and recovery behavior with reproducible fixtures, deterministic assertions and separately reported live-model evaluations. Release gates apply from the first local core.

## Requirements

### Requirement: Reproducible evaluation manifest
(P0) The repository SHALL contain versioned evaluation manifests identifying fixture repository commit, setup image digest or environment lock, task prompt, harness version, model ID and provider, decoding options, tool schemas, budgets, timeout and grading rules. Deterministic adapter fixtures SHALL be separate from live-provider runs. Live runs SHALL record resolved model metadata and timestamp and SHALL NOT claim deterministic model responses.

#### Scenario: Compare harness versions
- **WHEN** two harness revisions run the same evaluation manifest
- **THEN** the report identifies every changed harness, model, environment or prompt input

### Requirement: Coding quality and cost measures
(P0) `cyber eval run <manifest> --output <directory>` SHALL execute tasks in disposable trusted fixtures with an explicit budget, emitting a versioned JSON report of task success, regression checks, total and per-success cost, latency, tokens, human interventions and infrastructure failures. Functional grading SHALL use independent assertions and inspect final workspace state; model claims of completion SHALL NOT suffice. Reports SHALL keep failures in the denominator and label unpriced results. Grader commands and hidden expected results SHALL be outside agent-writable roots.

#### Scenario: Agent deletes failing tests
- **WHEN** an agent removes fixture tests to make its own test command pass
- **THEN** independent grading detects the missing behavior and the task fails

### Requirement: Recovery and trust release gates
(P0) Every release SHALL pass deterministic scenarios for admission crash boundaries, tool dispatch before settlement, disk-full, writer contention, replay gaps, untrusted config, secret-read attempts, permission ceilings, stale edits, rewind conflicts and repeated compaction. Later phases SHALL add parallel budget reservations, workflow replay and ownership transfer partitions. Process-kill tests SHALL be distinguished from filesystem or power-loss fault simulation. A report SHALL link each case to its owning requirement and list skipped cases as not verified.

#### Scenario: Crash after remote mutation
- **WHEN** the recovery fixture commits a fake remote mutation and crashes before local settlement
- **THEN** resume reconciles one mutation and the fixture observes no duplicate effect

### Requirement: Live quality release baseline
(P0) Before the first stable release, each supported provider adapter SHALL pass a published set of at least 20 coding tasks with 3 trials per task using declared budgets. Reports SHALL include all outcomes and uncertainty, and establish a reviewed versioned baseline. Subsequent releases SHALL fail the quality gate when task success drops by more than 5 percentage points or median cost per successful task rises by more than 20% under comparable model and environment conditions, unless a documented release exception explains the tradeoff. Changed provider models SHALL establish a separately labeled baseline. External benchmark comparisons SHALL identify exact versions and configurations.

#### Scenario: Model changed between releases
- **WHEN** the provider retires the model used by the previous baseline
- **THEN** the report marks the comparison non-equivalent and requires a new reviewed baseline

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

### Requirement: Local core exit evidence
(P0) The repository SHALL provide checked-in tool golden outputs, repeatable durable admission and writer workload measurements, real-terminal first-frame measurements on named hardware, and build jobs for each supported P0 target.

#### Scenario: Tool catalog grows
- WHEN a built-in tool is added to the host registry
- THEN the golden coverage guard SHALL fail until its output fixture is added

#### Scenario: Performance evidence
- WHEN the opt-in release probes run
- THEN they SHALL report workload, hardware, latency percentiles and storage measurements without requiring a live provider

#### Scenario: Platform builds
- WHEN CI runs
- THEN it SHALL build macOS arm64/x64 and Linux arm64/x64 with both gnu and musl

### Requirement: Complete evaluation token accounting
(P0) Evaluation trial reports and token budgets SHALL include input, output, reasoning, cache-read and cache-write tokens from completed main steps and successful compactions. Successful compaction cost SHALL contribute to trial cost without incrementing the Turn count. Any unpriced included work SHALL keep the trial marked unpriced. Legacy reports without a reasoning field SHALL remain identifiable by their harness revision and SHALL NOT have missing usage fabricated.

#### Scenario: Compaction exhausts a budget
- WHEN a successful compaction takes a trial above its declared token or cost budget
- THEN the evaluation harness SHALL request interruption at that accounting checkpoint without counting compaction as a main Turn

#### Scenario: Reasoning consumes the token allowance
- WHEN input and output fit the budget but reasoning takes the total above it
- THEN the trial SHALL report reasoning separately and SHALL request interruption

#### Scenario: Unpriced hidden work
- WHEN compaction cost is unavailable and a subsequent main step has a known price
- THEN the trial SHALL remain unpriced and its reported dollar total SHALL be a lower bound
