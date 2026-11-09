## 1. Provider-neutral layer

- [x] 1.1 Request, history and event types; usage classes; finish reasons.
- [x] 1.2 SSE parser, error classification and redaction, retry before output.

## 2. Adapters

- [x] 2.1 OpenAI-compatible Chat (also Ollama, llama.cpp, vLLM, OpenRouter).
- [x] 2.2 OpenAI Responses, with reasoning summaries and prompt cache keys.
- [x] 2.3 Anthropic Messages, with cache breakpoints and extended thinking.
- [x] 2.4 Scripted adapter for deterministic fixtures.

## 3. Catalog

- [x] 3.1 models.dev parsing, configured providers and models, variants, request layering.
- [x] 3.2 Credentials, availability reasons, roles, default model, recent models, cost tiers.
- [x] 3.3 Source: pinned path, fresh cache, locked refresh, stale fallback, bundled snapshot.

## 4. Surfaces and verification

- [x] 4.1 `cyber models` with text, verbose and JSON output.
- [x] 4.2 Wire tests per adapter, catalog tests, live test and example turn.
- [x] 4.3 Spec clarifications; ROADMAP status.
