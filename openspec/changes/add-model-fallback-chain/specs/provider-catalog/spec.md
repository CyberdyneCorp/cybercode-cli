## MODIFIED Requirements

### Requirement: Model fallback chain
(P1) `fallback_models` SHALL be an ordered list of model refs tried after the Session model when a Turn fails before any output with `RateLimit`, `QuotaExceeded`, `ProviderInternal` or `Transport` after the retry policy is exhausted, or immediately with `Authentication` or `ModelDeniedByPolicyError`. Other kinds (`ContextOverflow`, `InvalidRequest`, `ContentPolicy`) SHALL NOT fall back, and a stream that fails after output SHALL follow the stream retry rules instead. The failed step SHALL be recorded, and the Turn SHALL be re-sent on the next model of the list after the failed one that resolves, skipping the Session's configured model, with reasoning lowering applied. A durable `session.model.fallback.1` event, committed with the failed step, SHALL record `from`, `to`, the error `kind` and `reason`, `started_ms` and `until_ms`. A fallback that fails moves on to the next model; when the list is used up the Turn SHALL fail with the last error, and only then SHALL a usage limit wait be considered. The Session SHALL use the fallback until `until_ms`, which is `fallback.sticky_minutes` (1 to 1440, default 10) after it started, then return to its configured model at the next Turn; the user's next explicit model switch, including to the configured model, SHALL end it at once. The Session view SHALL report the active fallback as `fallback`, and the TUI SHALL announce it and show `fallback: <model>` in the status line while it lasts. `fallback_models` and `fallback` are trust-gated like other routing keys, so an untrusted checkout cannot set them. Agents and workflow tiers MAY set their own `fallback_models`. Fallback SHALL never select a model denied by org policy.

#### Scenario: Provider overloaded
- **WHEN** the primary model returns 529 four times and `fallback_models` is `["anthropic/claude-sonnet", "openai/gpt-6"]`
- **THEN** the Turn is re-sent on `anthropic/claude-sonnet`, the event records the fallback, and the footer shows `fallback: anthropic/claude-sonnet`

#### Scenario: The chain moves on
- **WHEN** the primary fails with `Authentication` and `anthropic/claude-sonnet` then fails with `QuotaExceeded`
- **THEN** the Turn is re-sent on `openai/gpt-6`, two `session.model.fallback.1` events record both hops, and no usage limit wait starts

#### Scenario: Return to the configured model
- **WHEN** `fallback.sticky_minutes` is `5` and the user sends a prompt six minutes after the fallback started
- **THEN** that Turn runs on the Session's configured model and the footer no longer shows the fallback

#### Scenario: No fallback for errors another model would repeat
- **WHEN** a Turn fails with `ContextOverflow` and `fallback_models` is set
- **THEN** no fallback model is tried and the overflow handling applies
