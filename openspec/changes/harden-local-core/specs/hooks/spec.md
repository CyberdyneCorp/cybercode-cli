## MODIFIED Requirements

### Requirement: Trust for project hooks
(P1) The system SHALL require explicit user trust before running project-scope or local-scope hooks. Trust SHALL use the checkout-scoped workspace-trust store and the SHA-256 of each handler definition. A new or changed handler SHALL be skipped and reported as `untrusted` until approved via `/hooks` or `cyber hooks trust`. In `exec` mode, untrusted hooks SHALL be skipped unless `--trust-project-hooks` explicitly approves the currently inspected handler digests for that invocation, without approving future changes.

#### Scenario: Changed hook requires re-trust
- **WHEN** a teammate changes `.cyber/cyber.jsonc` hook command after the user trusted it
- **THEN** the hook is skipped and the TUI shows `1 untrusted hook changed — review with /hooks`
