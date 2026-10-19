## Why

Today the only way to reach OpenAI models is an API key, which bills per token. Many users already pay for ChatGPT Plus or Pro, and OpenAI accepts that plan for coding harnesses other than Codex. OpenCode and Pi already sign users in with ChatGPT, and since DevDay 2026 "Sign in with ChatGPT" plan usage is an official program for partner tools. `provider-credentials` already requires OAuth credentials, `cyber providers login` and token refresh, but none of it is implemented. This change delivers the first OAuth provider credential: a ChatGPT subscription login.

## What Changes

- `provider-credentials`, new requirement "ChatGPT subscription login" (P1):
  - `cyber providers login chatgpt` signs in through OpenAI's OAuth (PKCE, loopback callback on `localhost:1455`), using the Codex public client like other third-party harnesses. `--method code` covers headless machines: the user pastes the URL the browser was redirected to.
  - `cyber providers logout chatgpt` forgets the login.
  - Tokens are stored in an owner-only file.
  - The access token is refreshed within 5 minutes of expiry, serialized across processes with a file lock, and the new tokens are persisted before use.
- `provider-catalog`, new requirement "ChatGPT subscription provider" (P1):
  - With a stored login, the catalog gains a `chatgpt` provider. Its models are the ones the backend lists for the account. The list is cached at login and refreshed daily, the models are unpriced, and they can be overridden under `providers.chatgpt`.
  - Requests use a dedicated adapter that speaks the Responses protocol to `https://chatgpt.com/backend-api/codex`. It sends the ChatGPT account header and leaves out `max_output_tokens`, which that backend rejects.

## Non-goals

- OS keyring storage, `cyber providers list|use`, labels, multiple connections per provider, and the credential HTTP API (`/api/v1/credentials`, `/connect/oauth`). These remain deferred with the rest of M0.5 provider connections.
- Applying to OpenAI's DevDay 2026 partner program with Cyber Code's own OAuth client. When a client is issued, the client ID and redirect URI change; nothing else does.
- Device-code login, `credential.expired`/`credential.updated` events, and remote token revocation on logout.
- Windows support. Login refuses until the keyring backend lands, because the owner-only file cannot be enforced there.
- Any use of the Cyber Account or of `~/.codex/auth.json`. Codex rotates its refresh token, so sharing that file would sign one of the two tools out.

## Impact

- `crates/cyber-llm`: new `chatgpt` module (OAuth requests, JWT claims, token file, refresh lock), new `chatgpt` adapter and `ApiKind::ChatgptCodex`, and the catalog inserts the provider when a login is stored.
- `crates/cyber-tools`: the loopback `CallbackListener` takes a callback path, so it can be reused.
- `crates/cyber-cli`: new `cyber providers login|logout`.
- Docs: `docs/` provider setup and the `ROADMAP.md` M0.2 note.
- No change to the database schema, API shape or config schema.
