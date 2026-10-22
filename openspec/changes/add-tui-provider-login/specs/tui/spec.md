## ADDED Requirements

### Requirement: Provider login commands
(P1) The TUI SHALL provide `/login [provider]` and `/logout [provider]`, backed by the server's provider login API:

- **Listing:** a bare `/login` SHALL open a picker of the subscriptions that support login (today `openai`, which is a ChatGPT Plus or Pro plan).
- **Login dialog:** `/login openai` (alias `chatgpt`) SHALL start an attempt and open a dialog. The dialog SHALL:
  - show the authorization URL, copied to the clipboard with Ctrl+Y,
  - say whether the server is waiting for the browser,
  - accept a pasted redirect URL submitted with Enter,
  - cancel the attempt on Esc.
- **Polling:** while the dialog is open, the TUI SHALL poll the attempt once a second.
- **Success and failure:** on success the dialog SHALL close, show `Signed in to ChatGPT as <email> (<plan>)` and open the model picker. A refused paste or a failed request SHALL be shown in the dialog, which stays open so the user can paste again or cancel. An attempt that ends `failed` SHALL close the dialog and show its error.
- **Refusals:** `/login google` (alias `gemini`) SHALL be refused, saying that Google subscription login is not supported. Any other provider SHALL be refused with a hint to configure its API key.
- **Logout:** `/logout openai` SHALL sign out and report whether a login was removed.

#### Scenario: Sign in from the TUI
- **WHEN** the user submits `/login openai`, opens the shown URL and approves in the browser
- **THEN** the dialog closes, the TUI shows the signed-in account and plan, and the model picker lists the plan's `chatgpt/*` models

#### Scenario: Headless paste
- **WHEN** the dialog says the server is not waiting for the browser and the user pastes the redirect URL and presses Enter
- **THEN** the attempt completes the same way

#### Scenario: Google refused
- **WHEN** the user submits `/login google`
- **THEN** no attempt starts and the TUI says that Google subscription login is not supported
