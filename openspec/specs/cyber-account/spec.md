# cyber-account Specification

## Purpose
A Cyber Account is the user's identity for networked features: remote control through the Relay, cross-machine messaging, hosted Runners, session sharing, Routines and org-managed policy. Identity comes from CyberdyneAuth (CyberdyneCorp/CyberdyneAuth) as an OpenID Connect provider, using the authorization-code flow with PKCE for the public client `cyber-cli`. The Account is optional: everything that runs on one machine works without login. Login UX follows OpenCode's console login and Claude Code's claude.ai login. Token handling respects CyberdyneAuth's contract: discovery-driven issuer and keys, refresh rotation with replay defense, and entitlement, org and role claims.

## Requirements

### Requirement: Account is optional
(P3) The system SHALL provide every local capability (sessions, tools, workflows, goals, loops, local cross-session messaging, local server API, snapshots, MCP, plugins) without a Cyber Account. Only features listed in this spec as account-gated SHALL require login. Provider credentials (LLM API keys) SHALL be independent of the Cyber Account.

#### Scenario: Offline use
- **WHEN** a user who never ran `cyber login` runs a workflow on a local Ollama model with `CYBER_OFFLINE=1`
- **THEN** the workflow runs without any request to the issuer

### Requirement: Issuer configuration and discovery
(P3) The issuer SHALL come from `account.issuer` or `CYBER_ACCOUNT_ISSUER`, defaulting to the build-time Cyber Cloud issuer. The system SHALL fetch `{issuer}/.well-known/openid-configuration` and take `issuer`, `authorization_endpoint`, `token_endpoint`, `userinfo_endpoint`, `jwks_uri`, `end_session_endpoint`, `introspection_endpoint`, `scopes_supported` and `code_challenge_methods_supported` from it. It SHALL NOT hard-code endpoint paths, issuer strings, keys or algorithms. Discovery SHALL be cached for 24 hours in `<cache>/oidc-<hash>.json`, and refetched when token validation fails with an unknown `kid`.

#### Scenario: Self-hosted issuer
- **WHEN** `account.issuer = "https://auth.acme.internal"` and its discovery lists `token_endpoint = https://auth.acme.internal/api/v1/auth/oauth2/token`
- **THEN** login and refresh use that endpoint

#### Scenario: Issuer mismatch rejected
- **WHEN** the discovery document's `issuer` differs from the configured issuer after normalizing the trailing slash
- **THEN** login fails with `IssuerMismatchError` and no request is sent to the authorization endpoint

### Requirement: Public client registration contract
(P3) The CLI SHALL authenticate as the public OAuth client `account.client_id` (default `cyber-cli`) with no client secret. The client SHALL be registered in CyberdyneAuth with `client_type = "public"`, `grant_types = ["authorization_code", "refresh_token"]`, redirect URIs `http://127.0.0.1/callback` and `http://[::1]/callback` (any port accepted, as CyberdyneAuth allows for exact loopback authorities), and the private-use scheme `dev.cyber-code:/oauth/callback` for desktop and mobile apps.

#### Scenario: Loopback redirect with an ephemeral port
- **WHEN** login binds a callback listener on `127.0.0.1:53682`
- **THEN** the authorization request uses `redirect_uri=http://127.0.0.1:53682/callback`, which CyberdyneAuth accepts for the registered loopback client

### Requirement: Browser login with PKCE
(P3) `cyber login` SHALL:
1. generate a 64-byte random `code_verifier`, an S256 `code_challenge`, a 32-byte `state` and a `nonce`
2. bind a one-shot HTTP listener on `127.0.0.1` with an OS-assigned port
3. open `authorization_endpoint` with `response_type=code`, `client_id`, `redirect_uri`, `scope`, `state`, `nonce`, `code_challenge` and `code_challenge_method=S256`
4. wait up to 5 minutes for the callback
5. exchange the code at `token_endpoint` with the `code_verifier`

A callback with a missing or mismatched `state` SHALL be rejected as possible CSRF.

#### Scenario: Successful login
- **WHEN** the user completes the browser flow and the callback returns `code=abc&state=<matching>`
- **THEN** the CLI exchanges the code, stores the tokens, and prints `Logged in as ada@example.com (org: acme)`

