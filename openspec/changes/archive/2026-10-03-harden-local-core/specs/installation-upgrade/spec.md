## MODIFIED Requirements

### Requirement: Release targets
(P0) Each release SHALL publish static binaries for the following targets:
- `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`
- `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`
- `x86_64-apple-darwin`, `aarch64-apple-darwin`
- `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`

Windows binaries SHALL be labeled preview until P1 sandbox enforcement passes its release gate. Each binary SHALL be packaged as `cyber-<version>-<target>.tar.gz` (`.zip` on Windows), with a SHA-256 checksum file and a Sigstore (cosign keyless) signature bundle.

#### Scenario: Release assets
- **WHEN** version 1.4.0 is released
- **THEN** the release contains 8 archives, `SHA256SUMS`, `SHA256SUMS.sigstore.json`, and one `.sigstore.json` bundle per archive
