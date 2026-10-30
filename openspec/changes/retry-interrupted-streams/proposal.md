## Why

A same-model pilot against Codex CLI lost a task to `transport: error decoding response body` in the middle of an answer ([pilot](../../../docs/measurements/gpt61-sol-chatgpt-pilot.md), [follow-up](../../../docs/measurements/provider-stream-reliability-follow-up.md)). `provider-catalog` → Retry policy only retries before any assistant output. Once the first text, reasoning or tool-input event arrives, a dropped connection, an idle timeout or an in-stream overload ends the Turn as interrupted, even though nothing of that answer has had any effect yet. Codex CLI handles this case by sending the whole request again and dropping the partial answer. We adopt the same behaviour, written from scratch.

## What Changes

- **Replay of an interrupted answer** (`provider-catalog`, MODIFIED "Retry policy").
  - A retryable failure (`Transport`, `ProviderInternal`, `RateLimit`, including the idle timeout and in-stream overload errors) after partial text, reasoning, streamed tool arguments or complete tool calls discards that attempt and sends the same request again.
  - Retries before and after output share the existing budget of 4 retries, backoff and server-advised delays.
  - Replay stops for good once a tool call of the answer has been dispatched and after protocol completion.
  - `session.retry` gains `message_id` and `discarded`. Clients drop the partial answer of that message when `discarded` is true.
  - A discarded attempt's reported usage is recorded as a failed `turn` call (`usage.recorded.1`). It is never folded into the step's usage, and never lost.
- **Durable content waits for the end of the stream** (`tool-registry`, MODIFIED "Eager execution during streaming"; `session-runtime`, MODIFIED "Eager tool execution"). Reasoning items and complete tool calls are recorded together with the text, when the stream completes or fails for good. Before that the attempt may still be discarded, and nothing durable may outlive it. Starting a call early still records it first, and ends replay for that answer.
- **`cyber exec` stream-json** (`exec-mode`, MODIFIED "Stream JSON event schema"). Adds a `retry` line. Exec only prints recorded content, so a discarded attempt never appears in its output.
- TUI and web dashboard drop the live text of a discarded attempt. The TypeScript SDK reads only recorded events and needs no change.

## Non-goals

- Resuming a broken stream from where it stopped (`previous_response_id`, WebSocket transport). That is part of the later native-transport change.
- Replaying an answer after one of its calls was dispatched. That stays a settled interruption, as before.
- Recording usage for the last failed attempt of an unreserved request, or for a failed attempt before any output. Their accounting does not change.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `provider-catalog`: "Retry policy".
- `tool-registry`: "Eager execution during streaming".
- `session-runtime`: "Eager tool execution".
- `exec-mode`: "Stream JSON event schema".

The open change `improve-model-protocol-fidelity` adds "Stream idle timeout". Its "Stalled stream" scenario now describes a replay instead of a settled interruption.

## Impact

- `crates/cyber-llm`: `open_with_retry` returns a replaying stream (`RetryingStream`), the callback receives a `RetryNotice`, and `LlmEvent::Discarded` is added. `collect` drops a discarded attempt's output.
- `crates/cyber-server`: the Drain records step content when the stream ends, handles `Discarded` (reset, wasted usage), and publishes the new `session.retry` fields. Compaction uses the same path.
- Clients: TUI (`crates/cyber-tui`), `cyber exec` (`crates/cyber-cli`) and the web dashboard (`web/src/model.ts`).
- Docs: `docs/local-core-guide.md`, ROADMAP model-fidelity entry, a dated note in the reliability follow-up.
