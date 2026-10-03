## ADDED Requirements

### Requirement: Reasoning display and effort command
(P1) `tui.reasoning` SHALL accept `collapsed` (default), `hidden` and `expanded`, and Ctrl+T SHALL still toggle per Session. `/effort <minimal|low|medium|high|xhigh|max>` SHALL switch the Session model's reasoning variant (equivalent to `/model <current>#<level>`), listing only variants the model supports and respecting the org ceiling `models.max_variant` (`org-policy`). The footer `variant` field SHALL reflect the change at the next Turn.

#### Scenario: Raise effort for a hard problem
- **WHEN** the user runs `/effort xhigh` on a model that supports it
- **THEN** the next Turn uses the `xhigh` variant and the footer shows it

#### Scenario: Effort capped by policy
- **WHEN** org policy sets `models.max_variant: "high"` and the user runs `/effort max`
- **THEN** the TUI shows `effort "max" exceeds org ceiling "high"` and keeps the current variant
