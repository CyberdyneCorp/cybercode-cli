## MODIFIED Requirements

### Requirement: Computer use owner commands
(P2) `cyber computer status|setup|allow|stop` SHALL expose native capability/OS-consent health, deliberate OS setup, owner-armed Session/target grants and immediate revocation. It SHALL clearly distinguish disabled, unsupported, missing OS consent, unarmed, active and stopped states, and it SHALL print the platform's specific refusal codes. `cyber computer allow <session> --pick window|output` SHALL start owner review through the platform picker, show the actual selected source before arming, and accept `--yes` only to confirm that reviewed source. `--app` and `--display` SHALL be refused on platforms that cannot honor them, never reinterpreted. Models and hub agents SHALL NOT call the owner arming path as a substitute for human consent.

#### Scenario: Inspect capability without capturing
- **WHEN** the owner runs `cyber computer status`
- **THEN** it reports platform and OS/grant readiness without recording the screen, posting input, prompting for OS consent or opening a portal chooser

#### Scenario: Linux owner picks a window
- **WHEN** the owner runs `cyber computer allow <session> --pick window` on a supported Wayland session
- **THEN** the portal dialog appears, the command prints the selected source kind, size and a private review-frame reference, and nothing is armed until the owner confirms

#### Scenario: Bundle ID on Linux
- **WHEN** the owner runs `cyber computer allow <session> --app org.example.Editor` on Linux
- **THEN** the command fails with `selector_unsupported_on_platform` and suggests `--pick window`
