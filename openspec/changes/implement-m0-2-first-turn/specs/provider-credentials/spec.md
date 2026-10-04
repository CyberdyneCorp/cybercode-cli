## MODIFIED Requirements

### Requirement: Credential precedence
(P0) The system SHALL resolve a provider's credential in this order, first match wins: explicit `providers.<id>.api.settings.api_key` in config (supporting `{env:VAR}` and `{file:path}` placeholders), the active stored connection (most recently added unless one is marked active), environment variables declared for the provider, and the cloud IAM chain. `CYBER_AUTH_CONTENT` takes the place of stored connections for its process. A provider SHALL require no credential when its effective URL is a loopback address (`localhost`, `127.0.0.1`, `::1`, `0.0.0.0`) or its config sets `api.settings.auth: "none"`; any other provider without a resolved credential is unavailable with reason `missing_credentials`.

#### Scenario: Stored key beats environment
- **WHEN** both `OPENAI_API_KEY` and a stored OpenAI key exist and config sets no key
- **THEN** the stored key is used

#### Scenario: Local server needs no key
- **WHEN** a provider is configured with `api.url: "http://127.0.0.1:11434/v1"` and no key
- **THEN** its models are available and requests carry no authorization header

#### Scenario: Remote custom provider without a key
- **WHEN** a provider is configured with `api.url: "https://llm.corp/v1"`, no key and no `auth: "none"`
- **THEN** its models are unavailable with reason `missing_credentials`
