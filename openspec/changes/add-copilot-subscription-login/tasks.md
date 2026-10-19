## 1. Spike

- [ ] 1.1 Register a Cyber Code GitHub OAuth App with device flow and test whether `copilot_internal/v2/token` accepts its tokens on a real Copilot account. Record the client ID decision in `design.md`.

## 2. Shared subscription store

- [ ] 2.1 Extract `cyber-llm::subscription` (`LoginFile<T>`, `refresh_locked`, `ModelCache<M>`) from `chatgpt`; ChatGPT unit, wiremock and CLI tests pass unchanged.

## 3. Copilot core (`cyber-llm::copilot`)

- [ ] 3.1 Device flow: code request, polling with `interval`/`slow_down`/expiry/denial/cancellation; wiremock tests for each outcome.
- [ ] 3.2 Login file (`host`, `github_token`, `user`) and in-memory Copilot token mint reused until 5 minutes before expiry; a 401 on mint gives the login hint and keeps the file.
- [ ] 3.3 Model list: picker-enabled, not policy-disabled, simple slugs; limits and capabilities; protocol per `supported_endpoints`; cache and daily refresh.

## 4. Runtime, adapter and CLI

- [ ] 4.1 `LlmRequest::initiator` set by the Turn loop (`user` on the first call after a promoted prompt, `agent` on continuations and compaction); runtime test.
- [ ] 4.2 `ApiKind::CopilotChat` adapter: protocol choice, token, Copilot headers, vision header; wiremock tests for both protocols and `X-Initiator`.
- [ ] 4.3 Catalog inserts `copilot` with a stored login; not selectable from config.
- [ ] 4.4 `cyber providers login|logout copilot [--enterprise <host>]`; Windows refuses; real-binary tests against a fake GitHub (device flow, no-plan refusal, logout).

## 5. Docs and verification

- [ ] 5.1 README "Use your subscription" section and the local core guide.
- [ ] 5.2 Live: sign in with a real Copilot plan, list models, and run tool Turns on a Chat Completions model and a Responses model; confirm premium-request usage counts one request per prompt.
- [ ] 5.3 `cargo fmt`, `cargo clippy --workspace -- -D warnings`, touched-crate tests, `openspec validate --strict`.