#### Scenario: State mismatch
- **WHEN** the callback's `state` does not match
- **THEN** the listener responds with an error page, no token exchange happens, and login fails with `OAuthStateMismatchError`

### Requirement: Requested scopes
(P3) Login SHALL request `openid profile email offline_access cyber:relay cyber:runner cyber:share cyber:messaging`, minus any scope missing from the issuer's `scopes_supported` (a warning names the omitted scopes). Each account-gated feature SHALL check that its required scope is present in the token's `scope` claim and SHALL report `MissingScopeError` with a hint to run `cyber login` again when it is not.

#### Scenario: Scope not supported by a self-hosted issuer
- **WHEN** the issuer's `scopes_supported` lacks `cyber:runner`
- **THEN** login proceeds without it, warns `issuer does not support scope cyber:runner; hosted runners will be unavailable`, and `cyber --cloud` later fails with `MissingScopeError`

### Requirement: Headless login
(P3) `cyber login --no-browser` SHALL print the authorization URL, which uses the loopback redirect, and SHALL accept either the pasted full redirect URL (from the browser address bar after the redirect fails to load on a remote machine) or the bare `code` value. It SHALL then validate `state` and complete the exchange. When the discovery document advertises `device_authorization_endpoint` and `urn:ietf:params:oauth:grant-type:device_code` in `grant_types_supported` (RFC 8628, not offered by CyberdyneAuth today), `--no-browser` SHALL instead use the device flow, printing the verification URI and user code.

#### Scenario: Pasted redirect URL over SSH
- **WHEN** a user on a remote host runs `cyber login --no-browser`, signs in on a laptop, and pastes `http://127.0.0.1:53682/callback?code=abc&state=xyz`
- **THEN** the CLI validates `state = xyz` and completes the login

#### Scenario: Device flow when available
- **WHEN** discovery advertises `device_authorization_endpoint`
- **THEN** `cyber login --no-browser` prints `Go to <verification_uri> and enter code ABCD-EFGH` and polls the token endpoint, honoring `interval` and `slow_down`

### Requirement: Token storage
(P3) Tokens (access, refresh, ID token, expiry, issuer, subject) SHALL be stored in the OS keyring under service `cyber-code` and account `<issuer-host>:<sub>`. When no keyring is available (headless Linux without Secret Service, or containers), they SHALL fall back to `<data>/account.json` with file mode 0600 and a warning. Tokens SHALL never be written to logs, exports, telemetry or tool outputs.

#### Scenario: Keyring fallback
- **WHEN** login runs in a container without a Secret Service daemon
- **THEN** tokens are written to `~/.local/share/cyber/account.json` with mode 0600, and `cyber doctor` warns `account tokens stored in file (no OS keyring)`

### Requirement: Serialized refresh with rotation
(P3) Access tokens SHALL be refreshed when they expire within 60 seconds. Each refresh SHALL hold the cross-process lock `<state>/account.lock` for the whole read–refresh–persist cycle, re-read the stored tokens after acquiring it (another process may already have refreshed), and persist the rotated refresh token atomically before releasing the lock. Two processes SHALL never present the same refresh token, because CyberdyneAuth's replay defense revokes the whole chain on reuse.

#### Scenario: Concurrent refresh
- **WHEN** the server and a `cyber whoami` process both see an expiring token at the same moment
- **THEN** only one refresh request is sent, and the other process reads the new tokens after the lock is released

#### Scenario: Replay detected
- **WHEN** the token endpoint rejects a refresh with `invalid_grant` because the chain was revoked
- **THEN** stored tokens are deleted, account-gated features stop, and the user sees `Session expired — run "cyber login"`; local work continues

### Requirement: ID token and access token validation
(P3) The CLI SHALL validate the ID token's signature against `jwks_uri` using the algorithm advertised there, plus `iss`, `aud` (= client_id), `exp`, `iat` (60 seconds of clock skew allowed) and `nonce`. Services that receive access tokens (Relay, Runners, Share, Routines) SHALL validate signature, `iss` and `exp`. They SHALL check `aud` when the client has an access-token audience configured, and the required scope. For immediate revocation checks they SHALL use RFC 7662 introspection with a service token, cached for at most 60 seconds.

#### Scenario: Unknown key ID triggers a JWKS refresh
- **WHEN** an ID token carries a `kid` absent from the cached JWKS
- **THEN** the JWKS is refetched once, and validation fails with `UnknownSigningKeyError` only if the key is still absent

