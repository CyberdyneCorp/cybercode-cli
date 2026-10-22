## 1. Shared login and catalog reload

- [x] 1.1 `chatgpt::complete_login` in `cyber-llm`; `cyber providers login chatgpt` uses it and its tests pass unchanged.
- [x] 1.2 `CatalogResolver` keeps its inputs and rebuilds the catalog with `reload()`; `catalog()` returns `Arc<Catalog>`; covered end to end by the API test (models appear after login and disappear after logout without a restart).

## 2. Server API

- [x] 2.1 `Services` login hooks (start, status, code, cancel, logout) with a `503` default; routes under `/api/v1/providers/{provider}/login`; OpenAPI `v1.provider.*`; `ProviderLoginUnsupportedError` and `ProviderLoginNotFoundError`.
- [x] 2.2 `cyber-app` attempt registry: one pending attempt, 5-minute expiry, the callback listener (busy port gives `callback: false`), the state check, catalog reload on complete and logout, Windows refusal.
- [x] 2.3 Tests against a fake issuer and backend: pasted code completes and lists `chatgpt` models without a restart; forged state is refused; cancel; logout removes the provider; other providers give 400.
- [x] 2.4 Regenerate `openapi.json` and the TypeScript SDK.

## 3. TUI

- [x] 3.1 `/login`, `/login openai|chatgpt`, `/login google|gemini` refusal, `/logout openai`; built-in command entries.
- [x] 3.2 Login dialog: URL, waiting or paste hint, paste input, Enter and Esc, polling once a second, success toast and model picker, failure toast.
- [x] 3.3 TUI tests for dispatch, the dialog keys and the attempt outcomes.

## 4. Docs and verification

- [x] 4.1 README and `docs/local-core-guide.md`.
- [x] 4.2 `cargo fmt`, strict clippy, touched-crate tests, `openspec validate --all --strict`, spec inventory.
- [ ] 4.3 Live: `/login openai` in the TUI with a real plan, then a Turn on a `chatgpt/*` model without restarting the server.
