## MODIFIED Requirements

### Requirement: Release targets
(P0) Each release SHALL publish static binaries for the following targets:
- `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`
- `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`
- `x86_64-apple-darwin`, `aarch64-apple-darwin`
- `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`

Windows binaries SHALL be labeled preview until P1 sandbox enforcement passes its release gate. Each binary SHALL be packaged as `cyber-<version>-<target>.tar.gz` (`.zip` on Windows), with a SHA-256 checksum file and a Sigstore (cosign keyless) signature bundle. The release SHALL be produced by `.github/workflows/release.yml`, which SHALL run only when a `v<version>` tag is pushed and SHALL fail when the tag differs from the workspace version. It SHALL upload the assets to the GitHub Release `v<version>` (a prerelease unless the channel is `stable`) and replace `<channel>.json` on the rolling release `channel-<channel>` of the release source (→ Version and channel constants) only when the version is newer by semver than the one that manifest names; an older or equal tag SHALL be released without becoming the channel's (or GitHub's) latest. Releases SHALL run one at a time across tags. CI SHALL build every release target on each pull request, so a target that does not build is found before a tag is pushed.

Reason for change: the release is built by the repository's GitHub workflow and published to GitHub Releases, because the project's own host does not exist yet; building every target on pull requests keeps a failing (preview) target from blocking a tag that is already pushed.

#### Scenario: Release assets
- **WHEN** version 1.4.0 is released
- **THEN** the release contains 8 archives, `SHA256SUMS`, `SHA256SUMS.sigstore.json`, one `.sigstore.json` bundle per archive and `install.sh`, and `channel-stable` holds `stable.json` with version `1.4.0`

#### Scenario: Backport release
- **WHEN** `stable.json` names 1.4.0 and the tag `v1.3.5` is pushed
- **THEN** the release `v1.3.5` is published, not marked latest, and `stable.json` still names 1.4.0

#### Scenario: Not on pull requests
- **WHEN** a pull request is opened or `main` is pushed
- **THEN** the release workflow does not run

### Requirement: Install script
(P0) The install script SHALL be published as the release asset `install.sh`, reachable for the default release source at `https://github.com/CyberdyneCorp/cybercode-cli/releases/latest/download/install.sh`, and SHALL be a POSIX `sh` script that:
1. detects OS, CPU and libc (musl on Alpine)
2. downloads the matching archive
3. verifies its SHA-256 checksum (and the signature when `cosign` is available)
4. installs to `$CYBER_INSTALL_DIR` (default `~/.cyber/bin`), with the receipt `.cyber-receipt` beside the binaries

It SHALL accept `--version <v>`, `--dir <path>`, `--no-modify-path` and `--channel stable|beta|nightly`, and SHALL reject any other value of `--version` or `--channel` with exit 2. Unless `--no-modify-path` is given, it SHALL append a PATH line, preceded by the marker comment `# cyber installer: PATH`, to the first existing shell rc file for the user's shell, and SHALL not add it twice. On a checksum mismatch it SHALL abort with exit 1 without installing. `CYBER_INSTALL_BASE_URL` SHALL replace the release base URL (for mirrors and offline tests).

Reason for change: `https://cyber-code.dev/install` is not hosted; the script ships with each GitHub Release instead. The receipt keeps a `--dir` install recognizable as a script install (→ Installation method detection), since the PATH line does not set `CYBER_INSTALL_DIR`.

#### Scenario: Custom directory
- **WHEN** the user runs `sh install.sh --dir ~/bin` and later runs `~/bin/cyber upgrade` and `~/bin/cyber uninstall --force` without `CYBER_INSTALL_DIR`
- **THEN** the method is `script`, and the uninstall removes `cyber`, `cyber-sandbox-exec` and the receipt from `~/bin`

#### Scenario: Checksum mismatch
- **WHEN** the downloaded archive's SHA-256 differs from `SHA256SUMS`
- **THEN** the script prints `checksum verification failed` and exits 1, leaving any existing install untouched

#### Scenario: Already installed
- **WHEN** `cyber --version` on PATH already reports the requested version
- **THEN** the script prints `cyber <v> already installed` and exits 0 without downloading

