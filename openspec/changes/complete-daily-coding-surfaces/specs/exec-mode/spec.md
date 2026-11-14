## ADDED Requirements

### Requirement: Bounded interactive JSONL driver
(P1) `--input-format stream-json` SHALL parse bounded typed user/control/reply/end envelopes independently of model execution, retain durable admission identity and emit one correlated terminal result per admitted input under schema version 2. Replies SHALL target only live requests owned by that driver. EOF/end SHALL stop new admission and drain acknowledged owned work without leaving unattended asks pending indefinitely or cancelling unrelated Session work. Malformed, oversized and over-capacity frames SHALL produce explicit protocol errors rather than silent text admission. `--output-schema` SHALL use root structured results; unstructured stdin behavior SHALL remain compatible.

#### Scenario: Direct model requests retain their complete input cohort
- **WHEN** a model step starts on the active branch
- **THEN** its server-recorded cohort SHALL contain every input driving that step, preserve it across tool continuations and compaction, and exclude inactive branch promotions
- **AND** runtime append and replay SHALL refuse contradictory nonempty provenance; legacy or oversized cohorts SHALL grant no inferred ownership
- **WHEN** the step's exact tool call opens a permission or question request
- **THEN** the pending request SHALL expose its captured `input_message_ids`, bounded to 32 identities of at most 256 UTF-8 bytes each

#### Scenario: An admission acknowledgement is lost
- **WHEN** a prompt submission and its single resubmission under the same message identity both lose their response
- **THEN** the driver SHALL read the Session inbox for that message identity before reporting the input
- **AND** a held row that is not refused SHALL report the input `admitted` and track it to its result; no row SHALL report it `refused`
- **AND** when the inbox cannot be read the run SHALL end with a fatal error (exit 2) naming the message identity rather than report an input the server may run as refused

#### Scenario: Two drivers have pending input in one Session
- **GIVEN** the first driver owns a live direct request and the second driver has admitted a different input
- **WHEN** the second driver sees that request, tries to reply, or closes while the first driver remains attached
- **THEN** it SHALL neither retain the request as its own nor reply to or dismiss it
- **AND** the first driver SHALL retain normal reply authority, while a closed driver SHALL decline only its own later direct requests
- **AND** user-provided correlation labels SHALL NOT grant request ownership

#### Scenario: Only the bound input client can keep an owned ask waiting
- **WHEN** an input-client lease admits an original root user input
- **THEN** its immutable binding SHALL commit atomically with admission, participate in exact retry identity, and match that admission's digest
- **WHEN** its complete recorded input cohort opens a direct request, including through an authenticated Job handback
- **THEN** only one live input client bound to that complete cohort and Location SHALL keep it waiting
- **WHEN** that client releases or expires while another client, browser or stream remains present
- **THEN** its requests SHALL settle unattended without borrowing other attendance, and the other client's requests SHALL remain independent
- **AND** scoped input leases SHALL grant neither Session-wide attendance nor native computer authority; malformed, missing scoped evidence and mixed bindings SHALL NOT fall back to legacy attendance

#### Scenario: A descendant request carries its root input cohort
- **WHEN** a child Session asks for permission or a question and every link from it up to its root Session is a running background subagent Job whose immutable input origin is complete, bounded (1–32 unique identities of at most 256 UTF-8 bytes) and identical across that link's running Jobs
- **THEN** the durable and routed request SHALL expose `root_session_id` and that root link's `root_input_message_ids`
- **AND** when the root cohort is bound to an input client, only that live client at the bound Session and Location SHALL keep the request waiting and have its reply accepted; its release or expiry SHALL settle the request unattended without borrowing browser, stream or other client attendance
- **WHEN** a link is missing (a foreground or detached child, or one whose background Job is no longer running), mixed, malformed, oversized, cyclic or deeper than 32 links
- **THEN** the request SHALL carry no root cohort and SHALL keep its existing attendance; it SHALL grant no inferred input ownership
- **WHEN** the server restarts
- **THEN** the root cohort SHALL remain derivable from durable Job provenance and recorded requests, but SHALL NOT recreate the bound client's live attendance, and the interrupted Job SHALL NOT stamp new requests

#### Scenario: The driver owns descendant and hook-originated requests
- **WHEN** a request routed to the driven Session carries `root_session_id` equal to that Session and every `root_input_message_ids` identity is a pending input of the driver
- **THEN** the driver SHALL emit it with the asking child Session as `request_session_id`, accept a reply only for that request kind, and post the reply to the child Session
- **AND** the owning inputs SHALL stay pending while the request waits; EOF/end SHALL settle it (lease release, then decline) instead of leaving it pending
- **WHEN** another driver of the same Session sees that request
- **THEN** it SHALL neither emit nor answer it, and its reply SHALL be a protocol error
- **WHEN** a `PreToolUse` hook decision of `ask` opens a permission request
- **THEN** that request SHALL carry its step's input cohort and be owned like a direct request
- **WHEN** a tracked request is answered elsewhere
- **THEN** the driver SHALL stop tracking it

