# Design

## Decisions

**One stream type, decoders per protocol.** Each adapter builds a JSON body and plugs a `Decoder` into shared SSE plumbing. The plumbing handles chunk reassembly, transport errors and end-of-stream, and stops at the first error. Tool calls accumulate by protocol key (index or item ID) and finish with the full argument text, so a call with invalid JSON is reported with `input: None` rather than guessed.

**Retry before output only.** `open_with_retry` buffers events until the first output event. An error before that, including after a usage-only `message_start`, is retried. Once output exists, the stream is handed to the caller, so a failure mid-answer is never replayed as though nothing happened.

**Anthropic effort.** Variants that name an effort level map to an extended-thinking budget (low 4,096 up to max 63,999 tokens), and `max_tokens` is raised above the budget. Temperature is dropped when thinking is on, as the API requires.

**Token classes.** `input` excludes cache reads and writes, and `output` excludes reasoning (OpenAI reports both inclusive). Anthropic does not separate thinking tokens, so they stay in `output`.

**Credentials.** The order is config key, then `CYBER_AUTH_CONTENT` (which stands in for stored connections), then the provider's environment variables. A provider needs no key when its URL is loopback or its config sets `auth: "none"`. Debug output of a credential is redacted.

**Snapshot.** The bundled catalog keeps only providers whose SDK maps to a P0 adapter, drops descriptions, and is gzipped reproducibly (283 KB for 6,724 models). The live catalog still lists every provider, with `adapter_unsupported` where needed.

## Tradeoffs

Bundling 283 KB grows the binary but makes first run and air-gapped use work. The concurrent-refresh lock serializes fetches across processes; a waiting process re-checks freshness after the lock, so only one request is sent.
