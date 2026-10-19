## Context

OpenAI's Codex signs in with ChatGPT through an OAuth authorization-code + PKCE flow against `https://auth.openai.com`, using the public client `app_EMoamEEZ73f0CkXaXp7hrann` and the redirect URI `http://localhost:1455/auth/callback`. Requests then go to `https://chatgpt.com/backend-api/codex/responses`, which takes the Responses API body. Usage counts against the user's ChatGPT plan, not API billing. Third-party harnesses (OpenCode, Pi) reuse this flow. OpenAI has said publicly that this use is acceptable, but that statement is not a contract, and the backend is not a documented public API.

Cyber Code today resolves credentials synchronously from config, `CYBER_AUTH_CONTENT` and environment variables (`cyber-llm/src/catalog/credentials.rs`). An `Endpoint` carries a static `api_key`. Nothing renews a credential.

## Goals / Non-Goals

Goals: a working `cyber providers login|logout chatgpt`, a `chatgpt` provider that the catalog, TUI, exec and workflows use like any other, and refresh that is safe when the TUI, the server and workflows share one login.

Non-goals: listed in the proposal (keyring, generic provider connections, partner client, device code, Windows).

## Decisions

### Where the code lives

All runtime pieces live in `cyber-llm::chatgpt`, because the adapter that refreshes tokens runs there and `cyber-llm` must not depend on `cyber-tools`:

- OAuth requests: authorize URL, code exchange, refresh.
- Reading claims out of the ID and access tokens.
- The token file and its refresh lock.

The interactive part lives in `cyber-cli` and reuses `cyber-tools`' PKCE generator, `state` generator and loopback `CallbackListener`. The listener gains a constructor that takes the callback path (`/auth/callback`); MCP keeps `/mcp/oauth/callback`.

Rejected: reusing `McpCredentialStore`/`TokenSource`. Its keys are MCP-specific (server name + canonical URL + issuer) and its refresh sends an RFC 8707 `resource` parameter that OpenAI's token endpoint does not expect. Generalizing it belongs to the deferred keyring work, not to this change.

### Token storage

The login is stored in one owner-only document, `<data>/chatgpt-auth.json`, written with `cyber_core::trust::write_private_atomic` (mode 0600, atomic replace). A file that is accessible to other users, not regular, or larger than 64 KiB is refused, not trusted. The document holds:

- `version`
- `access_token`, `refresh_token`, `id_token`
- `account_id`, `email`, `plan`
- `expires_at` (Unix seconds)
- `updated_at`

`Debug` never prints a token.

This matches what Codex does by default (`~/.codex/auth.json`) and what the repo's MCP file backend does on Linux. The keyring backend remains the target from `provider-credentials` → "Credential storage". Windows refuses login, because the file backend cannot be made owner-only there, which is the same rule as the MCP file backend.

### Expiry and refresh

- `expires_at` is the `exp` claim of the access token (a JWT). If that is missing, it is `expires_in` from the token response. If neither exists, the token never counts as due.
- A token is due within 5 minutes of `expires_at`.
- Before each request the adapter re-reads the file on the blocking pool, so it picks up a login or refresh made by another process. If the token is due, the adapter:
  1. Takes the exclusive lock `<data>/chatgpt-auth.json.refresh.lock`, retrying for up to 60 s.
  2. Re-reads the file. If another holder already refreshed and the token is no longer due, it uses that token.
  3. Otherwise it POSTs `{client_id, grant_type: "refresh_token", refresh_token, scope: "openid profile email"}` as JSON to `/oauth/token`.
  4. It writes the new tokens (absent fields keep their old values) before releasing the lock and before using them.

  This satisfies "Token refresh" in `provider-credentials` for this credential.
- If refresh fails with 400 or 401, the result is an `Authentication` `LlmError` with the message `ChatGPT login expired or was revoked; run cyber providers login chatgpt`. The stored file is left as it is, so a concurrent login is never overwritten.

### Login flow

`cyber providers login chatgpt [--method browser|code]`:

1. Generates PKCE S256 and `state`.
2. Builds the authorize URL `https://auth.openai.com/oauth/authorize` with:
   - `response_type=code`, `client_id`, `redirect_uri=http://localhost:1455/auth/callback`
   - `scope=openid profile email offline_access`
   - `code_challenge`, `code_challenge_method=S256`
   - `id_token_add_organizations=true`, `codex_cli_simplified_flow=true`
   - `state`, `originator=cyber`
