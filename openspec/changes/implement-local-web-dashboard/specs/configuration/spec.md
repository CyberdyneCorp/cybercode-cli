## ADDED Requirements

### Requirement: Owner listener TLS settings
(P1) The configuration SHALL accept `server.tls.cert` and `server.tls.key` as a pair of nonempty absolute PEM file paths, read only from defaults/global user configuration. Project, profile and request overrides SHALL NOT select owner listener certificates or keys. The owner listener SHALL fail to start when its configured certificate/key cannot be loaded or do not match. Off-loopback owner listeners SHALL require TLS; loopback HTTP MAY be used directly or through an operator-managed SSH forward. Client certificate and hostname verification SHALL NOT be disabled. Certificate/private-key provisioning, trust installation, tunnel creation and public exposure remain operator responsibilities.

#### Scenario: Project cannot select owner TLS identity
- **WHEN** a project defines `server.tls` while the owner starts a listener
- **THEN** that project's values do not select the owner certificate/key

#### Scenario: Invalid configured identity
- **WHEN** a global TLS pair is incomplete, relative, unreadable or mismatched
- **THEN** the owner listener does not start and reports an actionable configuration error
