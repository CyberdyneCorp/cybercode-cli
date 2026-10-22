# provider-credentials Specification

## Purpose
Provider credentials are the secrets Cyber Code uses to call LLM providers: API keys, environment variables, provider OAuth subscriptions, and cloud IAM chains. The design follows OpenCode's integration and connection model and Codex/Claude Code's keyring storage. These credentials are separate from the Cyber Account (`cyber-account`): the account is an identity used for networked features, while provider credentials pay for and authorize model calls.

## Requirements

### Requirement: Credential kinds
(P0) The system SHALL support the credential kinds `api_key`, `env` (one or more named environment variables), `oauth` (access token, refresh token, expiry, account metadata), and `cloud_iam` (AWS credential chain, Google Application Default Credentials, Azure Entra ID via Azure CLI or managed identity).

#### Scenario: Bedrock via AWS profile
- **WHEN** `AWS_PROFILE=dev` is set and the `amazon-bedrock` provider has no API key
- **THEN** Bedrock requests are signed using the AWS credential chain for profile `dev`

### Requirement: Separation from the Cyber Account
(P0) The system SHALL NOT use the Cyber Account access token as an LLM provider credential, except for the optional hosted `cyber-gateway` provider, which SHALL be listed as a normal provider and SHALL authenticate with the account token only when the user selects it.

#### Scenario: Logged in but no provider keys
- **WHEN** a user is logged into a Cyber Account but has no provider credentials and has not selected `cyber-gateway`
- **THEN** all non-gateway providers remain unavailable with reason `missing_credentials`

### Requirement: Secure storage
(P0) The system SHALL store credentials in the OS keyring (macOS Keychain, Windows Credential Manager, Secret Service on Linux) under service `cyber` and account `provider:<provider_id>:<credential_id>`. When no keyring is available it SHALL fall back to `~/.local/share/cyber/credentials.json` written atomically with mode 0600, and SHALL warn once that the fallback is in use.

#### Scenario: Headless Linux without Secret Service
- **WHEN** no Secret Service is reachable
- **THEN** the credential is written to `credentials.json` with mode 0600 and a one-time warning is printed

### Requirement: Credential precedence
(P0) The system SHALL resolve a provider's credential in this order, first match wins: explicit `providers.<id>.api.settings.api_key` in config (supporting `{env:VAR}` and `{file:path}` placeholders), the active stored connection (most recently added unless one is marked active), environment variables declared for the provider, and the cloud IAM chain.

#### Scenario: Stored key beats environment
- **WHEN** both `OPENAI_API_KEY` and a stored OpenAI key exist and config sets no key
- **THEN** the stored key is used

### Requirement: Multiple connections per provider
(P1) The system SHALL allow several labeled connections per provider, list them newest first with the active one marked, and let the user switch the active connection with `cyber providers use <provider> <label>` or `PATCH /api/v1/credentials/{id}`.

#### Scenario: Switch between work and personal keys
- **WHEN** OpenAI has connections `work` and `personal` and the user runs `cyber providers use openai personal`
- **THEN** subsequent OpenAI requests use the `personal` key

### Requirement: Login command
(P0) The system SHALL provide `cyber providers login [provider] [--method <label>] [--label <name>]` that offers the provider's methods (API key with masked input, OAuth `auto` browser flow, OAuth `code` paste flow, device code where offered), validates a new API key with a lightweight models-list request when the adapter supports one, and stores the result.

#### Scenario: Invalid key rejected
- **WHEN** the user enters an API key and the validation request returns 401
- **THEN** the key is not stored and the command prints `Credential rejected by <provider>`

### Requirement: Provider OAuth flows
(P1) The system SHALL support provider OAuth methods supplied by built-in adapters or plugins with PKCE, a loopback callback on `127.0.0.1` with an ephemeral port, a 10-minute attempt lifetime, and a `code` mode for headless machines. Terminal attempts SHALL be retained for 1 minute and then discarded.

#### Scenario: Headless OAuth
- **WHEN** the user runs login with `--method code` over SSH
- **THEN** the CLI prints the authorization URL and waits for the pasted code instead of opening a browser

### Requirement: Token refresh
(P0) The system SHALL refresh an OAuth credential when it expires within 5 minutes, serialize refreshes per credential across processes with a file lock, persist the new tokens atomically before use, and mark the credential `expired` and publish `credential.expired` when refresh fails with an auth error.

#### Scenario: Two processes refresh concurrently
- **WHEN** the TUI and a background workflow both need a refreshed token
- **THEN** exactly one refresh request is sent and both use the persisted result

### Requirement: Logout and listing
(P0) The system SHALL provide `cyber providers list [--json]` (stored connections with provider name, label, kind, and status, plus detected environment variables) and `cyber providers logout <provider> [--label <name>]`, which deletes the credential from storage and publishes `catalog.updated`.

