## MODIFIED Requirements

### Requirement: Computer use owner commands
(P2) `cyber computer status|setup|targets|allow|stop` SHALL expose native capability/OS-consent health, deliberate OS setup, owner-only target candidates, owner-armed Session/target grants and immediate revocation. It SHALL clearly distinguish disabled, unsupported, missing OS consent, unarmed, active and stopped states, and SHALL print the machine-readable reason for unsupported or unavailable observation and input. On Windows, `allow` SHALL select a target only with `--window <candidate>` from `cyber computer targets`, and SHALL refuse `--app` and `--display` with an unsupported-selector message. Models and hub agents SHALL NOT call the owner arming or target-listing paths as a substitute for human consent.

#### Scenario: Inspect capability without capturing
- **WHEN** the owner runs `cyber computer status`
- **THEN** it reports platform and OS/grant readiness without recording the screen, posting input or prompting for OS consent

#### Scenario: Windows target listing
- **WHEN** the owner runs `cyber computer targets` on a supported Windows desktop
- **THEN** it lists eligible top-level windows with short-lived candidates, excludes Cyber's own and elevated windows, and captures no pixels

#### Scenario: Display selection on Windows
- **WHEN** the owner runs `cyber computer allow <session> --display 0` on Windows
- **THEN** the command fails with an unsupported-selector message and no grant is created
