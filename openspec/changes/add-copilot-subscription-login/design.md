## Context

`add-chatgpt-subscription-login` built the first subscription provider. Its pieces are an owner-only login file, refresh under a cross-process lock with a re-read under the lock, a model list fetched from the backend and cached for the synchronous catalog build, an adapter that renews the token right before each request, and a provider that exists only while a login is stored. Copilot needs all of these, and adds three things ChatGPT doesn't have:

1. **A device-code login** instead of a loopback redirect, so it also works over SSH.
2. **Two tokens.** The long-lived GitHub OAuth token is the stored credential. Each request uses a short-lived Copilot API token, minted from the GitHub token at `GET https://api.github.com/copilot_internal/v2/token`, which also returns the account's API base URL (`endpoints.api`). These tokens live roughly 30 minutes.
3. **Two protocols.** Copilot serves Chat Completions (`<api>/chat/completions`) for every model and Responses (`<api>/responses`) for models whose `supported_endpoints` lists it. GPT-5-family models must use Responses.

## Goals / Non-Goals

Goals: `cyber providers login|logout copilot`, a `copilot` provider listing exactly the account's picker-enabled models, correct premium-request attribution, and GitHub Enterprise (data residency) hosts.

Non-goals: listed in the proposal.

## Decisions

### Shared subscription store

The ChatGPT login file, refresh lock and model cache move into `cyber-llm::subscription`:

- `LoginFile<T: Serialize + DeserializeOwned>`: owner-only, at most 64 KiB, versioned, atomic replace, refused when other users can read it.
- `refresh_locked(file, due, renew)`: the lock, the re-read under the lock, persisting before the lock is released, and leaving the file untouched when renewal is rejected.
- `ModelCache<M>`: cached list, 24-hour age, back off a day after a failure.

`chatgpt` keeps its public API and behavior, and its tests must pass unchanged. That proves the extraction changed nothing.

### Device flow

- `POST https://<host>/login/device/code` with `{client_id, scope: "read:user"}` returns `device_code`, `user_code`, `verification_uri`, `interval` and `expires_in`.
- The CLI prints `Open <verification_uri> and enter <user_code>`, then polls `POST https://<host>/login/oauth/access_token` with `grant_type=urn:ietf:params:oauth:grant-type:device_code` every `interval` seconds:
  - `slow_down` adds 5 s to the interval,
  - `expired_token` and `access_denied` fail the login,
  - `expires_in` (at most 15 minutes) bounds the wait,
  - Ctrl+C stops polling and stores nothing.
- `<host>` is `github.com`, or the `--enterprise <host>` value. That value must be a bare hostname; it is stored with the login.
- After the device flow, the login immediately mints a Copilot token. A 401/403, or a response without `endpoints.api`, means the account has no Copilot plan: the login fails with `This GitHub account has no active Copilot subscription` and stores nothing.

**Client ID (open question, decided by task 1.1).** Cyber Code should register its own GitHub OAuth App with device flow enabled. GitHub's partner support may only accept known clients at the token-mint endpoint. The task tests a Cyber-registered client against a real Copilot account:

- If it is accepted, that client is the constant.
- If not, the fallback is the client ID other harnesses used before the partnership, and the risk is recorded as for ChatGPT.

Either way the client ID is one constant.

### Stored login and token mint

- `<data>/copilot-auth.json` holds `{version, host, github_token, user}`. The GitHub token does not expire unless revoked.
- The Copilot token is kept in memory per process, not in the file. Each process mints its own, so the refresh lock is only needed when another process might write the file. The GitHub token never rotates, so concurrent mints are harmless.
- A minted token is reused until 5 minutes before its `expires_at`.
- If minting fails with 401, the GitHub token was revoked: the request fails with `GitHub Copilot login expired or was revoked; run cyber providers login copilot`, and the file is kept.

### Catalog and models

- `GET <api>/models` with the Copilot token. Kept: entries with `model_picker_enabled: true` and `policy.state` not `disabled`, and slugs matching `[A-Za-z0-9._-]`.
- Each kept entry becomes a model:
  - `name` and context limit from `capabilities.limits.max_context_window_tokens`,
  - output limit from `max_output_tokens`,
  - `vision` from `capabilities.supports.vision`,
  - tool support from `capabilities.supports.tool_calls`,
  - its protocol (Responses when `supported_endpoints` lists `/responses`, else Chat Completions).
- Cost is `None` (unpriced). Premium-request multipliers are not modeled as cost.
- The provider (id `copilot`, name `GitHub Copilot (subscription)`) exists only with a stored login, like `chatgpt`. `ApiKind::CopilotChat` is not selectable from configuration.

### Requests

- Common headers: `Authorization: Bearer <copilot token>`, `Copilot-Integration-Id: vscode-chat`, `Editor-Version: cyber/<version>`, `Editor-Plugin-Version: cyber/<version>`, `Openai-Intent: conversation-edits`, and `X-Initiator`.
- `X-Initiator` is `user` when the request is the first model call after a promoted user prompt, and `agent` for later steps of the same Turn: tool-result continuations and compaction. This needs the runtime to say which a request is, through a new `LlmRequest::initiator: Option<Initiator>`. Other adapters ignore it.
- Requests with images also send `Copilot-Vision-Request: true`.
- The Responses path reuses the Responses body (with `store: false`). The Chat path reuses the OpenAI-compatible adapter. The Copilot adapter only chooses the protocol, attaches the token and adds the headers.

### Model-list refresh

The same as ChatGPT: fetched at login, refreshed by the adapter when older than 24 hours, a failure keeps the old list and waits a day, and logout deletes the cache.

## Risks / Trade-offs

- [GitHub changes the internal token-mint endpoint or headers] → All endpoints and headers are constants in `copilot`, and a live check is part of acceptance (task 5.2).
- [Premium-request attribution is wrong] → `X-Initiator` follows Copilot agent mode's documented semantics. A test pins `user` on the first call of a Turn and `agent` on continuations.
- [Business/Enterprise policy disables some models] → They are filtered out by `policy.state`, and a 403 on a request names the model and the org policy.
- [The shared-store refactor changes ChatGPT behavior] → ChatGPT's existing tests run unchanged against the extracted store.
