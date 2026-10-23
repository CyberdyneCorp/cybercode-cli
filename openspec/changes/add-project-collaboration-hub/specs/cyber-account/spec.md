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
