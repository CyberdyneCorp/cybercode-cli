# Project collaboration implementation evidence

Approved design: [add-project-collaboration-hub](../../openspec/changes/add-project-collaboration-hub/README.md), merged as PR #26 on 2026-10-23. The user then approved implementation. The collaboration hub and account CLI/API are not yet available.

## Increment 1: discovery and PKCE preparation (2026-10-23)

The new `cyber-account` library isolates account protocol code from LLM-provider credentials, local owner authentication and runtime execution. No existing command is wired to it yet, and local use performs no new account network requests.

Implemented:

- Explicit discovery with a ten-second request timeout, no HTTP redirect following and a 256 KiB response bound, including chunked bodies.
- HTTPS issuer/endpoint validation; plain HTTP requires explicit literal-loopback development opt-in. Reject issuer mismatch, credentials in URLs and URL fragments. Endpoints come from discovery.
- Scope selection including `cyber:collaborate`; unavailable advertised scopes are returned for caller warnings. If `scopes_supported` is omitted, request the normal scope set and leave feature authorization to later validated-token checks. An explicitly advertised set without `openid` cannot begin OIDC login.
- Device-flow readiness only when both its endpoint and grant are advertised.
- Public-client login preparation using a 64-byte random verifier, S256, independent 32-byte state and nonce, and a five-minute monotonic deadline. Secrets have no Debug/Serialize/Clone implementation and use zeroizing storage. Authorization URLs are sensitive interactive UI data.
- Full loopback callback validation with exact redirect matching, constant-time state comparison, duplicate/ambiguous parameter rejection, safe provider-denial errors and single-use attempt ownership. The low-level API requires the full callback URL; bare-code completion remains pending at the CLI layer and cannot bypass state validation.
- Sensitive exchange parameters retain the original verifier, redirect and nonce for later token exchange/ID-token validation. A `CodeGrant` is not an authenticated identity.

Still pending: callback listener/browser/headless UX, discovery cache, actual code exchange, signed ID/access-token checks and key rotation, protected credential storage, serialized refresh, account CLI/API, required live introspection, membership enrollment, hub storage/API, messaging/tasks, local runtime connectors, TUI/SDK and end-to-end multi-principal acceptance. Tasks 2.1–2.4 remain open.

Verification on local macOS:

- `cargo test --locked -p cyber-account`: 20 passed (2 expiry unit tests and 18 protocol tests against local HTTP fixtures); doctests contain no cases.
- `cargo clippy --locked -p cyber-account --all-targets -- -D warnings`: passed.
- `cargo fmt --all --check`: passed.
- OpenSpec 1.13.2 `validate --all --strict`: 87 passed. Canonical lint reports 0 errors and 23 warnings; inventory remains 49 capabilities / 899 requirements.
- Changed Markdown links (116) resolve; `git diff --check` passes.

Code review uses the repository's Rust module boundaries: the PKCE state machine performs no I/O; discovery owns bounded HTTP transport. Review covers all new source/tests, manifests, Windows test selection and docs. No unresolved review findings were identified. Tests validate protocol preparation only; they do not establish completed OIDC login, live Auth provisioning or collaboration acceptance. Native Linux/Windows and the full workspace CI results must be reported from the implementation PR, separately from local checks.

The [live Auth deployment contract](../../openspec/changes/add-project-collaboration-hub/auth-contract.md) records required scope/client/audience/service provisioning. This increment does not change the Auth deployment or use real account credentials.


## Increment 2: verified account credentials (in progress)

The account library now validates signed ID/access tokens using discovered asymmetric signing algorithms and keys, refreshes metadata/JWKS once on an unknown signing key, binds public-client code exchange to the ID nonce and access-token subject/audience, and rejects forged refresh ID tokens and identity changes. A real temporary loopback listener and headless full-redirect submission share the same single-use state validation; browser responses contain no credentials and do not claim login success before verification.

Hub authorization requires uncached backend-authenticated live introspection with an active user identity, matching subject/audience/scope and current `orgs`. Returned authorization evidence retains a monotonic deadline of at most 60 seconds, additionally bounded by both token expiries. Organization and step-up checks fail after expiry. Managed MFA requires a verified MFA indication or password plus OTP; token refresh preserves the original authentication time.

Protected account storage uses generation-based native keyring chunks with bound manifests, a dedicated issuer/client/subject namespace and verified writes, or a Unix owner-only atomic file fallback. Login, refresh and logout share a cross-process lock. A durable pending marker precedes refresh, and an owned task holds the lock through rotation/persistence when a caller is cancelled. Unknown refresh outcomes clear credentials or leave the pending marker, requiring fresh login rather than replay. Retired keyring generations remain tracked through cleanup for later logout.

Local account suites now cover 41 tests, including signed protocol fixtures, token/key/organization failures, real callback HTTP, concurrent refreshes, cancellation during a held token response, pending-marker recovery, chunk tampering and unsafe file modes/symlinks. Account CLI/API integration, persistent discovery cache, device flow, deployment acceptance and the collaboration hub remain pending. This is progress toward the full PR #27 target; account tasks are not marked accepted yet.
