## MODIFIED Requirements

### Requirement: Computer use owner commands
(P2) `cyber computer status|setup|allow|stop` SHALL expose native capability/OS-consent health, deliberate OS setup, owner-armed Session/target grants and immediate revocation. It SHALL clearly distinguish disabled, unsupported, missing OS consent, unarmed, active and stopped states, and SHALL print the capability mode (`unsupported`, `capture_only` or `input_enabled`), the reason codes and the evidence level. `cyber computer allow <session> --preview <id> --yes` SHALL confirm an existing pending review by identifier without starting another review or opening a picker, and SHALL fail if that review expired, was superseded or belongs to another Session. For selectors that are not re-resolvable, `--yes` without `--preview` SHALL be refused with `interactive_review_required`. Selectors the platform does not honor SHALL be refused with `selector_unsupported_on_platform`. Models and hub agents SHALL NOT call the owner arming path as a substitute for human consent.

#### Scenario: Inspect capability without capturing
- **WHEN** the owner runs `cyber computer status`
- **THEN** it reports platform, mode, reason codes and OS/grant readiness without recording the screen, posting input or prompting for OS consent

#### Scenario: Confirm a pending review by identifier
- **WHEN** the owner runs `cyber computer allow <session> --preview <id> --yes` for a pending review before its deadline
- **THEN** exactly that reviewed target is armed and no new review starts

#### Scenario: macOS app grant unchanged
- **WHEN** the owner runs `cyber computer allow <session> --app <bundle-id> --yes` on macOS
- **THEN** the command reviews and arms the application as it did before this change
