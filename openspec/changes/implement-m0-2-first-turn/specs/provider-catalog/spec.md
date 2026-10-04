## MODIFIED Requirements

### Requirement: Availability
(P0) The system SHALL treat a provider as available when it is not disabled, is allowed by org policy, and has a resolvable credential (or requires none), and a model as available when it is enabled, its status is not `deprecated`, and its provider is available. Models with status `alpha` SHALL be hidden unless `CYBER_ENABLE_EXPERIMENTAL_MODELS` is truthy. Unavailable models SHALL report exactly one reason, checked in this order: `provider_disabled`, `disabled`, `denied_by_policy`, `adapter_unsupported` (the provider's protocol has no native adapter in this build; P0 maps `@ai-sdk/openai` to `openai-responses`, `@ai-sdk/anthropic` to `anthropic`, and `@ai-sdk/openai-compatible` and `@openrouter/ai-sdk-provider` to `openai-compatible`), `missing_url`, `missing_credentials`, `deprecated` and `experimental`.

#### Scenario: Provider without credentials
- **WHEN** `anthropic` has no API key, OAuth credential, or environment variable
- **THEN** its models are listed as unavailable with reason `missing_credentials`

#### Scenario: Provider without a native adapter
- **WHEN** the catalog lists a provider whose SDK has no P0 adapter, such as `@ai-sdk/google`
- **THEN** its models are listed as unavailable with reason `adapter_unsupported` until the P1 adapter ships
