# Project collaboration implementation evidence

Approved design: [add-project-collaboration-hub](../../openspec/changes/add-project-collaboration-hub/README.md), merged as PR #26 on 2026-10-23. The user then approved implementation. The branch implements the experimental hub and account/CLI/TUI/API/SDK surfaces, off by default. The evidence below separates completed local checks from Linux real-process CI and live deployment acceptance.

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


## Increment 2: verified account credentials (historical checkpoint)

This point-in-time boundary is superseded by Increment 3 below.

The account library now validates signed ID/access tokens using discovered asymmetric signing algorithms and keys, refreshes metadata/JWKS once on an unknown signing key, binds public-client code exchange to the ID nonce and access-token subject/audience, and rejects forged refresh ID tokens and identity changes. A real temporary loopback listener and headless full-redirect submission share the same single-use state validation; browser responses contain no credentials and do not claim login success before verification.

Hub authorization requires uncached backend-authenticated live introspection with an active user identity, matching subject/audience/scope and current `orgs`. Returned authorization evidence retains a monotonic deadline of at most 60 seconds, additionally bounded by both token expiries. Organization and step-up checks fail after expiry. Managed MFA requires a verified MFA indication or password plus OTP; token refresh preserves the original authentication time.

Protected account storage uses generation-based native keyring chunks with bound manifests, a dedicated issuer/client/subject namespace and verified writes, or a Unix owner-only atomic file fallback. Login, refresh and logout share a cross-process lock. A durable pending marker precedes refresh, and an owned task holds the lock through rotation/persistence when a caller is cancelled. Unknown refresh outcomes clear credentials or leave the pending marker, requiring fresh login rather than replay. Retired keyring generations remain tracked through cleanup for later logout.

Local account suites now cover 41 tests, including signed protocol fixtures, token/key/organization failures, real callback HTTP, concurrent refreshes, cancellation during a held token response, pending-marker recovery, chunk tampering and unsafe file modes/symlinks. Account CLI/API integration, persistent discovery cache, device flow, deployment acceptance and the collaboration hub remain pending. This is progress toward the full PR #27 target; account tasks are not marked accepted yet.

## Increment 3: account runtime and storage review (2026-10-24)

The account CLI and local-owner API now provide browser/headless login, whoami,
organization selection and logout. Discovery metadata is cached for 24 hours;
unknown signing keys force uncached metadata/JWKS refresh. Advertised RFC 8628
device login respects polling intervals and slowdown. Explicit reauthentication
(`cyber login --reauth`, API `reauthenticate: true`) always uses PKCE with
`prompt=login` and `max_age=900`, and rejects missing, future or stale ID-token
`auth_time` rather than trusting that the issuer honored the request.

Native account identities include the canonical installation directory so separate
installation locks cannot share rotating credentials. A private cleanup journal
precedes native chunk writes. Recovery removes uncommitted chunks or restores
cleanup reachability after an interrupted manifest/index commit, without activating
an unfinished login. Retired chunk references remain until deletion is verified.
Small host secrets use a namespace-bound protected map, native vault entries where
available and a private Unix fallback. Native metadata contains no raw secret.

Local logout cancels pending login commits and calls registered runtime cleanup
hooks after removing credentials. The CLI uses the matching running local owner
when available so connectors can stop and revoke their publications before
best-effort issuer revocation. Tokens remain inside the owner service and trusted
runtime; account HTTP responses expose display claims/status only.

After the storage/security review, `cargo test --locked -p cyber-account` passes
54 tests (2 unit, 22 OIDC protocol, 30 token/storage) on macOS with injected memory
vaults. Coverage includes two independent live subjects, backend client-credentials
minting, explicit reauthentication, installation isolation, partial native writes,
interrupted index commits, namespace separation and private fallback storage.
The build used `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=line-tables-only`
and `CARGO_PROFILE_TEST_DEBUG=line-tables-only`. Actual native-vault interaction,
provisioned live issuer acceptance and multi-host deployment acceptance remain
unverified; fixtures do not establish those outcomes.

## Receiver consent and terminal surfaces (2026-10-24)

The implementation branch adds `/collab` people, conversation and shared-task panels using the local owner API. Viewer controls are read-only; direct human sends carry explicit participants, and task updates carry revision/claim fences. The panel sanitizes remote text and invalidates asynchronous replies when switching Sessions. Failed refreshes clear previously displayed membership and task data.

Hub deliveries enter the production runtime messaging admission path with `origin_kind: collaboration`. The immutable event/projected sender key includes structured hub, project, verified member, optional agent, membership kind/generation, remote message, receiver publication and chain depth/hops. No remote local Session ID is stored. All admissions begin held; generic peer/local allow rules cannot promote them. The local inbox remains the consent surface. Release requires a connector callback to revalidate current hub authorization, runs without a Session state lock, then checks current local inbound policy. Missing callbacks, missing durable provenance and uncertain authorization keep messages held. Existing messaging remains unchanged in behavior.

Local verification completed:

- `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=line-tables-only cargo test -p cyber-server --test collaboration_messaging --test messaging`: 4 collaboration inbox integration tests and 19 existing messaging integration tests passed. Coverage includes immutable retry binding, restart/rebuild, metadata loss, absent/revoked release authorization, current disabled/noninteractive policy, archived Sessions and chain limits.
- `cargo test -p cyber-tui collaboration --lib` with the same build settings: 7 focused state/attribution/render tests passed. An additional terminal/bidi sanitization test is included in the messaging suite.
- `cargo check -p cyber-tui --lib`: passed.
- SDK production and test TypeScript projects typecheck with the new signed multi-principal local OIDC fixture. The fixture provides independent Alice/Bob/Mallory authorization and mutable current-user/org state; it is preparation, not evidence of a completed full binary/hub acceptance run.

These checks use local scripted models and synthetic signed identities. Live CyberdyneAuth provisioning, multi-host deployment and complete binary/SDK collaboration acceptance must be recorded separately when run. No native account vault or real account/model credential was used by these tests.

Account runtime acceptance on macOS also passes
`cargo test --locked -p cyber-app -p cyber-cli --test account`: 4 local-owner API
and 4 real-process CLI tests. These cover owner authentication, token-free/no-store
responses, logout races and runtime cleanup hooks, explicit reauthentication with
a device-advertising issuer, global issuer isolation and registered-owner logout.
Positive file-fallback CLI login remains Linux-only; no macOS test writes the
actual native vault. Account credential/vault tests use injected memory vaults.

The hub Auth manager's 4 fixture regressions pass, covering the original cache
lease deadline, independent subjects, fresh user/org revocation and Auth outage
checks, expired source credentials, and shared renewable backend service tokens.
The enrollment module's 8 transactional regressions pass, including repository
confirmation before history access, exact-recipient approval, keyed codes,
durable failed attempts, atomic consumption, reissue/expiry/revocation, issuance
limits unaffected by 10,001 older challenge rows, last-owner protection and
cross-project refusal, positive cross-org guest admission/policy revocation, and
preserved managed MFA for owner administration and internal admission, and
recipient-only application status/descriptor visibility with revoked-admission
refusal. These
tests use local signed OIDC fixtures and protected
fake/file stores; they do not establish live issuer or multi-host acceptance.
