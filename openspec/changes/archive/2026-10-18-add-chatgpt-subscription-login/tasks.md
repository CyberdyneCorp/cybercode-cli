## 1. Auth core (`cyber-llm::chatgpt`)

- [x] 1.1 Constants (issuer, client ID, redirect URI, backend URL), the `CYBER_CHATGPT_ISSUER` override (HTTPS, or HTTP only on loopback), and the authorize URL builder; unit test for its parameters.
- [x] 1.2 Claims from the ID and access tokens (account ID, plan, email, `exp`) without signature checks; unit tests for present, missing and malformed claims.
- [x] 1.3 Code exchange (form) and refresh (JSON) requests with bounded responses and token-free errors; wiremock tests, including a 400 `invalid_grant`.
- [x] 1.4 The owner-only token file: load (refuse other-readable, non-regular or files over 64 KiB), atomic save, remove; unit tests.
- [x] 1.5 `AuthFile::fresh`: re-read, check whether the token is due, take the refresh lock, re-read under the lock, refresh, persist. Tests: concurrent refresh sends one request; a revoked refresh token leaves the file unchanged.

## 2. Catalog and adapter

- [x] 2.1 `ApiKind::ChatgptCodex`, not parseable from config or from models.dev data.
- [x] 2.2 `ChatGptAdapter`: fresh token, account/beta/originator headers, no `max_output_tokens`, `instructions` always present, `with_http_client`; wiremock test of the request shape.
- [x] 2.3 The catalog inserts `chatgpt` when a login is stored, with credential source `login`, before `providers` config applies; tests for both "provider appears after login" and "no login".
- [x] 2.4 The plan's model list: fetch `GET <backend>/models` (listed entries only), cache it in `<data>/chatgpt-models.json` at login, refresh it from the adapter when it is missing or older than 24 h, and wait a day after a failure; the catalog builds `chatgpt` models from the cache (replaces mirroring `openai` `gpt-5*`, which the live test showed was wrong).

## 3. CLI

- [x] 3.1 `CallbackListener` takes a callback path; MCP behavior is unchanged.
- [x] 3.2 `cyber providers login chatgpt [--method browser|code]` and `cyber providers logout chatgpt`; any other provider exits 2 with its environment variable names; Windows refuses.
- [x] 3.3 Real-binary tests: code-method login against a mock issuer (file written with mode 0600), forged state refused, logout, other provider.

## 4. Docs

- [x] 4.1 Document ChatGPT sign-in in `docs/local-core-guide.md`, and note it in `ROADMAP.md` M0.2.

## 5. Verification

- [x] 5.1 `cargo fmt`, `cargo clippy --workspace`, `cargo test` for the touched crates, and `openspec validate --strict`.
- [x] 5.2 Live (2026-10-18, ChatGPT `prolite` account): browser login, `cyber models chatgpt` listed the plan's 7 models (`gpt-6.1-sol`, `gpt-6-astra`, `gpt-6-sol`, `gpt-6-luna`, `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`), and Turns with tool calls completed on `chatgpt/gpt-6-sol` and `chatgpt/gpt-5.6-sol`. The backend accepted Cyber's own `instructions`. Before 2.4, the `openai` mirror listed `gpt-5.1`, which the backend refused.
