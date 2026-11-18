## MODIFIED Requirements

### Requirement: Install script
(P0) The install script SHALL be published as the release asset `install.sh` of each `v<version>` release, reachable for the default release source at `https://github.com/CyberdyneCorp/cybercode-cli/releases/latest/download/install.sh`, and SHALL be a POSIX `sh` script that:
1. detects OS, CPU and libc (musl on Alpine)
2. downloads the matching archive
3. verifies its SHA-256 checksum (and the signature when `cosign` is available)
4. installs to `$CYBER_INSTALL_DIR` (default `~/.cyber/bin`), with the receipt `.cyber-receipt` beside the binaries

It SHALL accept `--version <v>`, `--dir <path>`, `--no-modify-path` and `--channel stable|beta|nightly`, and SHALL reject any other value of `--version` or `--channel` with exit 2. Unless `--no-modify-path` is given, it SHALL append a PATH line, preceded by the marker comment `# cyber installer: PATH`, to the first existing shell rc file for the user's shell, and SHALL not add it twice. On a checksum mismatch it SHALL abort with exit 1 without installing. `CYBER_INSTALL_BASE_URL` SHALL replace the release base URL (for mirrors and offline tests). GitHub's latest release SHALL only ever be a `v<version>` release: the release workflow SHALL mark a newer stable release latest explicitly, and SHALL publish every rolling `channel-<channel>` release, which holds only `<channel>.json`, as a prerelease not marked latest, converting an existing one on its next update. Before the first stable release that URL does not resolve, and the docs SHALL give a tagged release's script with `--channel beta` instead.

Reason for change: `https://cyber-code.dev/install` is not hosted; the script ships with each GitHub Release instead. A rolling channel release that is not a prerelease became GitHub's fallback latest (no release had been marked latest explicitly), so the `latest` URL pointed at a release without `install.sh` and returned 404. The receipt keeps a `--dir` install recognizable as a script install (→ Installation method detection), since the PATH line does not set `CYBER_INSTALL_DIR`.

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

#### Scenario: Latest install link
- **WHEN** no stable release exists, the tag `v0.1.0-beta.3` is pushed, and later the tag `v1.0.0`
- **THEN** after `v0.1.0-beta.3` both it and `channel-beta` are prereleases and GitHub has no latest release, and after `v1.0.0` the latest release is `v1.0.0`, `channel-stable` is a prerelease, and `releases/latest/download/install.sh` resolves to the `install.sh` of `v1.0.0`
