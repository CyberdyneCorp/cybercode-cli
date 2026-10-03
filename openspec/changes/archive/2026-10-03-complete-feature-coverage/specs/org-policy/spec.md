## ADDED Requirements

### Requirement: Feature and effort ceilings
(P3) Policy SHALL support `features.lock` (a map of feature name → value that users cannot change, reported by `cyber features list` with source `policy`) and `models.max_variant` (the highest reasoning variant members may select, ordered `minimal < low < medium < high < xhigh < max`; a higher request is clamped with a notice). Policy SHALL also support `plugins.deny_capabilities` (a list of plugin capabilities such as `provider.intercept` that may not be granted).

#### Scenario: Locked feature
- **WHEN** policy sets `features.lock = { "teams": false }` and a user sets `features.teams: true`
- **THEN** teams stay disabled and `cyber features list` shows `teams off (policy: org acme)`
