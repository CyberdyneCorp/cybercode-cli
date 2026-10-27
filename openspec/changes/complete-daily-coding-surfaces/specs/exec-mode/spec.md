## ADDED Requirements

### Requirement: Bounded interactive JSONL driver
(P1) `--input-format stream-json` SHALL parse bounded typed user/control/reply/end envelopes independently of model execution, retain durable admission identity and emit one correlated terminal result per admitted input under schema version 2. Replies SHALL target only live requests owned by that driver. EOF/end SHALL stop new admission and drain acknowledged owned work without leaving unattended asks pending indefinitely or cancelling unrelated Session work. Malformed, oversized and over-capacity frames SHALL produce explicit protocol errors rather than silent text admission. `--output-schema` SHALL use root structured results; unstructured stdin behavior SHALL remain compatible.

#### Scenario: Interrupt while inference runs
- **WHEN** an active JSONL driver sends an interrupt while its model request is streaming
- **THEN** the reader processes it without waiting for model completion and interruption applies only to the driver's owned execution

#### Scenario: EOF with a pending approval
- **WHEN** the driver's input closes while one of its requests awaits a reply
- **THEN** that request follows the defined disconnected/non-interactive policy and the process does not wait forever or fabricate approval
