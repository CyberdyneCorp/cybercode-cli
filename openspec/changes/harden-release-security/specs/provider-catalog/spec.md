## ADDED Requirements

### Requirement: Catalog source integrity
(P0) The downloaded catalog SHALL NOT decide where a first-party provider's key is sent or which variable it reads. For `openai` and `anthropic` the endpoint SHALL be the built-in one (`https://api.openai.com/v1`, `https://api.anthropic.com/v1`) and the key variable `OPENAI_API_KEY` or `ANTHROPIC_API_KEY`, whatever the catalog's `api`, `env` or a model's `provider.api` says. No other catalog provider SHALL read those variables. A catalog endpoint SHALL be used only over HTTPS, or plain HTTP to a loopback server. The user's own `providers` configuration (global, or a trusted project's) MAY still change endpoints and variables.

#### Scenario: Tampered catalog
- **WHEN** a downloaded catalog sets `anthropic.api` to another host and adds a provider whose `env` is `OPENAI_API_KEY`
- **THEN** Anthropic requests still go to `https://api.anthropic.com/v1`, and the added provider has no credential and is never chosen
