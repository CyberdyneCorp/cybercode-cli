## Why

Pi and OpenCode let users pay for models with subscriptions they already have. Cyber Code now supports ChatGPT (`add-chatgpt-subscription-login`). The next most valuable plan is GitHub Copilot:

- **Official support:** since January 2026, GitHub officially supports Copilot Pro, Pro+, Business and Enterprise subscriptions in third-party agents (it announced OpenCode first).
- **Many models, one plan:** a single Copilot plan serves OpenAI, Anthropic and Google models, so it is also the supported way to reach Claude models with a subscription. Anthropic does not allow Claude Pro/Max logins in third-party tools.

## What Changes

- `provider-credentials`, new requirement "GitHub Copilot subscription login" (P1):
  - `cyber providers login copilot [--enterprise <host>]` signs in with GitHub's OAuth device flow. The CLI prints the verification URL and a user code, then polls.
  - The GitHub token is stored owner-only in `<data>/copilot-auth.json`.
  - A short-lived Copilot API token is minted from it and renewed before expiry, under the same cross-process refresh lock rules as the ChatGPT login.
  - `logout` removes the login and the cached model list.
- `provider-catalog`, new requirement "GitHub Copilot subscription provider" (P1):
  - With a stored login, the catalog gains a `copilot` provider.
  - Its models are the ones Copilot lists for the account (`GET <api>/models`, picker-enabled entries), cached in `<data>/copilot-models.json` at login and refreshed daily. They are unpriced, and each model uses the protocol Copilot lists for it: Chat Completions, or Responses for models that support it.
  - Requests carry the Copilot integration headers and `X-Initiator`: `user` for the request a user prompt opens, `agent` for the tool-loop steps that follow it. This matches how Copilot's own agent mode counts premium requests.
- `provider-credentials`, modified "ChatGPT token refresh": the refresh lock, owner-only file and "re-read under the lock" rules move into a shared subscription-login store that both providers use. Behavior does not change.

## Non-goals

- **Google Gemini CLI / Antigravity / Code Assist logins.** In February 2026 users reported automated 403 "ToS violation" account suspensions for using these OAuth logins from third-party tools (OpenCode, OpenClaw), and Google has published no policy allowing it. Gemini stays on API keys through the planned native Gemini adapter (`provider-catalog` → Additional native provider adapters).
- **Claude Pro/Max login.** Anthropic forbids it for third-party tools.
- Keyring storage, `providers list|use`, several Copilot accounts at once, Windows (refused, as with ChatGPT), and Copilot's coding-agent or code-review APIs.

## Impact

- **`crates/cyber-llm`:**
  - New shared `subscription` module (owner-only login file, refresh lock, model cache), extracted from `chatgpt`.
  - New `copilot` module: device flow, token mint, model list.
  - New `ApiKind::CopilotChat` adapter, which picks Chat Completions or Responses per model.
- **`crates/cyber-cli`:** `providers login|logout copilot`.
- **`crates/cyber-server`:** the Turn loop tells the adapter whether a request starts a prompt or continues a tool loop, through a new `LlmRequest::initiator` field.
- **Docs:** `README.md` and `docs/local-core-guide.md`.