#### Scenario: Offline install from a mirror
- **WHEN** `CYBER_INSTALL_BASE_URL` points at a local HTTP server holding a release and the user runs `sh install.sh --version 1.4.0 --dir /tmp/c --no-modify-path`
- **THEN** `/tmp/c/cyber --version` reports 1.4.0 and no rc file changes

### Requirement: Package channels
(P0) Cyber Code SHALL be installable from:
- the install script (`curl`)
- the GitHub Release archives
- Cargo (`cargo install --locked --git https://github.com/CyberdyneCorp/cybercode-cli --tag v<version> cyber-cli`)
- the repository `Dockerfile` (→ Container and CI usage)

The following channels are partial until their registry accounts exist: Homebrew (`brew install cyber-code/tap/cyber`), npm (`npm i -g @cyber-code/cli`, which downloads the platform binary via optionalDependencies), Scoop, WinGet, the AUR, Nix (`nixpkgs#cyber-code`) and a Docker image `ghcr.io/cyber-code/cyber:<version>`. For each of them the repository SHALL keep the manifest or template under `packaging/`, and every release SHALL attach them rendered with that release's version and checksums, without publishing them.

Reason for change: publishing to package registries needs external accounts that do not exist yet; the manifests are kept ready so publishing is a follow-up step only.

#### Scenario: npm launcher
- **WHEN** the rendered `@cyber-code/cli` package is installed on `linux-arm64` together with the rendered `@cyber-code/cli-linux-arm64`
- **THEN** the `cyber` launcher executes the binary from `@cyber-code/cli-linux-arm64` and forwards arguments, stdio, exit code and SIGINT/SIGTERM

#### Scenario: Rendered manifests
- **WHEN** version 1.4.0 is released
- **THEN** the release carries the Homebrew formula, Scoop and WinGet manifests and AUR `PKGBUILD` naming 1.4.0 and the SHA-256 values from `SHA256SUMS`

### Requirement: Installation method detection
(P0) The system SHALL detect how the running binary was installed:
- `script`, when its path is under `~/.cyber/bin` or `$CYBER_INSTALL_DIR`, or the install script's receipt `.cyber-receipt` is a regular file beside it
- otherwise `brew`, `npm`, `pnpm`, `bun`, `cargo`, `scoop`, `winget`, `aur`, `nix` or `docker`, from the executable path and package-manager listings, trying managers named in the path first; each listing query SHALL time out after 3 seconds and SHALL run without a shell
- else `unknown`

A method known from a package-manager listing alone SHALL NOT be acted on unattended: the listing shows a package is installed somewhere, not that the running binary came from it.

Reason for change: a listing can name another install than the running binary, and a `--dir` install must stay a script install without the environment variable.

#### Scenario: Listing only
- **WHEN** the binary is `/usr/local/bin/cyber` built from source and `brew list --versions cyber` lists a package
- **THEN** the method is `brew`, known from the listing alone

#### Scenario: Homebrew detection
- **WHEN** the binary path is `/opt/homebrew/Cellar/cyber/1.4.0/bin/cyber`
- **THEN** the method is `brew`

#### Scenario: Script detection with a custom directory
- **WHEN** `CYBER_INSTALL_DIR=/opt/tools/cyber` and the binary is `/opt/tools/cyber/cyber`
- **THEN** the method is `script`

### Requirement: Version and channel constants
(P0) The build SHALL embed the version (semver), the channel (`stable`, `beta`, `nightly` or `dev` for local builds), the git SHA, the target triple and a bundled model-catalog snapshot. A build is a preview when its channel is not `stable`. The build SHALL also embed one release source: the base URL under which `v<version>/` holds the archives and `SHA256SUMS`, and `channel-<channel>/<channel>.json` holds the channel manifest, plus the expected Sigstore certificate identity prefix and OIDC issuer. The defaults SHALL be `https://github.com/CyberdyneCorp/cybercode-cli/releases/download`, `https://github.com/CyberdyneCorp/cybercode-cli/.github/workflows/release.yml@refs/tags/` and `https://token.actions.githubusercontent.com`. Only build-time variables (`CYBER_RELEASE_BASE_URL`, `CYBER_RELEASE_IDENTITY`, `CYBER_RELEASE_ISSUER`) SHALL change them; no config key or runtime variable of a release build SHALL redirect lookups or change the trusted identity. Debug builds SHALL accept the documented test hook `CYBER_TEST_RELEASE_BASE_URL` and `CYBER_TEST_RELEASE_TRUST`, which release builds SHALL ignore.

