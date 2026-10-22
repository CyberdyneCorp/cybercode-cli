## MODIFIED Requirements

### Requirement: ChatGPT subscription provider
(P1) When a ChatGPT login is stored, the catalog SHALL include provider `chatgpt` (name `ChatGPT (subscription)`, base URL `https://chatgpt.com/backend-api/codex`), with credential source `login`.

- **Models:** the models the backend lists for the signed-in account (`GET <base>/models?client_version=1.0.0`, keeping only `visibility: "list"` entries). Each SHALL use the backend's display name and context window, have one reasoning variant per supported effort, and have unknown pricing (`unpriced`). `providers.chatgpt` configuration SHALL apply on top of these models.
- **Model list cache:** the list SHALL be cached in `<data>/chatgpt-models.json`. Login SHALL fetch it. A request SHALL refresh it when it is missing or older than 24 hours. A failed fetch SHALL keep the old list, never fail the request, and not be retried for 24 hours. Logout SHALL delete the cache.
- **Without a login:** the catalog SHALL NOT contain `chatgpt`.
- **Reload on login:** a login or logout completed through the provider login API SHALL rebuild the running server's catalog, so `chatgpt` models are listed and resolvable (or gone) without a restart. Turns already running SHALL keep the model they resolved.
- **Requests:** the provider's adapter SHALL send Responses API requests to `<base>/responses` with:
  - the stored access token as bearer,
  - `chatgpt-account-id` set to the stored account ID,
  - `OpenAI-Beta: responses=experimental` and `originator: cyber`,
  - `store: false`,
  - no `max_output_tokens`,
  - `instructions` always present.
- **Protocol:** the `chatgpt-codex` protocol SHALL NOT be selectable through `api.kind` or models.dev data.

#### Scenario: Provider lists the plan's models
- **WHEN** the backend lists `gpt-6-sol` (`visibility: "list"`) and `gpt-reserve` (`visibility: "hide"`) for the account, and the user signs in with `cyber providers login chatgpt`
- **THEN** `cyber models chatgpt` lists `chatgpt/gpt-6-sol` as available and unpriced, and does not list `chatgpt/gpt-reserve`

#### Scenario: Model list refresh failure
- **WHEN** the cached list is older than 24 hours and the backend answers the model list with an error
- **THEN** the Turn still runs, the old list is kept, and no model list request is sent for the next 24 hours

#### Scenario: Subscription request shape
- **WHEN** a Turn runs on `chatgpt/gpt-6-sol` with a 32000-token output cap
- **THEN** the request goes to `https://chatgpt.com/backend-api/codex/responses` with the bearer token, the `chatgpt-account-id` header and `store: false`, and without `max_output_tokens`

#### Scenario: No login
- **WHEN** no ChatGPT login is stored
- **THEN** `cyber models chatgpt` fails with `Provider not found: chatgpt`
#### Scenario: Login without restart
- **WHEN** a server started without a ChatGPT login completes an attempt through `POST /api/v1/providers/chatgpt/login`
- **THEN** `GET /api/v1/models` lists the plan's `chatgpt/*` models and a new Turn can use them, without restarting the server
