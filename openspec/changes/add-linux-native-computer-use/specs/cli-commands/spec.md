## MODIFIED Requirements

### Requirement: Computer use owner commands
(P2) `cyber computer status|setup|allow|stop` SHALL expose native capability/OS-consent health, deliberate OS setup, owner-armed Session/target grants and immediate revocation. It SHALL clearly distinguish disabled, unsupported, missing OS consent, unarmed, active and stopped states, and it SHALL print the platform's specific refusal codes. `cyber computer allow <session> --pick window|output` SHALL require an interactive terminal and refuse otherwise with `interactive_review_required` before any picker opens. It SHALL start owner review through the platform picker, print the selected source kind, logical size, position for outputs, the exposure warning, the review identifier and its deadline and where the owner can see the private review frame, and then ask for confirmation on the same terminal. Confirmation SHALL arm exactly that reviewed source by its review identifier and SHALL NOT start another review. The CLI SHALL NOT copy the review frame out of private storage or print credentials. `--pick` combined with `--yes` SHALL be refused. `cyber computer allow <session> --preview <id> --yes` SHALL confirm an existing pending review by identifier without opening a picker, and SHALL fail if that review expired, was superseded or belongs to another Session. `--app` and `--display` SHALL keep their existing review-then-confirm behavior on platforms that honor them, and SHALL be refused on platforms that cannot, never reinterpreted. Models and hub agents SHALL NOT call the owner arming path as a substitute for human consent.

#### Scenario: Inspect capability without capturing
- **WHEN** the owner runs `cyber computer status`
- **THEN** it reports platform and OS/grant readiness without recording the screen, posting input, prompting for OS consent or opening a portal chooser

#### Scenario: Linux owner picks a window
- **WHEN** the owner runs `cyber computer allow <session> --pick window` in a terminal on a supported Wayland session and picks a window
- **THEN** the command prints the source kind, size, review identifier, deadline and where the review frame is shown, and arms that reviewed source only after the owner confirms at the prompt before the deadline

#### Scenario: Blind pick confirmation
- **WHEN** the owner runs `cyber computer allow <session> --pick output --yes`, or runs `--pick` without a terminal
- **THEN** the command fails with `interactive_review_required`, no portal dialog opens and nothing is armed

#### Scenario: Confirm a review by identifier
- **WHEN** the owner has seen a pending pick review on the dashboard and runs `cyber computer allow <session> --preview <id> --yes` before its deadline
- **THEN** exactly that reviewed source is armed and no new portal dialog opens

#### Scenario: Bundle ID on Linux
- **WHEN** the owner runs `cyber computer allow <session> --app org.example.Editor` on Linux
- **THEN** the command fails with `selector_unsupported_on_platform` and suggests `--pick window`
