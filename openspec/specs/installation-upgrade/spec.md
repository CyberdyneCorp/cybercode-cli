# installation-upgrade Specification

## Purpose
Covers how `cyber` is distributed, installed, verified, kept up to date and removed on every supported platform and channel. Cyber Code ships as one static Rust binary, like Codex, so installs are a single verified file. The channel detection, `upgrade`/`uninstall` commands and auto-update policy follow OpenCode v1. Users on any channel can upgrade or remove the tool with one command, and every artifact is signed so supply-chain tampering is detectable.

## Requirements

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

### Requirement: Install script
(P0) `https://cyber-code.dev/install` SHALL be a POSIX `sh` script that:
1. detects OS, CPU and libc (musl on Alpine)
2. downloads the matching archive
3. verifies its SHA-256 checksum (and the signature when `cosign` is available)
4. installs to `$CYBER_INSTALL_DIR` (default `~/.cyber/bin`)

It SHALL accept `--version <v>`, `--dir <path>`, `--no-modify-path` and `--channel stable|beta|nightly`. Unless `--no-modify-path` is given, it SHALL append a PATH line to the first existing shell rc file for the user's shell. On a checksum mismatch it SHALL abort with exit 1 without installing.

#### Scenario: Checksum mismatch
- **WHEN** the downloaded archive's SHA-256 differs from `SHA256SUMS`
- **THEN** the script prints `checksum verification failed` and exits 1, leaving any existing install untouched

#### Scenario: Already installed
- **WHEN** `cyber --version` on PATH already reports the requested version
- **THEN** the script prints `cyber <v> already installed` and exits 0 without downloading

### Requirement: Package channels
(P0) Cyber Code SHALL be installable from:
- the install script (`curl`)
- Homebrew (`brew install cyber-code/tap/cyber`)
- npm (`npm i -g @cyber-code/cli`, which downloads the platform binary via optionalDependencies)
- Cargo (`cargo install cyber-code`)
- Scoop, WinGet, the AUR, and Nix (`nixpkgs#cyber-code`)
- a Docker image `ghcr.io/cyber-code/cyber:<version>`

#### Scenario: npm launcher
- **WHEN** a user installs `@cyber-code/cli` on `linux-arm64`
- **THEN** the `cyber` launcher executes the binary from `@cyber-code/cli-linux-arm64` and forwards arguments, stdio, exit code and SIGINT/SIGTERM

### Requirement: Installation method detection
(P0) The system SHALL detect how the running binary was installed:
- `script`, when its path is under `~/.cyber/bin` or `$CYBER_INSTALL_DIR`
- otherwise `brew`, `npm`, `pnpm`, `bun`, `cargo`, `scoop`, `winget`, `aur`, `nix` or `docker`, from the executable path and package-manager listings, trying managers named in the path first
- else `unknown`

#### Scenario: Homebrew detection
- **WHEN** the binary path is `/opt/homebrew/Cellar/cyber/1.4.0/bin/cyber`
- **THEN** the method is `brew`

### Requirement: Version and channel constants
(P0) The build SHALL embed the version (semver), the channel (`stable`, `beta`, `nightly` or `dev` for local builds), the git SHA, the target triple and a bundled model-catalog snapshot. A build is a preview when its channel is not `stable`.

#### Scenario: Local build
- **WHEN** the binary is built with `cargo build` without release metadata
- **THEN** `cyber --version` reports channel `dev`, and auto-update is disabled

### Requirement: Latest version lookup
(P0) The system SHALL resolve the latest version per channel from `https://cyber-code.dev/releases/<channel>.json` (fields `version`, `published_at`, `notes_url`, `min_supported`), with a 10 second timeout and 2 retries, caching the result for 1 hour in `<cache>/release-<channel>.json`.

#### Scenario: Cached lookup
- **WHEN** the release manifest was fetched 20 minutes ago
- **THEN** no network request is made and the cached version is used

### Requirement: Upgrade command
(P0) `cyber upgrade [version] [--method <m>] [--channel <c>]` SHALL upgrade with the detected method:
- `script`: download, verify, and atomically replace the binary via rename, keeping `cyber.old` until next start
- `brew upgrade cyber`
- `npm i -g @cyber-code/cli@<v>`
- `cargo install cyber-code --version <v>`
- `scoop update cyber`
- `winget upgrade cyber-code.cyber`

