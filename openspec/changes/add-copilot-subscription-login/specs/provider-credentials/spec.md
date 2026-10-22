## ADDED Requirements

### Requirement: GitHub Copilot subscription login
(P1) The system SHALL provide `cyber providers login copilot [--enterprise <host>]`. It SHALL sign the user in with GitHub's OAuth device flow:

- **Device flow:**
  - The flow runs on `github.com`, or on the bare hostname given with `--enterprise`.
  - The CLI SHALL print the verification URL and the user code, then poll at the server's `interval`, adding 5 seconds on `slow_down`.
  - Polling SHALL stop at the code's expiry (at most 15 minutes), on `access_denied` or `expired_token`, or on Ctrl+C. In every one of those cases nothing SHALL be stored.
- **Subscription check:** after the device flow, the system SHALL mint a Copilot API token from the GitHub token. If minting fails, or names no API endpoint, the login SHALL fail with `This GitHub account has no active Copilot subscription` and SHALL store nothing.
- **Storage:** the GitHub token, host and user SHALL be stored in the owner-only file `<data>/copilot-auth.json` (mode 0600, replaced atomically; refused when accessible to other users), and SHALL be unreadable by every sandboxed process. Minted Copilot API tokens SHALL be kept only in memory and reused until 5 minutes before they expire.
- **Revoked login:** a mint rejected with HTTP 401 SHALL fail the request with an `Authentication` error that says `GitHub Copilot login expired or was revoked; run cyber providers login copilot`, and SHALL leave the stored login untouched.
- **Logout:** `cyber providers logout copilot` SHALL delete the login and the cached model list without revoking the GitHub token.
- **Windows:** login SHALL refuse on Windows until keyring storage exists.

#### Scenario: Device login
- **WHEN** the user runs `cyber providers login copilot`, opens the printed URL, enters the printed code and approves
- **THEN** `<data>/copilot-auth.json` is written with mode 0600 and the CLI prints the GitHub user and the plan's models

#### Scenario: Slow down honored
- **WHEN** the token endpoint answers a poll with `slow_down`
- **THEN** the next poll is sent at least `interval + 5` seconds later

#### Scenario: No Copilot plan
- **WHEN** the device flow succeeds but the Copilot token request answers 403
- **THEN** the login fails with `This GitHub account has no active Copilot subscription` and nothing is stored
