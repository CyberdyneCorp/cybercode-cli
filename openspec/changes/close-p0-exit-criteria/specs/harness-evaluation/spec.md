## ADDED Requirements
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
