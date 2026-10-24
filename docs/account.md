# Cyber Account

Local Sessions, models, tools, workflows, Teams and direct peers work without an account. A Cyber Account identifies a person for the project collaboration hub; it is separate from `cyber providers login` and never supplies LLM-provider credits.

The account CLI, local owner API and experimental project collaboration hub are implemented. Collaboration is disabled by default and requires explicit trusted configuration. Live CyberdyneAuth client/audience provisioning and deployment acceptance remain separate operator steps; see [implementation evidence](implementation/project-collaboration.md) and the [collaboration guide](collaboration.md).

Configure the issuer and registered public client in user-global `cyber.json`/`cyber.jsonc`:

```json
{
  "account": {
    "issuer": "https://auth.backend.coolify.cyberdynecorp.ai",
    "client_id": "cyber-cli",
    "audience": "your-registered-collaboration-audience"
  }
}
```

`CYBER_ACCOUNT_ISSUER` overrides the issuer. A checkout's account settings are ignored. The default issuer is the supplied CyberdyneAuth deployment (overridable at build time with `CYBER_ACCOUNT_DEFAULT_ISSUER`); the default client is `cyber-cli` and the default access audience is the client ID. Set the actual hub audience before collaboration login. Production endpoints require HTTPS. `account.allow_loopback_http: true` is a user-global development option for literal-loopback fixtures only.

```sh
cyber login
cyber login --no-browser
cyber login --reauth
cyber whoami
cyber whoami --org team
cyber logout
cyber logout --all
```

Browser login uses a temporary loopback listener, PKCE S256, state and nonce. Headless login accepts the full redirect URL; a bare authorization code additionally requires the state copied from that same redirect. When discovery advertises an enabled device endpoint and grant, headless login prints its verification URI/user code and polls with the issuer's interval and slowdown handling.

`cyber login --reauth` requests `prompt=login` and `max_age=900`. It requires a signed ID token with an authentication time within 15 minutes; an issuer that ignores the freshness request cannot silently complete this flow. Reauthentication uses PKCE even with `--no-browser` and an advertised device grant. Managed MFA is additionally checked from live introspection by sensitive hub operations.

Login warns about scopes absent from discovery. An omitted `cyber:collaborate` disables collaboration; a valid account alone grants no project membership. `whoami` displays verified token claims, which are a snapshot; the hub separately checks current active-user/org authorization. Selecting an organization does not grant GitHub permissions or project access.

Credentials use the native macOS/Windows vault, or an owner-only Unix file fallback with a warning and `cyber doctor` diagnostic. Native entries are bound to the canonical installation directory and account identity. The account index contains no tokens. Partial native chunk writes have a durable cleanup journal, and retired generations remain tracked until deleted. Small collaboration capability and hub MAC secrets use the same native vault/private-file policy through a separate protected secret map. Refresh is serialized across processes and journals a pending rotation before sending it. A crash or uncertain refresh outcome requires fresh login rather than replaying a possibly consumed refresh token. Logout removes account credentials and cancels older pending login commits, while preserving provider logins and local data. Remote token/session revocation is best effort.

The local owner API exposes `GET /api/v1/account`, `POST /api/v1/account/login`, `GET /api/v1/account/login/:attemptID`, `POST /api/v1/account/login/:attemptID/code` and `DELETE /api/v1/account?all=true`. Login accepts optional `no_browser` and `reauthenticate` booleans, for example `{ "no_browser": true, "reauthenticate": true }`. Responses contain display claims/status, never raw access, refresh or ID tokens; they are marked `Cache-Control: no-store`. The generated SDK exposes `client.account`.

The [live deployment contract](../openspec/changes/add-project-collaboration-hub/auth-contract.md) records public discovery/OpenAPI verification. Public CLI registration, redirect URIs, API audience, collaboration scope and backend introspection credentials still need operator provisioning/acceptance. Fixture results do not establish that the live deployment is ready.
