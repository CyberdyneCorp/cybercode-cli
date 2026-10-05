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
