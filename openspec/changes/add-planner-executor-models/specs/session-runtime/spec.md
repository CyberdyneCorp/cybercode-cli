## MODIFIED Requirements

### Requirement: Reasoning lowering across models
(P0) The system SHALL send historical reasoning and provider metadata (signatures, encrypted reasoning) natively only when the historical message's provider and model exactly match the Turn's model, and otherwise SHALL lower non-empty reasoning to plain assistant text and drop empty reasoning. Reasoning of a step run on the planner (`model_role: "plan"`, `provider-catalog` → Planner and executor models) SHALL be dropped, not lowered, when history is projected for a different model, so the executor receives the planner's visible answers and tool calls but not its reasoning.

#### Scenario: Reasoning after a provider switch
- **WHEN** history holds Anthropic thinking blocks and the Turn uses OpenAI
- **THEN** the thinking text is sent as ordinary assistant text without signatures

#### Scenario: Planner reasoning stays with the planner
- **WHEN** planner steps on `anthropic/claude-opus` produced thinking blocks and the executor `openai/gpt-6-mini` runs the next Turn
- **THEN** the executor's request carries the planner's text and tool calls, and neither the thinking text nor its signatures
