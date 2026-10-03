## ADDED Requirements

### Requirement: Model fallback chain
(P1) `fallback_models` SHALL be an ordered list of model refs tried after the Session model when a Turn fails with `RateLimit`, `QuotaExceeded`, `ProviderInternal` or `Transport` after the retry policy is exhausted, or immediately with `Authentication` or `ModelDeniedByPolicyError`. The failed request SHALL be re-sent on the next available model before any assistant output or tool dispatch, with reasoning lowering applied, and a durable `session.model.fallback.1` event SHALL record the reason and both models. The Session SHALL return to its configured model after `fallback.sticky_minutes` (default 10) or on the user's next explicit model switch. The status line SHALL show the active fallback. Agents and workflow tiers MAY set their own `fallback_models`. Fallback SHALL never select a model denied by org policy.

#### Scenario: Provider overloaded
- **WHEN** the primary model returns 529 four times and `fallback_models` is `["anthropic/claude-sonnet", "openai/gpt-6"]`
- **THEN** the Turn is re-sent on `anthropic/claude-sonnet`, the event records the fallback, and the footer shows `fallback: anthropic/claude-sonnet`
