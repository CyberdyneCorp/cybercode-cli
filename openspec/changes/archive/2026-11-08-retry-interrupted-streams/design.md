## Context

Where we stand:
- `cyber-llm/src/retry.rs`: `open_with_retry` buffers each attempt until its first output event (`first_output`). A failure before that is retried. After it, the stream is handed to the caller, and any later error ends that stream.
- `cyber-server/src/runtime/drain.rs` (`consume`, `on_stream_event`) publishes live deltas and, while the stream is still open, commits `session.reasoning.ended.1` for every closed reasoning block and `session.tool.called.1` for every complete call. Text is committed at the end (`end_step`, or `stop_step` on failure).
- Tool calls are dispatched only in `execute_tools`, which runs after `end_step`, so after the stream has completed. Nothing in the Drain starts a call while a stream is open. `tool-registry` → "Eager execution during streaming" allows early starts (MAY), but none is implemented.
- Each attempt goes through `GatedAdapter::stream` (`reservations/dispatch.rs`). It checks the Session budget for `turn` and `compaction`, reserves the request when reserved budgets apply, and settles the reservation when that attempt's stream ends or fails (`reservations/stream.rs`).
- `TurnState` (`x-codex-turn-state`) is shared by every request of a Turn, retries included. It keeps the first value it receives.

Codex CLI, by behaviour only: a disconnected stream re-sends the whole request and drops the partial answer, within its stream retry budget.

## Decisions

### D1. Replay inside the retrying stream
`open_with_retry` opens the first attempt as before, then returns a stream that can reopen itself. When a retryable failure interrupts an attempt with output, the stream yields `LlmEvent::Discarded`. It then waits the policy delay and yields the events of a new attempt of the same request. Retries before output (inside `open`) and after output share one counter, so the total stays at 4 retries. The callback receives a `RetryNotice { attempt, delay, error, discarded }`. When the budget runs out, or the error is not retryable, the error is returned after the final attempt's partial output. The caller then settles the Turn as interrupted, as today.

Keeping the replay in `cyber-llm` means compaction, which uses `open_with_retry` and `collect`, gets it without its own code. Title, recap, side chat, evaluator, prompt hooks and MCP sampling call `Adapter::stream` directly and never retried. They do not change.

`Discarded` is a variant of `LlmEvent` with `#[serde(skip)]`, so adapters and scripts cannot produce it. `collect` resets its output on it and keeps the summed usage.

### D2. A completed but undispatched tool call does not block the replay
Decision: only a dispatched call (or one whose execution started) blocks replay. A call that is complete in the stream but not dispatched is discarded with the rest of the attempt.

Why this is safe: a call has no effect until it is dispatched. In the Drain, dispatch (`execute_tools` → `dispatch`, which records `session.tool.dispatched.1` before running anything) happens only after `consume` has returned the completed stream. By then the retrying stream has ended and cannot replay. So no call of a replayed answer can have run. This is what makes discarding the call safe, and the spec states it as a rule: the Drain SHALL NOT start a call of an answer while its stream can still be replayed. If early starts are built later, starting a call records the answer so far and ends replay for that answer. `cyber-llm` cannot see dispatch, so the rule is enforced where dispatch happens, not by a flag inside the stream.

Protocol completion (`Finish`) ends the stream, and the adapter does not wait for the HTTP tail (`completed_response_survives_broken_http_tail`). So a completed answer is never replayed.

### D3. Nothing durable from a discarded attempt
If closed reasoning blocks and complete calls were committed while the stream is open, a discarded attempt would leave `session.reasoning.ended.1` and `session.tool.called.1` events in history. Every projection would then need to undo them: the session fold, the SQL projections, MCP serve, telemetry, exec and eval. Instead, the Drain keeps them in the step accumulator (`Accum::pending`, in stream order) and commits them in the same batch as the text: in `end_step` when the stream completes, or in `stop_step` when it fails for good. A discarded attempt resets the accumulator, so nothing of it is ever written. That covers reasoning items with opaque state, calls, text and the reasoning block index.

Trade-offs:
- `tool-registry` → "Eager execution during streaming" required each complete call to be durable "as soon as its input finishes streaming". It now says: no later than stream completion, and before the call starts. Calls are still recorded before any dispatch.
- A crash while a stream is open now loses the step's streamed reasoning and calls as well as its text. Recovery records the open step as `interrupted`, as before. None of those calls had been dispatched, so nothing with an effect is lost. Before, recovery settled them as "recovered: never dispatched".
- Clients see `session.tool.called.1` when the stream ends, not when the call completes. The live `session.tool.input.delta` still shows calls while they stream.

`stop_step` now settles the step's undispatched calls from the accumulator, because they are not in Session state yet.

### D4. Usage of a discarded attempt
Decision: when a discarded attempt reported usage (Anthropic sends input tokens in `message_start`, and some gateways send usage early), the Drain records it at once as `usage.recorded.1`: purpose `turn`, `call_id` = the step's message ID, `failed: true`, the step's origin, and the price from the catalog. It counts toward Session totals, budgets and the usage ledger. It is not added to `session.step.ended.1`, whose usage drives the context meter: adding a second copy of the input tokens there would overstate context use and could trigger compaction early.

When a reservation billed the attempt, the reservation's own receipt already records that attempt's usage (`usage.recorded.1`, `failed: true`). The Drain then records nothing, so no token is counted twice. Most interrupted streams report no usage, and then nothing is recorded.

Not changed: a failed attempt before any output (its buffered usage is dropped, as today) and the last failed attempt of a step. Reserved budgets already bill both.

### D5. Budgets and reservations
Each replay calls `GatedAdapter::stream` again. Main-turn and compaction attempts run `check_budget` again, so a hard cap reached by the discarded attempt stops the replay with the budget error. When reserved budgets apply, each attempt is reserved on its own, and the discarded attempt's reservation is settled (`flush`) before its error reaches the retrying stream. So there are never two open reservations for one step, and nothing escapes the cap. `session.step.ended.1` is still marked accounted for every reservation of the step (`mark_for_call`).

### D6. Turn state and reasoning indices
The replay reuses the same `LlmRequest`, so its `Routing.turn_state` is the Turn's shared `TurnState`. A turn state that the discarded attempt received is kept and sent on the replay, as it is for retries before output. Each Turn has one value, the first one received. The reset of the accumulator also resets the reasoning block counter, so the recorded items of the replayed attempt are numbered from 0.

### D7. Clients
- `session.retry` gains `message_id` and `discarded`. It is a live-only event: no OpenAPI schema or SDK type describes it.
- TUI: on `discarded`, remove the message's entry from `streaming` (`crates/cyber-tui/src/app.rs`).
- Web: on `discarded` for the live message, clear `liveText` (`web/src/model.ts`).
- `cyber exec`: prints only recorded content, so a discarded attempt never reaches stdout. Stream-json adds a `retry` line (additive, schema version 1).
- SDK `prompt()` reads only recorded events. No change.

## Risks / Trade-offs

- A provider that charges for the discarded attempt now charges for one more full request. The retry budget limits this, the hard caps are checked before each attempt, and the wasted usage is recorded where the provider reports it.
- When an answer is long, the user may see part of it, then see it cleared, then see it streamed again. The `session.retry` notice explains why.
