## ADDED Requirements

### Requirement: Automation tokens
(P3) `cyber tokens create --name <label> [--scopes <list>] [--expires <duration, max 90d>]` SHALL obtain a long-lived, non-rotating access token from CyberdyneAuth for CI and headless automation (ROADMAP dependency A7), print it once, and record only its ID and label locally. `cyber tokens list|revoke <id>` SHALL manage them. The CLI and hosted services SHALL accept such a token from `CYBER_ACCOUNT_TOKEN` or `--account-token`, use it for every account-gated feature its scopes allow (sharing, routines API, Orchestrator, Relay as a headless Device), and SHALL refuse it for Device pairing, policy changes and anything requiring step-up. Token use SHALL be recorded in the account audit log with the token label.

#### Scenario: CI shares a session
- **WHEN** a CI job sets `CYBER_ACCOUNT_TOKEN` to a token with scope `cyber:share` and runs `cyber exec --share "explain"`
- **THEN** the Session is shared without any browser login and the audit log names the token

#### Scenario: Token cannot pair a device
- **WHEN** `cyber remote pair` runs with only `CYBER_ACCOUNT_TOKEN` set
- **THEN** it fails with `step-up authentication required; automation tokens cannot pair devices`
