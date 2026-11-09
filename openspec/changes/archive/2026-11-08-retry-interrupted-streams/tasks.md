## 1. Replaying stream (`cyber-llm`)

- [x] 1.1 `open_with_retry` returns a `RetryingStream`. A retryable failure after output yields `LlmEvent::Discarded`, waits, and replays the same request. Retries before and after output share one budget. The callback receives a `RetryNotice` with `discarded`.
  - `LlmEvent::Discarded` is `#[serde(skip)]` and is not output. `collect` resets its output on it and keeps the summed usage.
  - Bare-adapter consumers (evaluator, prompt hook, MCP sampling) match the variant and never receive it.
- [x] 1.2 Real HTTP fault tests (`tests/transport_faults.rs`), covering:
  - partial text, reasoning, streamed arguments, and a complete call that is replayed and then succeeds;
  - one shared budget before and after output, with the final partial output returned;
  - a stalled stream replayed after the idle window;
  - no replay after protocol completion.

  Also in `tests/adapters.rs`: an in-stream Anthropic overload is replayed, and a discarded attempt's usage is kept by `collect`.

## 2. Runtime (`cyber-server`)

- [x] 2.1 The Drain keeps closed reasoning blocks and complete calls in `Accum::pending`, and commits them with the text in `end_step`/`stop_step`. `stop_step` settles undispatched calls from the accumulator.
- [x] 2.2 `Discarded` resets the accumulator. Usage reported by the discarded attempt becomes a failed `usage.recorded.1` (purpose `turn`, origin of the step), unless a reservation billed it (`reservations::reserved_call`).
- [x] 2.3 `session.retry` carries `message_id` and `discarded`.
- [x] 2.4 Tests:
  - history after a replay holds only the replayed attempt, before and after a restart;
  - the dispatched call runs once;
  - wasted usage is outside the step usage;
  - retry exhaustion still settles as interrupted (updated `transport_failure_preserves_tool_outcomes_without_replaying_actions`);
  - a complete call is recorded when its stream completes, before dispatch (replaces the eager durability test).

## 3. Clients

- [x] 3.1 TUI drops the streamed text of a discarded attempt (`app.rs`, test in `tests.rs`).
- [x] 3.2 Web dashboard clears `liveText` for the discarded live message (`model.ts`, `model.test.ts`).
- [x] 3.3 `cyber exec` stream-json `retry` line, plus a verbose text note. Recorded content only, so the result is the replayed answer.
- [x] 3.4 SDK: `prompt()` reads only recorded events. No change.

## 4. Docs and specs

- [x] 4.1 `docs/local-core-guide.md` retry paragraph, ROADMAP model-fidelity note, dated note in `docs/measurements/provider-stream-reliability-follow-up.md`.
- [x] 4.2 The "Stalled stream" scenario of `improve-model-protocol-fidelity` now describes the replay.
