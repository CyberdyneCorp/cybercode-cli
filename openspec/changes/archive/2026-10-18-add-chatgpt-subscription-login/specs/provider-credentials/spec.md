## ADDED Requirements

### Requirement: ChatGPT subscription login
(P1) The system SHALL provide `cyber providers login chatgpt [--method browser|code]`. It SHALL sign the user in with their ChatGPT plan through OpenAI's OAuth authorization-code flow:

- **Client and redirect:** issuer `https://auth.openai.com`, Codex public client `app_EMoamEEZ73f0CkXaXp7hrann`, PKCE S256, an unguessable `state`, scope `openid profile email offline_access`, redirect URI `http://localhost:1455/auth/callback`, and `originator=cyber`.
- **`browser` (default):** the CLI SHALL print the authorization URL to stderr and wait up to 5 minutes for the callback on `127.0.0.1:1455`.
- **`code`:** the CLI SHALL read the redirect URL the user pastes from the browser, and SHALL refuse it when its `state` is missing or differs.
- **What is stored:** after the code exchange the system SHALL store the access, refresh and ID tokens with the ChatGPT account ID, email, plan and expiry in the owner-only file `<data>/chatgpt-auth.json` (mode 0600, replaced atomically). A file accessible to other users or not a regular file SHALL be refused.
- **What is printed:** the CLI SHALL print `Signed in to ChatGPT as <email> (<plan>)` and the plan's models. If the model list cannot be fetched, the CLI SHALL keep the login and warn that signing in again retries.
- **Account check:** a login whose ID token has no ChatGPT account ID SHALL fail without storing anything.

`cyber providers logout chatgpt` SHALL delete the file and the model list cache without revoking the tokens remotely. Login and logout for any other provider SHALL exit 2 and name the provider's API-key environment variables. Login SHALL refuse on Windows until keyring storage exists.

#### Scenario: Headless login
- **WHEN** the user runs `cyber providers login chatgpt --method code` and pastes `http://localhost:1455/auth/callback?code=c&state=<state>`
- **THEN** the code is exchanged, `<data>/chatgpt-auth.json` is written with mode 0600, and the CLI prints the signed-in email and plan

#### Scenario: Forged state refused
- **WHEN** the pasted redirect URL carries a `state` different from the one in the printed authorization URL
- **THEN** the login fails, no token request is sent and nothing is stored

#### Scenario: Other provider
- **WHEN** the user runs `cyber providers login anthropic`
- **THEN** the CLI exits 2 and says to set `ANTHROPIC_API_KEY`

### Requirement: ChatGPT token refresh
(P1) Before each request the `chatgpt` provider SHALL re-read the stored login. When the access token expires within 5 minutes (its JWT `exp`, or the token response's `expires_in`), the system SHALL:

- take the cross-process lock `<data>/chatgpt-auth.json.refresh.lock`, waiting at most 60 seconds,
- re-read the login and use it unchanged if another holder already refreshed it,
- otherwise send one refresh request,
- persist the new tokens before releasing the lock and before using them.

A refresh rejected with HTTP 400 or 401 SHALL fail the request with an `Authentication` error that says `ChatGPT login expired or was revoked; run cyber providers login chatgpt`, and SHALL leave the stored login untouched.

#### Scenario: Two processes refresh concurrently
- **WHEN** two processes each send a request while the stored access token expires in 1 minute
- **THEN** exactly one refresh request reaches the token endpoint and both requests use the persisted new access token

#### Scenario: Revoked refresh token
- **WHEN** the token endpoint answers the refresh with 400 `invalid_grant`
- **THEN** the Turn fails with the `cyber providers login chatgpt` hint and `<data>/chatgpt-auth.json` is unchanged
