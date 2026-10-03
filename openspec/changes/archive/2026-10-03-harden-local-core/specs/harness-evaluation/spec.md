## ADDED Requirements

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
