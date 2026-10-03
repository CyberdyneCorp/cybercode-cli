## MODIFIED Requirements

### Requirement: Trust and signing
(P1) The system SHALL record trust per plugin and version digest using workspace-trust. It SHALL prompt before first enabling a plugin from project scope, and in P4 SHALL verify Sigstore signatures when the marketplace entry declares `signature`, refusing install on mismatch. Unsigned plugins SHALL be marked `unsigned` in listings.

#### Scenario: Signature mismatch
- **WHEN** a marketplace entry declares a signature that does not verify against the downloaded archive
- **THEN** install is refused with `signature verification failed`