Reason for change: the release source was implied as `cyber-code.dev`; it is now one explicit, build-time constant set that defaults to the real repository's GitHub Releases.

#### Scenario: Local build
- **WHEN** the binary is built with `cargo build` without release metadata
- **THEN** `cyber --version` reports channel `dev`, and auto-update is disabled

#### Scenario: Release build ignores the test hook
- **WHEN** a release build runs `cyber upgrade` with `CYBER_TEST_RELEASE_BASE_URL=http://127.0.0.1:9`
- **THEN** it looks up the default release source and never contacts `127.0.0.1:9`

### Requirement: Latest version lookup
(P0) The system SHALL resolve the latest version per channel from `<base_url>/channel-<channel>/<channel>.json` of the release source (fields `version`, `published_at`, `notes_url`, `min_supported`), with a 10 second timeout and 2 retries, caching the result for 1 hour in `<cache>/release-<channel>.json`. With `CYBER_OFFLINE` set it SHALL make no request and use the cache regardless of age, failing with `offline: no cached release information` when there is none. A `dev` build SHALL look up `stable`.

Reason for change: the manifest is served from the release source instead of `cyber-code.dev`.

#### Scenario: Cached lookup
- **WHEN** the release manifest was fetched 20 minutes ago
- **THEN** no network request is made and the cached version is used

#### Scenario: Offline lookup
- **WHEN** `CYBER_OFFLINE=1` and the cache is three days old
- **THEN** no network request is made and the cached version is used

### Requirement: Upgrade command
(P0) `cyber upgrade [version] [--method <m>] [--channel <c>] [--dry-run]` SHALL upgrade with the detected method:
- `script`: download, verify (→ Self-update integrity), and atomically replace the binary and its sandbox helper via rename, keeping `cyber.old` until next start
- `brew upgrade cyber`
- `npm i -g @cyber-code/cli@<v>` (`pnpm add -g`, `bun add -g` for those methods)
- `cargo install --locked --git https://github.com/CyberdyneCorp/cybercode-cli --tag v<v> cyber-cli`
- `scoop update cyber`
- `winget upgrade cyber-code.cyber`

For `nix`, `aur`, `docker` and `unknown` it SHALL print the manual command instead. It SHALL skip with a notice when the target equals the installed version. A version taken from the channel manifest (no `version` argument) SHALL be installed only when it is newer by semver than the running version; otherwise the command SHALL change nothing and say so. Only a named version SHALL downgrade. With `CYBER_OFFLINE` set, a plan that downloads or runs a package manager SHALL fail with `offline: ...` and exit 1 before any request. A package-manager command SHALL run without a shell, and on failure the CLI SHALL print the command and exit 1. When the method is known from a listing alone and `--method` is not given, the command SHALL print the manager's command instead of running it (action `manual`) and SHALL NOT report the binary as upgraded. `--dry-run` SHALL resolve the target version and method and print the action it would take (JSON `dry_run: true`) without downloading anything beyond the channel manifest, running a manager or changing any file.

Reason for change: the Cargo channel installs from the repository until a crate is published. The channel manifest is not signed, so it must not be able to roll an install back to an older signed release, and `CYBER_OFFLINE` covers the downloads as well as the lookup. A listed package may be another install, which the upgrade would change while the running binary stays as it is. `--dry-run` lets a user see the target and method before anything is replaced.

#### Scenario: Dry-run upgrade
- **WHEN** the user runs `cyber upgrade --dry-run` on a `script` install with a newer release on the channel
- **THEN** it prints `cyber <from> → <to> via script (dry run: ...)`, fetches only the channel manifest, and leaves the binary unchanged

#### Scenario: Listing-only method
- **WHEN** brew lists a cyber package, the running binary is outside every manager's path, and the user runs `cyber upgrade`
- **THEN** the command prints `brew lists a cyber package, but this binary is not from it; to upgrade that package, run: brew upgrade cyber (or "cyber upgrade --method brew" to run it)`, runs no manager command and exits 0

