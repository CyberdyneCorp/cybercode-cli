## ADDED Requirements

### Requirement: Self-hosted PR auto-fix execution
(P3) A reachable self-hosted server SHALL support an explicitly owner-enabled durable auto-fix subscription for an open same-repository GitHub PR. It SHALL queue deduplicated completed failing checks and review feedback from currently authorized repository writers, retaining immutable trigger and attempt identities across restart. The initial release SHALL expose offline and fork limitations and SHALL NOT imply hosted Runner failover is implemented.

#### Scenario: Duplicate failure after restart
- **WHEN** the same completed check failure is observed before and after server restart
- **THEN** at most one attempt is admitted for that trigger identity

#### Scenario: Offline server
- **WHEN** the subscribed server is unavailable
- **THEN** no local attempt runs and status SHALL identify availability rather than claim cloud execution

### Requirement: Verified and fenced auto-fix publishing
(P3) Every attempt SHALL use an isolated worktree, pinned owner verification commands and enforceable token/time limits. GitHub credentials SHALL remain outside model and verification sandboxes. The host SHALL permit only verified nonempty patches to the authorized PR branch using a non-force fast-forward push bound to the expected head. Unknown external outcomes SHALL pause and reconcile through durable receipts before further actions.

#### Scenario: Human pushes during verification
- **WHEN** the remote branch head changes before the attempted push
- **THEN** the subscription pauses without overwriting the new commits

#### Scenario: Lost push acknowledgement
- **WHEN** a push response is lost after dispatch
- **THEN** recovery checks the exact intended remote commit and does not blindly repeat the push or fix attempt

### Requirement: Auto-fix lifecycle controls and evidence
(P3) Owner API, CLI and linked-Session controls SHALL expose enable, status, pause, resume and disable. Subscriptions SHALL stop when a PR closes, pause after three consecutive failed attempts by default, and await actual exact-commit CI before declaring success or resetting failures. Status SHALL include limits, test evidence, actual usage and unknown outcomes. Unpriced dollar usage SHALL NOT be presented as zero cost.

#### Scenario: Successful push with pending checks
- **WHEN** a fix commit is pushed but its CI has not completed
- **THEN** the attempt remains awaiting CI and its failure counter is not reset

#### Scenario: Terminal closes
- **WHEN** the user closes the CLI while the subscribed server remains available
- **THEN** the durable subscription remains enabled until explicitly disabled or stopped by policy
