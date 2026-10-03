## MODIFIED Requirements

### Requirement: Team budgets
(P2) A Team SHALL enforce a Budget (`observability-costs`: `max_turns`, `max_tokens`, `max_cost_usd`, `max_wall_seconds`) across all members, defaulted by `budgets.team`; on exceed, teammates SHALL stop after their current Turn and the lead SHALL be notified with spend per member.

#### Scenario: Budget exceeded
- **WHEN** a team with `max_cost_usd: 20` spends $20.10
- **THEN** all teammates stop and the lead receives a spend breakdown
