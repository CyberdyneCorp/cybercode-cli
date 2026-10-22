## ADDED Requirements

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
