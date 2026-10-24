## MODIFIED Requirements

### Requirement: Requested scopes
(P3) Login SHALL request `openid profile email offline_access cyber:relay cyber:runner cyber:share cyber:messaging cyber:collaborate`, minus any scope missing from the issuer's `scopes_supported` (a warning names the omitted scopes). Each account-gated feature SHALL check that its required scope is present in the token's `scope` claim and SHALL report `MissingScopeError` with a hint to run `cyber login` again when it is not.

#### Scenario: Scope not supported by a self-hosted issuer
- **WHEN** the issuer's `scopes_supported` lacks `cyber:runner`
- **THEN** login proceeds without it, warns `issuer does not support scope cyber:runner; hosted runners will be unavailable`, and `cyber --cloud` later fails with `MissingScopeError`


### Requirement: ID token and access token validation
(P3) The CLI SHALL validate the ID token's signature against `jwks_uri` using the algorithm advertised there, plus `iss`, `aud` (= client_id), `exp`, `iat` (60 seconds of clock skew allowed) and `nonce`. Services that receive access tokens (Relay, Runners, Share, Routines, collaboration hub) SHALL validate signature, `iss` and `exp`. They SHALL check `aud` when the client has an access-token audience configured, and the required scope. The collaboration hub SHALL always require its configured access-token audience and `cyber:collaborate`; an ID token or access token without that audience SHALL NOT authorize project access. For immediate revocation checks they SHALL use RFC 7662 introspection with a service token, cached for at most 60 seconds.

#### Scenario: Unknown key ID triggers a JWKS refresh
- **WHEN** an ID token carries a `kid` absent from the cached JWKS
- **THEN** the JWKS is refetched once, and validation fails with `UnknownSigningKeyError` only if the key is still absent


#### Scenario: Missing collaboration audience or scope
- **WHEN** a caller presents an otherwise valid token without the hub's configured audience or `cyber:collaborate`
- **THEN** collaboration is refused without accepting a local password, peer grant or provider login as a fallback

### Requirement: Headless login
(P3) `cyber login --no-browser` SHALL print the authorization URL, which uses the loopback redirect, and SHALL accept either the pasted full redirect URL (from the browser address bar after the redirect fails to load on a remote machine) or a bare `code` together with the `state` copied from that same redirect. A bare code without state SHALL NOT bypass callback validation. It SHALL then validate `state` and complete the exchange. Except during explicit recent-authentication login (`cyber login --reauth` or local API `reauthenticate: true`), when the discovery document advertises `device_authorization_endpoint` and `urn:ietf:params:oauth:grant-type:device_code` in `grant_types_supported` (RFC 8628, enabled per issuer and permitted client), `--no-browser` SHALL instead use the device flow, printing the verification URI and user code.

#### Scenario: Pasted redirect URL over SSH
- **WHEN** a user on a remote host runs `cyber login --no-browser`, signs in on a laptop, and pastes `http://127.0.0.1:53682/callback?code=abc&state=xyz`
- **THEN** the CLI validates `state = xyz` and completes the login

#### Scenario: Device flow when available
- **WHEN** discovery advertises `device_authorization_endpoint`
- **THEN** `cyber login --no-browser` prints `Go to <verification_uri> and enter code ABCD-EFGH` and polls the token endpoint, honoring `interval` and `slow_down`


### Requirement: Recent authentication login
Login requested for recent authentication SHALL use PKCE with `prompt=login` and `max_age=900`, including headless clients when a device grant is advertised. The verified ID token SHALL contain `auth_time` no later than the current time and no older than 900 seconds. Missing or stale authentication time SHALL fail with `StepUpRequiredError`; refresh SHALL NOT renew authentication time.

#### Scenario: Issuer ignores authentication freshness
- **WHEN** recent-authentication login returns a signed ID token without a current `auth_time`
- **THEN** login fails and does not persist the returned credentials

### Requirement: Token storage
(P3) Tokens (access, refresh, ID token, expiry, issuer, subject) SHALL be stored in the OS keyring under service `cyber-code`. Account entries SHALL use a dedicated `account:` namespace bound to the canonical installation directory, exact issuer, client ID and subject, separate from peer, MCP and plugin credentials; metadata SHALL bind those identities before reading a credential. A durable cleanup journal SHALL precede native chunk writes so partial or interrupted commits remain removable; retired generations SHALL remain tracked until verified deletion. Bounded generation-based chunks MAY accommodate native keyring entry limits without storing raw tokens in the account index. When no keyring is available, tokens SHALL fall back to `<data>/account.json` with file mode 0600 and a warning on platforms with owner-only storage; otherwise storing credentials SHALL fail. Refresh, login and logout SHALL share a cross-process lock. A durable pending marker SHALL precede rotating refresh requests; uncertain outcomes or crashes SHALL require fresh login instead of reusing a possibly consumed token. Tokens SHALL never be written to logs, exports, telemetry or tool outputs.

#### Scenario: Keyring fallback
- **WHEN** login runs in a container without a supported OS keyring
- **THEN** tokens are written to `~/.local/share/cyber/account.json` with mode 0600, and `cyber doctor` warns `account tokens stored in file (no OS keyring)`

#### Scenario: Identity and credential namespaces stay separate
- **WHEN** two issuers share a hostname and subject, or an account resembles a peer credential name
- **THEN** their credential entries remain distinct and metadata cannot redirect tokens to another issuer or client

#### Scenario: Refresh outcome is unknown after a crash
- **WHEN** the process exits after recording a pending refresh but before saving its rotated result
- **THEN** subsequent account use requires fresh login and never presents the previous refresh token again