#### Scenario: Atomic self-replace
- **WHEN** `cyber upgrade` runs on a `script` install and the download is interrupted
- **THEN** the existing binary is unchanged and still runs

#### Scenario: Unsupported method
- **WHEN** the method is `nix`
- **THEN** the command prints `installed via nix; run: nix profile upgrade cyber-code` and exits 0

#### Scenario: Already current
- **WHEN** the channel manifest's version equals the running version
- **THEN** the command prints `cyber <v> is already installed` and changes nothing

#### Scenario: Older channel manifest
- **WHEN** 1.4.0 is running and the channel manifest names 1.3.5
- **THEN** `cyber upgrade` reports action `current`, downloads nothing and leaves the binary byte-identical, while `cyber upgrade 1.3.5` installs 1.3.5

#### Scenario: Offline upgrade
- **WHEN** `CYBER_OFFLINE=1` and the user runs `cyber upgrade 1.5.0` on a `script` install
- **THEN** the command exits 1 with `offline: installing cyber 1.5.0 needs the network` and makes no request

### Requirement: Background update policy
(P0) About 5 seconds after the server starts, and then every 24 hours, the server SHALL check for updates unless `autoupdate = false`, `CYBER_DISABLE_AUTOUPDATE` is set, `CYBER_OFFLINE` is set, `CI` is true, the channel is `dev`, or org policy (managed configuration, `policy.autoupdate`) disables it. For `autoupdate = "notify"`, or any minor or major bump, it SHALL publish the live-only event `installation.update_available.1`. For patch bumps with `autoupdate = true` on a `script`/`brew`/`npm` install that the binary's path identifies it SHALL upgrade silently and publish the live-only event `installation.updated.1`; a failed silent upgrade SHALL publish `installation.update_available.1` instead. After one silent upgrade, the server SHALL only announce later releases until it restarts, since its running build no longer matches the installed binary; the binary it replaces SHALL be the path resolved when the server started. A configuration that fails to load SHALL skip that check, never fall back to defaults that drop org policy. The default SHALL be `"notify"`. `GET /api/v1/service/update` SHALL return the last result (current and latest version, bump kind, method, notes URL, `min_supported`, whether the running version is unsupported, whether a silent upgrade installed the latest version, and the check time). The server SHALL never restart itself to apply an update.

Reason for change: the check runs in the server for every client that started it, and the last result is readable by clients that attach later. A long-lived server must not install a second release over the renamed `cyber.old`, nor ignore org policy when the user's config is broken, nor change a package-manager install that the listing found but the running binary is not.

#### Scenario: Notify by default
- **WHEN** 1.4.0 is installed with default config and 1.5.0 is released
- **THEN** the TUI shows `cyber 1.5.0 is available — run "cyber upgrade"`

#### Scenario: Listing-only method is only announced
- **WHEN** `autoupdate = true`, a patch release exists, and only brew's listing names the method of a binary outside every manager's path
- **THEN** the server publishes `installation.update_available.1`, `GET /api/v1/service/update` reports `updated: false`, and no manager command runs

#### Scenario: Disabled in CI
- **WHEN** the server starts with `CI=true`
- **THEN** no release lookup is made and `GET /api/v1/service/update` reports `latest: null`

### Requirement: Minimum supported version
(P0) When the cached release manifest's `min_supported` is greater than the running version, the TUI and `exec` SHALL print a prominent warning on every start, `warning: cyber <v> is no longer supported (minimum <m>); run "cyber upgrade"`, without waiting for the network. (P3) Hosted services (Relay, Runners, Share) MAY reject the client with HTTP 426 and an `upgrade_required` body.

Reason for change: the warning reads the cached manifest so startup never waits on the network; the HTTP 426 behavior belongs to the P3 hosted services.

#### Scenario: Unsupported version warning
- **WHEN** 1.0.0 is installed and the cached manifest has `min_supported` 1.2.0
- **THEN** `cyber exec "hi"` prints the warning to stderr before the run, and the TUI shows it as a banner

