## MODIFIED Requirements

### Requirement: Release targets
(P0) Each release SHALL publish static binaries for the following targets:
- `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`
- `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`
- `x86_64-apple-darwin`, `aarch64-apple-darwin`

The Windows targets `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc` SHALL NOT be published until Windows sandbox enforcement passes its release gate; CI SHALL still build and test them on each pull request, and one switch in `scripts/release_assets.py` (`WINDOWS_RELEASED`) SHALL add their archives, npm packages and Scoop/WinGet manifests back, labeled preview. Each binary SHALL be packaged as `cyber-<version>-<target>.tar.gz` (`.zip` on Windows), with a SHA-256 checksum file and a Sigstore (cosign keyless) signature bundle. The release SHALL be produced by `.github/workflows/release.yml`, which SHALL run only when a `v<version>` tag is pushed and SHALL fail when the tag differs from the workspace version. It SHALL upload the assets to the GitHub Release `v<version>` (a prerelease unless the channel is `stable`) and replace `<channel>.json` on the rolling release `channel-<channel>` of the release source (→ Version and channel constants) only when the version is newer by semver than the one that manifest names; an older or equal tag SHALL be released without becoming the channel's (or GitHub's) latest. Releases SHALL run one at a time across tags. CI SHALL build every release target, Windows included, on each pull request, so a target that does not build is found before a tag is pushed. Signing SHALL not fail when a release has no `.zip` archive.

Reason for change: Windows confinement is still partial (sandboxed Git Bash and PowerShell cannot start in the AppContainer), so 0.1.0 ships macOS and Linux only. The release is built by the repository's GitHub workflow and published to GitHub Releases, because the project's own host does not exist yet; building every target on pull requests keeps a failing (preview) target from blocking a tag that is already pushed.

#### Scenario: Release assets
- **WHEN** version 1.4.0 is released
- **THEN** the release contains 6 archives (macOS and Linux), `SHA256SUMS`, `SHA256SUMS.sigstore.json`, one `.sigstore.json` bundle per archive and `install.sh`, and `channel-stable` holds `stable.json` with version `1.4.0`

#### Scenario: Backport release
- **WHEN** `stable.json` names 1.4.0 and the tag `v1.3.5` is pushed
- **THEN** the release `v1.3.5` is published, not marked latest, and `stable.json` still names 1.4.0

#### Scenario: Not on pull requests
- **WHEN** a pull request is opened or `main` is pushed
- **THEN** the release workflow does not run

#### Scenario: Windows is built but not released
- **WHEN** a pull request changes Windows-only code and later version 0.1.0 is tagged
- **THEN** CI builds both Windows targets for the pull request, and the 0.1.0 release has no `.zip` archive, no `win32` npm package and no Scoop or WinGet manifest

### Requirement: Package channels
(P0) Cyber Code SHALL be installable from:
- the install script (`curl`)
- the GitHub Release archives
- Cargo (`cargo install --locked --git https://github.com/CyberdyneCorp/cybercode-cli --tag v<version> cyber-cli`)
- the repository `Dockerfile` (→ Container and CI usage)

The following channels are partial until their registry accounts exist: Homebrew (`brew install cyber-code/tap/cyber`), npm (`npm i -g @cyber-code/cli`, which downloads the platform binary via optionalDependencies and lists only the published platforms), Scoop and WinGet (rendered only while Windows is released), the AUR, Nix (`nixpkgs#cyber-code`) and a Docker image `ghcr.io/cyber-code/cyber:<version>`. For each of them the repository SHALL keep the manifest or template under `packaging/`, and every release SHALL attach them rendered with that release's version and checksums, without publishing them.

Reason for change: publishing to package registries needs external accounts that do not exist yet; the manifests are kept ready so publishing is a follow-up step only.

#### Scenario: npm launcher
- **WHEN** the rendered `@cyber-code/cli` package is installed on `linux-arm64` together with the rendered `@cyber-code/cli-linux-arm64`
- **THEN** the `cyber` launcher executes the binary from `@cyber-code/cli-linux-arm64` and forwards arguments, stdio, exit code and SIGINT/SIGTERM

#### Scenario: Rendered manifests
- **WHEN** version 1.4.0 is released
- **THEN** the release carries the Homebrew formula, the AUR `PKGBUILD` and the Nix expression naming 1.4.0 and the SHA-256 values from `SHA256SUMS`