### Requirement: Identity claims model
(P3) The Account SHALL expose `sub`, `email`, `name`, `is_admin`, `entitlements`, `org`, `orgs`, `roles`, `scope`, `auth_time` and `amr` from the validated tokens. A missing `orgs` claim SHALL be treated as "no authorized orgs", never as "all orgs". `roles` SHALL be filtered to entries prefixed with `cyber-cli:` (or the configured client ID) when applied to Cyber Code authorization.

#### Scenario: Legacy token without orgs
- **WHEN** the access token has no `orgs` claim
- **THEN** org-scoped features (org policy, team Relay routing) treat the user as belonging to no org

### Requirement: Entitlement gating of hosted features
(P3) Hosted features SHALL check the `entitlements` claim for product key `cyber-code`:
- `cyber-code` or `cyber-code:free` → Relay remote control for 2 Devices, cross-machine messaging, and public share links
- `cyber-code:pro` → unlimited Devices, hosted Runners and Routines
- `cyber-code:team` → org policy, team Relay routing and shared Runners

Missing entitlements SHALL produce `EntitlementRequiredError` naming the required plan and the upgrade URL. Self-hosted services MAY disable entitlement checks with `services.require_entitlements = false`.

#### Scenario: Hosted runner on free plan
- **WHEN** a user whose `entitlements` is `["cyber-code:free"]` runs `cyber --cloud "task"`
- **THEN** the CLI fails with `EntitlementRequiredError: hosted runners require cyber-code:pro` and exit code 6

### Requirement: Organization selection
(P3) When `orgs` contains more than one org, the active org SHALL default to the `org` claim and be switchable with `cyber whoami --org <short_name>` (stored in `<state>/account-org.json`). The active org SHALL scope org policy, team Runners and Relay team routing. Switching to an org not in `orgs` SHALL fail.

#### Scenario: Switch org
- **WHEN** `orgs` contains `acme` and `beta-labs` and the user runs `cyber whoami --org beta-labs`
- **THEN** subsequent policy fetches use `beta-labs`

### Requirement: Logout
(P3) `cyber logout` SHALL delete the stored tokens for the active issuer and subject, close Relay connections, and best-effort call `end_session_endpoint` with the `id_token_hint`. `--all` SHALL remove all stored accounts. Logout SHALL NOT delete provider credentials or local data.

#### Scenario: Logout keeps local sessions
- **WHEN** the user logs out
- **THEN** all local Sessions remain accessible and `cyber whoami` prints `not logged in`

### Requirement: Account HTTP API
(P3) The local server SHALL expose:
- `GET /api/v1/account` → `{ logged_in, sub, email, org, orgs, entitlements, scopes, expires_at }`
- `POST /api/v1/account/login` → `{ authorization_url, attempt_id }`, so GUI clients can drive the flow
- `GET /api/v1/account/login/:attempt_id` → `{ status: pending|complete|failed }`
- `DELETE /api/v1/account`

Raw tokens SHALL never be returned by the local API.

#### Scenario: Desktop app login
- **WHEN** the desktop app calls `POST /api/v1/account/login`
- **THEN** it receives an authorization URL, opens it, and polls the attempt until `complete`

### Requirement: Step-up for sensitive actions
(P3) Pairing a new Device, registering a Runner and enabling org-wide routines SHALL require `auth_time` within the last 15 minutes. Otherwise the CLI SHALL run a fresh authorization with `prompt=login` and `max_age=900` before proceeding.

#### Scenario: Stale authentication on device pairing
- **WHEN** `auth_time` is 3 days old and the user runs `cyber remote pair`
- **THEN** the browser re-prompts for credentials (and TOTP when enrolled) before the pairing code is shown

### Requirement: Generic OIDC compatibility
(P3) The system SHOULD work with any OIDC provider that supports discovery, authorization code with PKCE S256 for public clients, loopback redirects, refresh tokens and JWT access tokens. When the provider's tokens lack `entitlements`, `orgs` or `roles`, entitlement and org features SHALL be disabled with a clear message rather than failing login.

#### Scenario: Keycloak issuer
- **WHEN** `account.issuer` points to a Keycloak realm with a public `cyber-cli` client
- **THEN** login succeeds, and `cyber whoami` shows `entitlements: (not provided by issuer)`
