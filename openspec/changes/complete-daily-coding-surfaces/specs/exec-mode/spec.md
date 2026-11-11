## ADDED Requirements

### Requirement: Bounded interactive JSONL driver
(P1) `--input-format stream-json` SHALL parse bounded typed user/control/reply/end envelopes independently of model execution, retain durable admission identity and emit one correlated terminal result per admitted input under schema version 2. Replies SHALL target only live requests owned by that driver. EOF/end SHALL stop new admission and drain acknowledged owned work without leaving unattended asks pending indefinitely or cancelling unrelated Session work. Malformed, oversized and over-capacity frames SHALL produce explicit protocol errors rather than silent text admission. `--output-schema` SHALL use root structured results; unstructured stdin behavior SHALL remain compatible.

#### Scenario: Direct model requests retain their complete input cohort
- **WHEN** a model step starts on the active branch
- **THEN** its server-recorded cohort SHALL contain every input driving that step, preserve it across tool continuations and compaction, and exclude inactive branch promotions
- **AND** runtime append and replay SHALL refuse contradictory nonempty provenance; legacy or oversized cohorts SHALL grant no inferred ownership
- **WHEN** the step's exact tool call opens a permission or question request
- **THEN** the pending request SHALL expose its captured `input_message_ids`, bounded to 32 identities of at most 256 UTF-8 bytes each

#### Scenario: Two drivers have pending input in one Session
- **GIVEN** the first driver owns a live direct request and the second driver has admitted a different input
- **WHEN** the second driver sees that request, tries to reply, or closes while the first driver remains attached
- **THEN** it SHALL neither retain the request as its own nor reply to or dismiss it
- **AND** the first driver SHALL retain normal reply authority, while a closed driver SHALL decline only its own later direct requests
- **AND** user-provided correlation labels SHALL NOT grant request ownership

#### Scenario: Interrupt while inference runs
- **WHEN** an active JSONL driver sends an interrupt while its model request is streaming
- **THEN** the reader processes it without waiting for model completion and interruption applies only to the driver's owned execution

#### Scenario: Bounded background ownership lookup
- **WHEN** an authenticated client queries Job state for 1–32 unique input identities, each at most 256 UTF-8 bytes
- **THEN** the server SHALL return one small record per supplied identity without returning Job payloads or collecting Session history pages
- **AND** a Job SHALL count only when its complete valid durable input cohort belongs to that Session and the supplied set; missing, malformed, oversized, duplicate or mixed-cohort provenance SHALL grant no inferred ownership
- **AND** the lookup SHALL NOT mutate, cancel or settle work, and handback admission SHALL NOT be treated as proof that its parent Turn has completed

#### Scenario: A handback starts a request or another Job
- **WHEN** a Job's handback is admitted
- **THEN** its notifier acceptance SHALL commit with that input before inference starts, and a rejected notifier write SHALL publish neither the input nor inference
- **WHEN** the handback-driven step opens a request or starts another Job
- **THEN** its input provenance SHALL preserve the complete original input cohort and any new steering inputs through exact durable admission/notification/Job-origin evidence preceding that step
- **AND** a source label, synthetic name, later notification or contradictory Job origin SHALL NOT grant inferred ancestry; unknown or excessive cohorts SHALL NOT become partial ownership

#### Scenario: EOF with a pending approval
- **WHEN** the driver's input closes while one of its requests awaits a reply
- **THEN** that request follows the defined disconnected/non-interactive policy and the process does not wait forever or fabricate approval

#### Scenario: Last-message publication collides or fails
- **WHEN** a driver publishes its final assistant text to `--output-last-message`
- **THEN** it SHALL stage bytes in an exclusively created sibling file and atomically replace the destination without following a pre-existing temporary or destination link
- **AND** concurrent writers SHALL publish complete answers and clean up their own temporary files, including after publication failure
- **AND** a publication failure SHALL preserve per-input results, report the error in `end`, and fail an otherwise successful aggregate run without replacing an existing interruption or protocol exit code

#### Scenario: Completed inputs do not consume pending capacity
- **WHEN** a driver emits terminal results across a run longer than its pending-input limit
- **THEN** it SHALL release completed text, structured results and correlation records, retaining only bounded aggregate accounting
- **AND** the limit SHALL apply before each HTTP admission to pending inputs, completion SHALL preserve the exact identities of other pending inputs, and `end.inputs` SHALL still count retired attempts
- **AND** retiring a failed input SHALL NOT erase its aggregate failure; premature termination SHALL emit a terminal result for every remaining pending input