#### Scenario: Logout removes availability
- **WHEN** the only Anthropic credential is logged out
- **THEN** Anthropic models become unavailable and `catalog.updated` is published

### Requirement: Credential HTTP API
(P0) The server SHALL expose `GET /api/v1/credentials` (metadata only, never secrets), `POST /api/v1/providers/{id}/connect/key`, `POST /api/v1/providers/{id}/connect/oauth`, `GET|DELETE /api/v1/oauth-attempts/{id}`, `POST /api/v1/oauth-attempts/{id}/complete`, `PATCH /api/v1/credentials/{id}`, and `DELETE /api/v1/credentials/{id}`.

#### Scenario: Secrets never returned
- **WHEN** a client calls `GET /api/v1/credentials`
- **THEN** each entry contains `id`, `provider_id`, `label`, `kind`, `status`, and `created_at` but no key or token material

### Requirement: Redaction
(P0) The system SHALL redact credential values from logs, error messages, `cyber debug config`, exported sessions, telemetry, and tool output shown to the model, replacing them with `***`, including values found under keys matching `key|secret|token|password|authorization|cookie|credential`.

#### Scenario: Debug config output
- **WHEN** the user runs `cyber debug config` with a key placed inline in config
- **THEN** the printed value is `***`

### Requirement: Environment injection for CI
(P0) The system SHALL accept `CYBER_AUTH_CONTENT` holding a JSON credential map that replaces stored credentials for the process without writing them to disk, ignoring the variable with a warning when the JSON is invalid.

#### Scenario: CI run with injected credentials
- **WHEN** a CI job sets `CYBER_AUTH_CONTENT='{"anthropic":{"kind":"api_key","key":"..."}}'`
- **THEN** Anthropic is available for that process and nothing is written to the keyring

### Requirement: Org-policy provider restrictions
(P3) The system SHALL hide and refuse providers denied by the active org policy (`provider.use` statements), report them as unavailable with reason `denied_by_policy`, and refuse to store new credentials for them.

#### Scenario: Org denies a provider
- **WHEN** org policy denies `provider.use` for `openrouter`
- **THEN** `cyber providers login openrouter` fails with `Provider openrouter is not allowed by your organization`

### Requirement: Cyber Gateway provider
(P3) The system SHALL offer an optional `cyber-gateway` provider that routes model calls through a Cyber Cloud or self-hosted gateway using the Cyber Account access token as bearer, available only when the account holds an entitlement starting with `cyber-code` that includes gateway access, and SHALL report usage and spend limits returned by the gateway.

#### Scenario: Gateway without entitlement
- **WHEN** a logged-in user without a gateway entitlement selects `cyber-gateway`
- **THEN** the provider is unavailable with reason `missing_entitlement`

### Requirement: Credential change events
(P0) The system SHALL publish `credential.updated` when a credential is added, relabeled, activated, refreshed, or removed, and SHALL rebuild the catalog afterwards.

#### Scenario: Activating a connection
- **WHEN** the user marks a different OpenAI connection active
- **THEN** `credential.updated` and `catalog.updated` are both published

### Requirement: Environment variable detection
(P0) The system SHALL detect provider environment variables declared by the catalog (for example `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`) and show them in `cyber providers list` under an `Environment` section without printing their values.

#### Scenario: Env key listed
- **WHEN** `GEMINI_API_KEY` is set
- **THEN** `cyber providers list` shows `google  GEMINI_API_KEY` under `Environment`

### Requirement: Command credentials
(P1) A credential of kind `command` (`providers.<id>.api.settings.api_key_command: { command, refresh_seconds? (default 3600), format?: "text" | "json" }`, or stored through `cyber providers login <id> --method command`) SHALL run the command outside the sandbox with the server's environment, and use its trimmed stdout as the API key (`text`) or parse `{ api_key, headers?, expires_at? }` (`json`). The result SHALL be cached until `expires_at` or `refresh_seconds`, never written to disk or logs, and a non-zero exit SHALL mark the provider unavailable with reason `credential_command_failed` and the command's stderr in `cyber doctor`. Project-defined commands SHALL require workspace-trust approval.

#### Scenario: Rotating gateway key
- **WHEN** `providers.corp.api.settings.api_key_command` is `{ command: "vault read -field=key llm/corp", refresh_seconds: 900 }`
- **THEN** requests use the command's output as the key and the command is re-run after 15 minutes

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

### Requirement: Provider login API
(P1) The server SHALL let clients drive the ChatGPT subscription login (same client, PKCE, scope, redirect URI and storage as "ChatGPT subscription login") through these routes of the `provider` group:

- `POST /api/v1/providers/{provider}/login` SHALL start an attempt and return `201` with `{ attempt_id, provider, status: "pending", authorization_url, callback, expires_at }`. `callback` SHALL be true when the server is listening for the redirect on `127.0.0.1:1455`, and false when the port is busy. Starting an attempt SHALL cancel any pending one. An attempt SHALL expire 5 minutes after it starts.
- `GET /api/v1/providers/{provider}/login/{attempt_id}` SHALL return the attempt, with `status` `pending`, `complete` (with `email`, `plan`, `models` and an optional `warning`) or `failed` (with `error`). A finished attempt SHALL remain readable for 10 minutes; after that, or for an unknown ID, the server SHALL answer `404 ProviderLoginNotFoundError`.
- `POST /api/v1/providers/{provider}/login/{attempt_id}/code` with `{ redirect_url }` SHALL complete a pending attempt with a redirect URL pasted by the user. It SHALL refuse a URL whose `state` is missing or differs with `400 InvalidRequestError`, send no token request, and leave the attempt pending.
- `DELETE /api/v1/providers/{provider}/login/{attempt_id}` SHALL cancel a pending attempt and stop its listener.
- `DELETE /api/v1/providers/{provider}/login` SHALL sign out like `cyber providers logout`, returning `{ removed }`.

The PKCE verifier, `state` and tokens SHALL never be returned by the API. Every route for a provider other than `chatgpt` SHALL answer `400 ProviderLoginUnsupportedError`, naming the provider's API-key environment variables. Starting an attempt SHALL be refused on Windows, as the CLI login is.

#### Scenario: Pasted redirect completes the attempt
- **WHEN** a client starts an attempt and posts `http://localhost:1455/auth/callback?code=c&state=<state of the attempt>` to its `code` route
- **THEN** the code is exchanged, `<data>/chatgpt-auth.json` is written with mode 0600, and the attempt reads `complete` with the account email, plan and models

#### Scenario: Forged state through the API
- **WHEN** the posted redirect URL carries a different `state`
- **THEN** the server answers 400, sends no token request, stores nothing and the attempt stays `pending`

#### Scenario: Busy callback port
- **WHEN** another process listens on port 1455 and a client starts an attempt
- **THEN** the attempt starts with `callback: false` and can still complete through the `code` route

#### Scenario: Unsupported provider
- **WHEN** a client calls `POST /api/v1/providers/google/login`
- **THEN** the server answers `400 ProviderLoginUnsupportedError` and starts nothing

### Requirement: GitHub Copilot subscription login
(P1) The system SHALL provide `cyber providers login copilot [--enterprise <host>]`. It SHALL sign the user in with GitHub's OAuth device flow:

- **Device flow:**
  - The flow runs on `github.com`, or on the bare hostname given with `--enterprise`.
  - The CLI SHALL print the verification URL and the user code, then poll at the server's `interval`, adding 5 seconds on `slow_down`.
  - Polling SHALL stop at the code's expiry (at most 15 minutes), on `access_denied` or `expired_token`, or on Ctrl+C. In every one of those cases nothing SHALL be stored.
- **Subscription check:** after the device flow, the system SHALL mint a Copilot API token from the GitHub token. If minting fails, or names no API endpoint, the login SHALL fail with `This GitHub account has no active Copilot subscription` and SHALL store nothing.
- **Storage:** the GitHub token, host and user SHALL be stored in the owner-only file `<data>/copilot-auth.json` (mode 0600, replaced atomically; refused when accessible to other users), and SHALL be unreadable by every sandboxed process. Minted Copilot API tokens SHALL be kept only in memory and reused until 5 minutes before they expire.
- **Revoked login:** a mint rejected with HTTP 401 SHALL fail the request with an `Authentication` error that says `GitHub Copilot login expired or was revoked; run cyber providers login copilot`, and SHALL leave the stored login untouched.
- **Logout:** `cyber providers logout copilot` SHALL delete the login and the cached model list without revoking the GitHub token.
- **Windows:** login SHALL refuse on Windows until keyring storage exists.

#### Scenario: Device login
- **WHEN** the user runs `cyber providers login copilot`, opens the printed URL, enters the printed code and approves
- **THEN** `<data>/copilot-auth.json` is written with mode 0600 and the CLI prints the GitHub user and the plan's models

#### Scenario: Slow down honored
- **WHEN** the token endpoint answers a poll with `slow_down`
- **THEN** the next poll is sent at least `interval + 5` seconds later

#### Scenario: No Copilot plan
- **WHEN** the device flow succeeds but the Copilot token request answers 403
- **THEN** the login fails with `This GitHub account has no active Copilot subscription` and nothing is stored
