## MODIFIED Requirements

### Requirement: Permission mode ceiling
(P3) Policy SHALL support `modes.max`, ordered by how much can execute without a human decision, least to most: `plan` < `default` < `dont-ask` < `accept-edits` < `auto` < `bypass` (`dont-ask` runs only rule-allowed actions; `accept-edits` additionally runs edits). Also `modes.disable` (a list) and `modes.default`. A requested mode above the ceiling SHALL be clamped to the ceiling. `bypass` SHALL be disabled by default in any policy unless explicitly allowed.

#### Scenario: Auto mode disabled
- **WHEN** policy sets `modes.disable = ["auto"]`
- **THEN** the mode picker omits `auto`, and `--mode auto` falls back to `default` with a warning

#### Scenario: CI mode under an accept-edits ceiling
- **WHEN** policy sets `modes.max = "accept-edits"` and CI runs `cyber exec` in its default `dont-ask` Mode
- **THEN** the run proceeds in `dont-ask`, because it is below the ceiling

### Requirement: Spend limits
(P3) Policy SHALL support `spend.limit_usd` per `day`, `week` or `month`, per user and per project. Spend SHALL be computed from recorded usage costs across all local Sessions, Workflow Runs and Loops. Usage recorded with `cost: null` (unpriced) SHALL count as zero toward spend and SHALL be reported separately as unpriced tokens; `spend.block_unpriced: true` SHALL refuse Turns on unpriced models while any spend limit is configured, with `UnpricedModelBlockedError`. At 80% of a limit the user SHALL see a warning. At 100%, new Turns SHALL be refused with `SpendLimitReachedError`, except for models with an explicitly configured zero cost.

#### Scenario: Daily limit reached
- **WHEN** policy sets `spend.limit_usd.day = 20` and today's recorded cost reaches $20.00
- **THEN** the next Turn fails with `SpendLimitReachedError` and the Drain stops; local zero-cost models remain usable

#### Scenario: Unpriced model under a hard policy
- **WHEN** policy sets `spend.block_unpriced = true` and the user selects a local model with `cost: null`
- **THEN** the Turn is refused with `UnpricedModelBlockedError` and a hint to configure a price for the model
