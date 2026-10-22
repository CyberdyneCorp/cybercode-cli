## Context

The ChatGPT login exists as a CLI command (`crates/cyber-cli/src/commands/providers.rs`). The TUI is an HTTP client of a server that may be embedded, auto-started or attached. The credentials must live where the model calls are made, which is the server. The server's `CatalogResolver` builds a `Catalog` once in `App::build`, and the catalog includes `chatgpt` only when the login file exists at that moment.

## Decisions

### The server runs the flow

The TUI asks the server to start an attempt, instead of running OAuth in the TUI process and writing the file itself:

- The login lands in the data directory of the server that uses it, even when the TUI is attached to a server it did not start.
- The server can rebuild its own catalog when the login completes.
- Other clients (SDK, IDE, web) get the same flow for free.

The shape mirrors `cyber-account`'s `POST /account/login` plus attempt polling.

### Attempts

- **Registry:** attempts live in memory in `cyber-app`, keyed by `pla_<random>`. Starting an attempt cancels a pending one, so only one listener ever holds port 1455.
- **Lifetime:** an attempt expires 5 minutes after it starts and then reads as `failed: login timed out`. A finished attempt stays readable for 10 minutes.
- **Callback:** the callback listener is bound when the attempt starts. If the port is busy (for example, a CLI or Codex login is in progress), the attempt still starts with `callback: false`, and the client must use the pasted-code path.
- **PKCE and state:** the PKCE verifier and `state` never leave the server. A pasted redirect URL is checked against the attempt's `state` before any token request is sent.

### Shared completion

`chatgpt::complete_login(http, issuer, backend, file, code, verifier)` does the following:

1. exchanges the code,
2. checks the account,
3. saves the login,
4. fetches and caches the models; a failed fetch becomes a warning and the login is kept.

The CLI's `login` calls it too, so both paths stay identical.

### Reloadable resolver

`CatalogResolver` keeps the catalog source data, the global config and the env, and holds the catalog behind an `RwLock<Arc<Catalog>>`. `reload()` rebuilds it. `catalog()` returns an `Arc<Catalog>`.

- Only provider login and logout call `reload()` in this change; config reload stays out of scope.
- Turns already running keep their `ResolvedModel` (the adapter and the endpoint), so a logout never breaks a running request beyond the auth failure the provider itself reports.

### TUI

- **Commands:** `/login [provider]` and `/logout [provider]`. The names `openai` and `chatgpt` map to provider `chatgpt`; `google` and `gemini` get the refusal toast.
- **Dialog:** an overlay shows the URL (Ctrl+Y copies it) and a one-line input for a pasted redirect URL. Enter submits a paste, and Esc cancels the attempt.
- **Polling:** while the dialog is open, the TUI polls the attempt once a second.
- **On success:** the dialog closes, a toast names the account and plan, and the model picker opens.

## Risks

- **A remote server cannot receive the browser redirect on localhost.** The dialog always offers the pasted-URL path, and says so when `callback` is false.
- **Port 1455 is shared with the Codex CLI.** That is the same constraint the CLI login already has; the busy case falls back to pasting.