For `nix`, `aur`, `docker` and `unknown` it SHALL print the manual command instead. It SHALL skip with a notice when the target equals the installed version.

#### Scenario: Atomic self-replace
- **WHEN** `cyber upgrade` runs on a `script` install and the download is interrupted
- **THEN** the existing binary is unchanged and still runs

#### Scenario: Unsupported method
- **WHEN** the method is `nix`
- **THEN** the command prints `installed via nix; run: nix profile upgrade cyber-code` and exits 0

### Requirement: Background update policy
(P0) About 5 seconds after the TUI starts, the server SHALL check for updates unless `autoupdate = false`, `CYBER_DISABLE_AUTOUPDATE` is set, `CYBER_OFFLINE` is set, the channel is `dev`, or org policy disables it. For `autoupdate = "notify"`, or any minor or major bump, it SHALL publish `installation.update_available.1`. For patch bumps with `autoupdate = true` on a `script`/`brew`/`npm` install it SHALL upgrade silently and publish `installation.updated.1`. The default SHALL be `"notify"`.

#### Scenario: Notify by default
- **WHEN** 1.4.0 is installed with default config and 1.5.0 is released
- **THEN** the TUI shows `cyber 1.5.0 is available — run "cyber upgrade"`

### Requirement: Minimum supported version
(P0) When the release manifest's `min_supported` is greater than the running version, the TUI and `exec` SHALL print a prominent warning on every start. Hosted services (Relay, Runners, Share) MAY reject the client with HTTP 426 and an `upgrade_required` body.

#### Scenario: Relay rejects an old client
- **WHEN** a 1.0.0 client connects to the Relay and `min_supported` is 1.2.0
- **THEN** the Relay responds 426, and the CLI prints `Error: Cyber Cloud requires cyber >= 1.2.0` with exit code 1

### Requirement: Uninstall command
(P0) `cyber uninstall` SHALL list what will be removed with sizes: the binary (for `script`), PATH lines in shell rc files, and the data, config, state and cache directories. It SHALL ask for confirmation unless `--force` is given. `--keep-config` and `--keep-data` SHALL preserve those directories, and `--dry-run` SHALL change nothing. For package-manager installs it SHALL run that manager's uninstall command and print it on failure. Before removing data it SHALL stop the background server and delete stored account tokens from the keyring.

#### Scenario: Keep data
- **WHEN** the user runs `cyber uninstall --keep-data --force`
- **THEN** `~/.local/share/cyber` remains and everything else listed is removed

### Requirement: Self-update integrity
(P0) Every in-place upgrade SHALL verify the downloaded artifact's SHA-256 against a `SHA256SUMS` whose Sigstore bundle validates against the identity `https://github.com/cyber-code/cyber/.github/workflows/release.yml@refs/tags/v<version>`. On failure it SHALL abort without replacing the binary.

#### Scenario: Forged manifest
- **WHEN** an attacker serves a modified `SHA256SUMS` without a valid Sigstore signature
- **THEN** `cyber upgrade` fails with `signature verification failed` and the binary is unchanged

### Requirement: Shell integration on install
(P1) The first interactive run SHALL offer, once, to install shell completion for the detected shell and to register the `cyber://` URL scheme handler for deep links. It SHALL record the answer in `<state>/onboarding.json`.

#### Scenario: Declined onboarding
- **WHEN** the user declines completion installation
- **THEN** the offer is not shown again

### Requirement: Container and CI usage
(P0) The Docker image SHALL run as a non-root user `cyber` (uid 10001), set `CYBER_HOME=/home/cyber/.cyber`, include `git` and `ripgrep`, and default the entrypoint to `cyber`. With `CI=true`, the CLI SHALL disable auto-update, the onboarding prompts and the TUI, and default `exec` to `--format text`.

#### Scenario: CI defaults
- **WHEN** `CI=true` and the user runs `cyber` with no arguments
- **THEN** the CLI prints `interactive TUI unavailable in CI; use "cyber exec"` and exits 2
