# Live authentication deployment contract

Public contract checked on 2026-10-23 UTC using unauthenticated, read-only requests. This records deployment readiness, not completed login or membership acceptance.

- [OpenAPI document](https://auth.backend.coolify.cyberdynecorp.ai/openapi.json)
- [OIDC discovery](https://auth.backend.coolify.cyberdynecorp.ai/.well-known/openid-configuration)
- Observed issuer: `https://auth.backend.coolify.cyberdynecorp.ai`

Use this deployment as the operator-provided integration reference. Keep `account.issuer` configurable for self-hosters and discover protocol endpoints at runtime; do not hardcode this hostname or infer enabled grants from OpenAPI paths alone.

## Advertised and documented capabilities

| Capability | Public evidence | Integration consequence |
| --- | --- | --- |
| Browser login | Discovery advertises authorization code, refresh tokens and PKCE `S256` | Use code + PKCE with the callback/state/nonce protections in the account contract. |
| Signing keys | Discovery advertises `/.well-known/jwks.json` | Validate issuer, audience, signature and token time using discovered keys. |
| Live token validation | Discovery advertises `/api/v1/auth/introspect`; OpenAPI requires the resource server's own client-credentials service bearer | Provision a separate backend credential. Require `active: true`, `token_type: user`, expected subject/audience/scope and current organization authorization as specified in the design. |
| Organization directory | OpenAPI documents `GET /api/v1/orgs/{org_id}/members`, requiring a service credential with `directory:read` or admin authority; discovery advertises `directory:read` | Optional owner user picker; use least-privilege directory access. This is separate from admitting a caller using live introspection. |
| Enrollment configuration | OpenAPI exposes admin client, scope and protected-resource registries | These are provisioning interfaces, not evidence that the collaboration clients, audience or scope exist. |
| Device login | OpenAPI lists a device authorization path, but discovery advertises neither its endpoint nor the device grant | Treat device login as unavailable on this deployment until advertised and client-enabled. Use the specified PKCE/headless fallback. |
| Collaboration scope | `cyber:collaborate` is absent from live `scopes_supported` | Register/enable the required scope before live collaboration acceptance. Do not weaken the scope guard to accommodate this gap. |

The introspection response schema exposes `active`, `sub`, `scope`, `aud`, `exp`, `client_id`, `token_type`, `org`, `orgs`, `auth_time` and `amr`, among other fields. A documented response shape does not establish that a particular user is active or belongs to an organization. Authenticated integration tests must verify those results against separate real principals, including rejection after membership removal and user deactivation.

## Remaining acceptance work

Task 2.4 remains open: verify/provision the public CLI client and allowed redirect URIs, the hub audience and scope, and the backend introspection client; optionally provision scoped directory access. Verify actual token issuance, refresh, introspection authorization, current organization claims and revocation against the configured deployment. No accounts, clients, scopes, credentials or deployment settings were created or changed during this check, and no authenticated flow was exercised.

Public contract snapshots are diagnostic evidence only, not runtime content pins:

- OpenAPI SHA-256: `0a76e139c858e440a417dfdf01f1e62ed72c732c5bd189c6a20ae4e1ea85bdb3`
- Discovery SHA-256: `293ddaac98f8787411fcb35d4b7a7a4168613a9f607db0b3dad290e20e7b645f`
