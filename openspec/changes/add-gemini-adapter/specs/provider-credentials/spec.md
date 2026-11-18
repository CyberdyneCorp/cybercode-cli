## ADDED Requirements

### Requirement: Gemini API key
(P1) The `google` provider SHALL be authorized by a Gemini API key from Google AI Studio, resolved by Credential precedence: `providers.google.api.settings.api_key`, then a stored `google` connection, then the environment variables `GEMINI_API_KEY`, `GOOGLE_GENERATIVE_AI_API_KEY` and `GOOGLE_API_KEY`, in that order, the first one set winning. As for `openai` and `anthropic` under Catalog source integrity, the downloaded catalog SHALL NOT change the `google` endpoint (`https://generativelanguage.googleapis.com/v1beta`) or the variables it reads, and no other catalog provider SHALL read `GEMINI_API_KEY`, `GOOGLE_GENERATIVE_AI_API_KEY` or `GOOGLE_API_KEY`. The user's own `providers.google` configuration MAY change them. `cyber providers login google` SHALL offer only the API key method (masked input) and SHALL validate a new key with `GET <base>/models` before storing it. The key SHALL be redacted from logs, errors and exports like every other provider key.

#### Scenario: Environment key
- **WHEN** only `GEMINI_API_KEY` is set
- **THEN** `cyber providers list` shows `google  GEMINI_API_KEY` under `Environment`, and Gemini models are available

#### Scenario: Variable order
- **WHEN** `GOOGLE_API_KEY` and `GOOGLE_GENERATIVE_AI_API_KEY` are set, `GEMINI_API_KEY` is not, and no key is configured or stored
- **THEN** the `google` provider uses `GOOGLE_GENERATIVE_AI_API_KEY`, and `cyber providers list` shows `google  GOOGLE_GENERATIVE_AI_API_KEY` under `Environment`

#### Scenario: Generic Google key alone
- **WHEN** only `GOOGLE_API_KEY` is set
- **THEN** the `google` provider uses it, and no other catalog provider receives it

#### Scenario: Tampered catalog endpoint
- **WHEN** a downloaded catalog sets the `google` provider's `api` to another host
- **THEN** Gemini requests still go to `https://generativelanguage.googleapis.com/v1beta`

### Requirement: Google subscription login refused
(P1) The system SHALL NOT sign in with, or use credentials from, Google consumer subscriptions or their OAuth clients: Google AI Pro/Ultra, Gemini Code Assist for individuals, the Gemini CLI login and the Antigravity login. `/login google` (alias `gemini`) SHALL be refused, `cyber providers login google` SHALL offer no OAuth method, and both SHALL say that Google subscription login is not supported and that a Gemini API key from AI Studio works instead. The reason is recorded here: the Antigravity terms (clause 6) and the Gemini CLI terms prohibit third-party use of their OAuth, consumer tiers stopped being served through Gemini CLI on 2026-06-18, and account suspensions for third-party use were reported in 2026.

#### Scenario: TUI login refused
- **WHEN** the user runs `/login google`
- **THEN** no browser opens, nothing is stored, and the TUI says Google subscription login is not supported and to use a Gemini API key from AI Studio

#### Scenario: API key login accepted
- **WHEN** the user runs `cyber providers login google` and enters a key that `GET <base>/models` accepts
- **THEN** the key is stored as the `google` connection and Gemini models become available without a restart

#### Scenario: Invalid Gemini key
- **WHEN** the user enters a key and `GET <base>/models` answers 400 `API_KEY_INVALID`
- **THEN** the key is not stored and the command prints `Credential rejected by google`
