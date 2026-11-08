# Implement M0.2: first turn

## Why

Milestone M0.2 (ROADMAP) makes Cyber Code talk to models. It covers the catalog from models.dev with an offline snapshot, adapters for OpenAI Responses, OpenAI-compatible Chat, Anthropic Messages and local servers, and a streaming Turn with tool calls. The session runtime that drives Turns arrives in M0.3; this milestone delivers the provider-neutral layer it will call.

Building it showed that two contracts were underspecified. The credentials spec never said when a provider "requires no credential". The availability requirement never listed the reasons clients display.

## What Changes

- `cyber-llm` crate:
  - Provider-neutral request, history and event types.
  - SSE parser and error classification with credential redaction.
  - Retry policy that retries only before any output and honors `retry-after(-ms)`.
  - Tool-call accumulator that reports unparseable arguments.
  - Native adapters for OpenAI Responses, OpenAI-compatible Chat (Ollama, llama.cpp, vLLM, OpenRouter) and Anthropic Messages, with prompt-cache hints, reasoning variants and request overlays.
  - A scripted adapter for deterministic fixtures.
- Catalog:
  - models.dev parsing, configured providers and models, credential resolution, availability reasons, reasoning variants, request option layering, model references, roles and default-model resolution.
  - Recent-model state and tiered cost accounting.
  - A source with a pinned path, a 5-minute fresh cache, a locked network refresh with retries, stale-cache fallback and a bundled gzip snapshot (`scripts/refresh_models_snapshot.py`).
- `cyber models [provider] [--verbose] [--refresh] [--format json]`.
- `cargo run -p cyber-llm --example turn` runs a real two-step exchange with a tool call. `CYBER_LIVE_TESTS=1` enables live provider tests.
- Spec clarifications for "requires no credential" and the availability reason set.

## Impact

No existing requirement is weakened. These parts are deferred:
- Keyring-stored connections and `cyber providers login` (M0.5).
- Background hourly catalog refresh and `catalog.updated` events (M0.3, they need the server).
- Tool-call emulation and additional native providers (P1).
