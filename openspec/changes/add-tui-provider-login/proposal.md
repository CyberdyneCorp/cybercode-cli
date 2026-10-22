## Why

Signing in to a ChatGPT subscription is only possible with `cyber providers login chatgpt`, run in a separate terminal. A server that is already running does not see the new login until it restarts, because it builds its model catalog once at startup. TUI users expect `/login openai` to sign in without leaving the TUI and to use the plan's models right away.

## What Changes

- `provider-credentials`, new requirement "Provider login API" (P1). The server drives the same ChatGPT OAuth flow for clients:
  - `POST /api/v1/providers/{provider}/login` starts an attempt and returns `{ attempt_id, authorization_url, callback }`. The server waits on `localhost:1455` when the port is free.
  - `GET /api/v1/providers/{provider}/login/{attempt_id}` returns `pending`, `complete` (with email, plan and models) or `failed` (with the error).
  - `POST …/login/{attempt_id}/code` completes an attempt with the redirect URL the user pastes. This is the headless path.
  - `DELETE …/login/{attempt_id}` cancels the attempt, and `DELETE /api/v1/providers/{provider}/login` signs out.
  - Raw tokens are never returned. Only `chatgpt` supports login; other providers get `400 ProviderLoginUnsupportedError` naming their API-key variables.
- `provider-catalog`, modified "ChatGPT subscription provider": a login or logout through the API rebuilds the server's catalog in place, so `chatgpt` models appear or disappear without a restart. Turns already running keep their resolved model.
- `tui`, new requirement "Provider login commands" (P1):
  - `/login` lists the subscriptions that can sign in.
  - `/login openai` (alias `chatgpt`) opens a dialog that shows the authorization URL, waits for the browser, accepts a pasted redirect URL and cancels on Esc. On success it opens the model picker.
  - `/logout openai` signs out.
  - `/login google` (alias `gemini`) is refused: Google subscription login is not supported.
- The CLI login and the API share one implementation of the code exchange and the model fetch.

## Non-goals

- **Google (Gemini CLI / Code Assist) subscription login.** It stays out of scope for the reasons in `add-copilot-subscription-login`: automated account suspensions were reported for third-party use, and Google publishes no policy allowing it.
- New providers, keyring storage, Windows support (still refused), and API-key entry from the TUI.

## Impact

- `crates/cyber-llm`: `chatgpt::complete_login` (code exchange, save and model cache), shared by the CLI and the server.
- `crates/cyber-server`: provider login routes and their OpenAPI operations (`v1.provider.*`), `Services` login hooks, and a reloadable `CatalogResolver`.
- `crates/cyber-app`: the attempt registry and callback listener.
- `crates/cyber-tui`: `/login` and `/logout`, the login dialog and polling.
- `crates/cyber-core`: built-in command entries. Generated: `openapi.json` and the TypeScript SDK.
- Docs: `README.md` and `docs/local-core-guide.md`.
