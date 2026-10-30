# Provider stream reliability follow-up

This follow-up to the [original same-model pilot](gpt61-sol-chatgpt-pilot.md) reproduces interrupted HTTP response bodies locally and records new attempts for the two Cyber tasks that originally stopped with transport errors. The original pilot's reports and scores are unchanged.

## Confirmed bug and fix

A local HTTP server sends valid Responses SSE events, including `response.completed` and final usage, then closes a chunked body without its terminating chunk. Before the fix, Cyber reports `Transport: error decoding response body` despite receiving complete protocol output. The shared SSE decoder now stops at its terminal `Finish` event. The result and usage survive a broken tail, and a held-open HTTP connection no longer delays completion.

Before protocol completion, a body failure remains a transport error. Diagnostics now identify the response-stream stage and received body-byte count while retaining existing credential redaction. They do not add response content, credentials or arbitrary low-level exception chains. The underlying network/provider/client cause is still unknown.

Automatic retry behavior is unchanged: up to four retries before output, none after text, reasoning, streamed tool arguments or a completed call. There is no transparent replay of partially completed work.

Update (2026-10-29, change `retry-interrupted-streams`): this paragraph describes the behaviour measured here. A retryable failure after partial output is now discarded and the request is replayed, within the same four retries, as long as none of the answer's tool calls has been dispatched (see `provider-catalog` → Retry policy). The results below predate that change.

## Local verification

Six real HTTP fault tests cover completed output followed by broken framing, completed output with a held-open connection, failure before output followed by success, failure after partial output, clean HTTP EOF without protocol completion, and retry exhaustion. The partial-output case checks text, reasoning, incomplete tool arguments and completed calls independently. The initial broken-tail regression fails against the original decoder with the same generic error observed in the pilot; it passes with the fix.

A runtime regression completes one mutation, then interrupts the next provider response after a new tool call has streamed. It asserts that the first mutation ran exactly once, the second call never dispatched, and both outcomes survive restart and explicit continuation without replay. The resumed request includes the recorded outcomes.

- LLM crate: 135 test functions reported passed, including six fault tests; the opt-in live-provider test returned without running because `CYBER_LIVE_TESTS` was unset. This is 134 exercised offline tests, not live provider acceptance.
- Runtime integration suite: 43 passed.
- Clippy: LLM and server crates, all targets, warnings denied; passed.
- Formatting, whitespace checks and strict OpenSpec validation: passed (101 OpenSpec items).
- CLI build: succeeded; the linker emitted an unwind-table size warning.
- Native Windows/Linux tests were not run locally. The portable fault tests and runtime regression are included in the Windows CI selection.

## Follow-up protocol

One new attempt per selected task, sequentially, in independent disposable Git workspaces. Requested model `chatgpt/gpt-6.1-sol#medium`; Python 3.13.5 and Node 22.18.0. All 505 original manifest/fixture/grader file hashes match. Original prompts, budgets, graders, native command-network policy and isolated benchmark environment are reused. The existing credential store and refresh locks are shared; credentials are not copied or published. No normal server sessions are used. The unchanged original sandbox helper is paired with a freshly built, immutable CLI.

The follow-up measures the embedded/in-memory eval path, not TUI startup or registered-server latency. Dollar pricing is unavailable for the subscription. These are two selected one-trial observations; they cannot establish statistical improvement, eliminate intermittent transport failures, or prove that the completed-response bug caused either original live failure. Backend model weights are not independently pinned. No candidate receives grader feedback or a replacement attempt.

## Follow-up results

| Task | Original pilot | Fresh follow-up | Native latency |
|---|---|---|---:|
| `build-inventory-cli` | Transport failure; not graded | Pass | 271.7 s |
| `refactor-callbacks-async` | Transport failure; not graded | Pass | 251.6 s |

These observations remain separate from the original standard 19/20 and hard 17/20 Cyber scores. A successful retry is evidence of one completed attempt, not proof that intermittent failures are resolved.

Source revision: `ac97d4104aa9b55bbf3908225e866bbc9ea90dba`. [Build provenance](provider-stream-reliability-follow-up/build.json), [test record](provider-stream-reliability-follow-up/checks.json), [summary](provider-stream-reliability-follow-up/summary.json), [integrity checks](provider-stream-reliability-follow-up/verification.json). Native reports: [inventory CLI](provider-stream-reliability-follow-up/build-inventory-cli.json) and [async refactor](provider-stream-reliability-follow-up/refactor-callbacks-async.json). Reports redact local directory prefixes only; numeric outcomes are unchanged. Environment/auth files, raw transcripts and private workspaces are not published.