#### Scenario: Relay rejects an old client
- **WHEN** (P3) a 1.0.0 client connects to the Relay and `min_supported` is 1.2.0
- **THEN** the Relay responds 426, and the CLI prints `Error: Cyber Cloud requires cyber >= 1.2.0` with exit code 1

### Requirement: Uninstall command
(P0) `cyber uninstall` SHALL list what will be removed with sizes: the binary (for `script`), PATH lines carrying the installer marker in shell rc files, and the data, config, state and cache directories. It SHALL ask for confirmation unless `--force` is given, and without a terminal it SHALL exit 2 unless `--force` is given. `--keep-config` and `--keep-data` SHALL preserve those directories, and `--dry-run` SHALL change nothing. For package-manager installs it SHALL list that manager's uninstall command with the entries before asking, run it, and print it on failure; it SHALL run it only when the binary's own path shows that manager installed it, and otherwise SHALL print it as a note to run by hand. Before removing data it SHALL stop the background server and delete the keyring entries cyber stored (provider, MCP OAuth, peer and plugin credentials). It SHALL never follow a symlink, SHALL refuse a directory that is not one of the resolved cyber directories, is a symlink, is the home directory or one of its ancestors, or is a filesystem root, and SHALL remove only the marked PATH lines.

Reason for change: the safety limits of the deletion are now explicit, and the confirmation covers every action it approves: a manager's listing shows a package is installed somewhere, not that the running binary came from it.

#### Scenario: Keep data
- **WHEN** the user runs `cyber uninstall --keep-data --force`
- **THEN** `~/.local/share/cyber` remains and everything else listed is removed

#### Scenario: Symlink out of a cyber directory
- **WHEN** `~/.cache/cyber/link` is a symlink to `~/projects` and the user runs `cyber uninstall --force`
- **THEN** the link is removed and `~/projects` is untouched

#### Scenario: Relocated directory refused
- **WHEN** `CYBER_CONFIG_DIR` is the user's home directory and the user runs `cyber uninstall --force`
- **THEN** the command refuses to remove the home directory and exits 1 without deleting anything

### Requirement: Self-update integrity
(P0) Every in-place upgrade SHALL verify the downloaded artifact's SHA-256 against a `SHA256SUMS` whose Sigstore bundle validates, against the trusted root embedded in the binary, for the release source's identity `<identity prefix>v<version>` (by default `https://github.com/CyberdyneCorp/cybercode-cli/.github/workflows/release.yml@refs/tags/v<version>`) and issuer (by default `https://token.actions.githubusercontent.com`), including the certificate chain, the transparency-log inclusion and the SCT. On failure it SHALL abort without replacing the binary, printing `signature verification failed` or `checksum verification failed`.

Reason for change: the identity names the real repository and its release workflow instead of `github.com/cyber-code/cyber`.

#### Scenario: Forged manifest
- **WHEN** an attacker serves a modified `SHA256SUMS` without a valid Sigstore signature
- **THEN** `cyber upgrade` fails with `signature verification failed` and the binary is unchanged

#### Scenario: Tampered archive
- **WHEN** `SHA256SUMS` verifies but the archive's SHA-256 differs from its line
- **THEN** `cyber upgrade` fails with `checksum verification failed` and the binary is unchanged

#### Scenario: Signature for another version
- **WHEN** the `SHA256SUMS` bundle of 1.4.0 is served for the requested 1.5.0
- **THEN** the identity does not match `refs/tags/v1.5.0` and `cyber upgrade` fails with `signature verification failed`

### Requirement: Container and CI usage
(P0) The repository `Dockerfile` SHALL build an image that runs as a non-root user `cyber` (uid 10001), sets `CYBER_HOME=/home/cyber/.cyber`, includes `git` and `ripgrep`, and defaults the entrypoint to `cyber`. With `CI=true`, the CLI SHALL disable auto-update, the onboarding prompts and the TUI (every command that opens it, `cyber attach` included), and default `exec` to `--format text`.

Reason for change: the image is defined by the repository `Dockerfile`; publishing it to a registry is a partial package channel.

#### Scenario: CI defaults
- **WHEN** `CI=true` and the user runs `cyber` with no arguments, or `cyber attach <session>@<peer>`
- **THEN** the CLI prints `interactive TUI unavailable in CI; use "cyber exec"` and exits 2
