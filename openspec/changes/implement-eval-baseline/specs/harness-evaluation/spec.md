## MODIFIED Requirements

### Requirement: Reproducible evaluation manifest
(P0) The repository SHALL contain versioned evaluation manifests identifying fixture repository commit, setup image digest or environment lock, task prompt, harness version, model ID and provider, decoding options, tool schemas, budgets, timeout and grading rules. Deterministic adapter fixtures SHALL be separate from live-provider runs. A `suite` manifest SHALL give every task its own fixture and SHALL NOT name a model; the model is chosen when the suite runs and the report SHALL record it, so one suite measures every provider. Live runs SHALL record resolved model metadata and timestamp and SHALL NOT claim deterministic model responses.

#### Scenario: Compare harness versions
- **WHEN** two harness revisions run the same evaluation manifest
- **THEN** the report identifies every changed harness, model, environment or prompt input

#### Scenario: One suite, two providers
- **WHEN** `cyber eval run eval/manifests/suite-coding-v1.json -m openai/gpt-6-luna` and the same command with `-m anthropic/claude-haiku-4-5` run
- **THEN** each report records its resolved model, the manifest hash and the harness SHA, and both use the same task fixtures and graders