#### Scenario: Input presence survives as provenance rather than live authority
- **WHEN** an already admitted input is retried after expiry or restart
- **THEN** the original admission receipt SHALL remain retrievable without renewing its lease, and a different client under that input identity SHALL conflict
- **AND** replayed binding metadata SHALL NOT recreate live attendance; legacy admissions without scoped negotiation SHALL preserve their existing behavior
- **WHEN** the driver observes its Session events
- **THEN** its connection SHALL explicitly opt out of broad Session presence while preserving replay/live cursor behavior and ordinary stream defaults

#### Scenario: Replies cannot cross input clients or consume a different request type
- **WHEN** an input client's request is answered
- **THEN** a scoped reply SHALL select its waiter under the live attendance check, and reject/always cascades SHALL NOT select another input client's pending requests
- **AND** a stop reply SHALL signal only the exact Drain whose complete current cohort belongs to that request, while legacy behavior remains compatible
- **WHEN** a question reply names a permission request or conversely
- **THEN** the API SHALL refuse without consuming the original request or stopping its work

#### Scenario: Interrupt while inference runs
- **WHEN** an active JSONL driver sends an interrupt while its model request is streaming
- **THEN** the reader processes it without waiting for model completion and interruption applies only to the driver's owned execution

#### Scenario: Retained interruption cannot target a replacement Drain
- **WHEN** an interruption selects 1–32 unique original input identities and a stable request identity
- **THEN** its intent and queued refusals SHALL commit before signalling the exact captured Drain, whose complete input cohort must belong to the selection
- **AND** mixed or unavailable provenance SHALL NOT authorize cancellation of that Drain or withdrawal of its requests
- **WHEN** the requester disappears or retries after another Drain starts
- **THEN** cleanup SHALL retain its runtime shutdown owner, wait for the captured owner's actual disposal, and return the original receipt without selecting replacement execution
- **AND** a different cohort under the same request identity SHALL refuse; missing retained cleanup or a changed runtime generation SHALL report unknown reconciliation rather than inventing acknowledgement

#### Scenario: Interruption acknowledgement is ahead of client replay
- **WHEN** the server has acknowledged the exact retained interruption but its receipt revision is ahead of the driver's consumed durable events
- **THEN** the driver SHALL retain the selected input slots until replay reaches that revision
- **AND** polling SHALL leave stdin and event processing live; a newer input and its request reply authority SHALL remain independent
- **WHEN** a submission response is lost
- **THEN** lookup and any necessary resubmission SHALL use the same request identity and original cohort, without a Session-wide fallback

#### Scenario: An interrupted input has direct process Jobs
- **WHEN** the captured Drain has released and pending Job publication has crossed its admission barrier
- **THEN** cancellation SHALL select only proven direct process Jobs of the exact Session and complete selected cohort, in bounded identity pages
- **AND** detached, mixed and descendant work SHALL NOT gain cancellation authority from Session membership; unresolved selected scope SHALL report unknown
- **WHEN** a completion notifier races the interruption intent
- **THEN** its exact authenticated handback SHALL remain refused and SHALL NOT restart the interrupted input, including after receipt settlement or restart
- **AND** these control receipts SHALL NOT acknowledge native effects or erase ordinary recovery evidence

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

#### Scenario: Background work outlives the initial answer
- **WHEN** the initial answer ends while its input still owns running background work or an admitted handback with unfinished model/tool work
- **THEN** the driver SHALL retain that input's pending slot and SHALL NOT emit its terminal result
- **WHEN** the handback ends and the driver has consumed the durable revision observed by the bounded Job query
- **THEN** the driver SHALL emit the correlated final handback answer and release that slot
- **AND** completion of one driver's input SHALL NOT wait for another driver's active question or adopt its text

#### Scenario: Replay and live notices share a driver connection
- **WHEN** the driver opens a Session event stream with a durable cursor and `live=true`
- **THEN** it SHALL receive contiguous durable replay plus live idle/delta notices for that Session on the same owned connection
- **AND** live notices SHALL NOT advance the durable SSE cursor; omitting the option SHALL preserve the existing stream behavior

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

#### Scenario: EOF during an owned handback question
- **WHEN** EOF closes a driver while a handback question belongs to its retained original input
- **THEN** it SHALL decline that exact request and drain its resulting answer under the original input identity without hanging
- **AND** an input stopped without a recorded final answer SHALL fail unless a valid structured outcome already establishes its terminal result; a valid structured object containing null fields SHALL remain successful

#### Scenario: Hook events are read from durable receipts of owned tool calls
- **WHEN** a driven run with `--include-hook-events` has an owned step call a tool whose hooks run
- **THEN** after that call settles and when the Session goes idle the driver SHALL read the Session's durable hook receipts through the authenticated hook-execution listing, without adding receipts to the Session event stream
- **AND** it SHALL emit one `hook_event` line per settled receipt whose `call_id` belongs to an owned step, carrying the receipt, hook id, event, call, tool, outcome, decision kind, duration and the owning inputs' `id`/`message_id`, deduplicated by receipt id within a bounded seen-set
- **AND** a receipt still running SHALL be reported once it settles; Session-scoped receipts without a `call_id` and receipts of other drivers' calls SHALL NOT be reported
- **AND** each read SHALL stop at a receipt older than the run (less a bounded clock allowance), at an already reported receipt when no owned receipt is still running, or after a bounded number of pages
- **AND** the line SHALL NOT carry hook stdin, stdout, stderr, reasons or other hook-supplied text, and the flag SHALL be refused with exit 2 without `--input-format stream-json`