3. `browser` (default): binds `127.0.0.1:1455`, prints the URL to stderr (like `cyber mcp auth`, it never opens a browser itself), and waits up to 5 minutes for the callback. If the port is busy, it fails with a hint to close the other login (Codex uses the same port) or to use `--method code`.
4. `code`: prints the URL and reads one line from stdin, which is the full redirect URL from the browser's address bar. The `state` in that URL must match. A missing or mismatched `state` fails.
5. Exchanges the code with a form POST to `/oauth/token`: `grant_type=authorization_code`, `code`, `redirect_uri`, `client_id`, `code_verifier`.
6. Reads `https://api.openai.com/auth.chatgpt_account_id`, `chatgpt_plan_type` and `email` from the ID token. The token came straight from the token endpoint over TLS, so the signature is not checked. A login without an account ID fails with `This OpenAI account has no ChatGPT workspace`.
7. Stores the result and prints `Signed in to ChatGPT as <email> (<plan>)`. With `--format json` it prints `{provider, email, plan, expires_at}`.

`cyber providers logout chatgpt` removes the file and reports `{provider, removed}`. Tokens are not revoked remotely. Any provider other than `chatgpt` exits 2 with `Provider login is not available for <id> yet; set its API key environment variable`.

### Catalog

- When `<data>/chatgpt-auth.json` exists, `Catalog::build` inserts provider `chatgpt` (name `ChatGPT (subscription)`, kind `ApiKind::ChatgptCodex`, url `https://chatgpt.com/backend-api/codex`) before `providers` config is applied. Configuration can therefore override or add models.
- The data directory is found from the same `EnvSource` (`Paths::resolve` with `home_dir`). Without a home directory, no login is looked up.
- Its models are the ones the backend lists for the account, from `GET <backend>/models?client_version=1.0.0`. Only entries with `visibility: "list"` and a simple slug (`[A-Za-z0-9._-]`) are kept. Each becomes a model with the backend's display name and context window, one reasoning variant per supported effort, vision when `input_modalities` has `image`, and `cost = None` (the plan pays, so it reports as `unpriced`).
  - *Why not mirror `openai`:* the first live test showed the plan serves a different set than the API catalog. Mirroring `openai` `gpt-5*` listed `gpt-5.1`, which the backend rejects ("not supported when using Codex with a ChatGPT account"), and left out every `gpt-6*` model the plan offers.
  - *Client version:* the backend filters the list by the Codex client version it is told. Cyber Code sends `1.0.0` so it gets every model current clients may use. This is a constant in `chatgpt/models.rs`.
  - *Caching:* the list is cached in `<data>/chatgpt-models.json`, so the synchronous `Catalog::build` never waits on the network. Login fetches it. A request refreshes it when it is missing or older than 24 hours. A failed fetch keeps the old list, never fails the request, and stamps the cache so the next attempt waits a day. Logout deletes it.
- `CYBER_CHATGPT_BACKEND` overrides the backend URL, with the same HTTPS-or-loopback rule as the issuer.
- The credential source is `login`. The provider is never inserted without a stored login, so catalog listings for users who never signed in do not change.
- `ApiKind::ChatgptCodex` (`chatgpt-codex`) is not accepted by `ApiKind::parse` or `from_npm`, so configuration cannot point the token at another provider.

### Adapter

`ChatGptAdapter` wraps the Responses protocol. For each request it:

- obtains a fresh access token (see "Expiry and refresh"),
- builds an `Endpoint` with that token and the headers `chatgpt-account-id: <account>`, `OpenAI-Beta: responses=experimental` and `originator: cyber`,
- clears `max_output_tokens` and ensures `instructions` is present (an empty string when there is no system prompt),
- delegates to `OpenAiResponsesAdapter`, keeping the `store: false` it already sends.

It implements `with_http_client`, so refresh and requests both go through the transport the runtime owns. Overrides for tests and proxies:

- `CYBER_CHATGPT_ISSUER` changes the OAuth issuer.
- `CYBER_CHATGPT_BACKEND` (or `providers.chatgpt.api.url` for requests) changes the backend URL.
- The issuer must be HTTPS, or HTTP only on loopback.

### Client identity

Cyber Code identifies itself with `originator: cyber` and its own user agent. It never presents itself as Codex. The OAuth client ID is Codex's, because OpenAI offers no self-serve client for this flow. That is the accepted risk below.

## Risks / Trade-offs

- [OpenAI changes policy or the backend] → The provider is isolated: logout removes it and the API-key `openai` provider is unaffected. The endpoint, client ID and issuer are constants in one module, so moving to a partner client is a one-file change.
- [The backend rejects non-Codex `instructions`] → Some older third-party reports said the backend validated `instructions`. Live verification (task 5.2) checks this. If it fails, the fallback is to send the system prompt as a leading `developer` input item and set `instructions` to the published Codex prompt. That fallback would be a spec change.
- [Port 1455 is busy because Codex is mid-login] → Clear error, and `--method code` works without the port.
- [A refresh token is used twice across processes] → Prevented by the refresh lock and the re-read under the lock.
- [Plaintext token file on Linux and macOS] → It is mode 0600 and refused if loosened. This is the same exposure as Codex's default, and the keyring backend is the planned remedy.
