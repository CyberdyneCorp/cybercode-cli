# Implement P0 installation and upgrade

## Why

Twelve P0 requirements of `installation-upgrade` and the cli-commands row "Upgrade and uninstall commands" have no implementation. There is no release workflow (`ci.yml` cross-builds six targets but publishes nothing), no install script, no `cyber upgrade` or `cyber uninstall`, and no update check. Today the only way to get `cyber` is to build it from source, and nothing tells a user that a newer or safer build exists.

The canonical spec names `https://cyber-code.dev` and the Sigstore identity `github.com/cyber-code/cyber`. Neither exists. The project lives at `github.com/CyberdyneCorp/cybercode-cli` and can release through GitHub Releases today. This change points every release lookup at one build-time release source that defaults to those GitHub Releases. The spec deltas below modify the requirements that name the old host or identity.

## What Changes

- **Release source** (`installation-upgrade`, modified "Version and channel constants"): one build-time constant set holds the release base URL (channel manifests and artifacts) and the expected Sigstore certificate identity and issuer. It defaults to GitHub Releases of `CyberdyneCorp/cybercode-cli` and `.github/workflows/release.yml@refs/tags/v<version>`. Debug builds alone accept a documented test hook that points the base URL and trust root at a local stub. Release builds compile the hook out.
- **Release packaging** (modified "Release targets"): `.github/workflows/release.yml` runs only on a `v*` tag push. It builds the eight targets and packages `cyber-<version>-<target>.tar.gz` (`.zip` on Windows). It writes `SHA256SUMS`, cosign keyless bundles for `SHA256SUMS` and each archive, and the channel manifest `<channel>.json`, then uploads everything to the GitHub Release. It also renders the package-manager manifests as release assets without publishing them.
- **Install script** (modified "Install script"): `scripts/install.sh`, a POSIX `sh` script attached to each release. It detects OS, CPU and libc, verifies SHA-256 (and the bundle when `cosign` is on PATH), installs atomically, and supports `--version`, `--dir`, `--no-modify-path` and `--channel`. Its PATH line carries a marker that uninstall removes.
- **Package channels** (modified): the install script, GitHub Releases archives, `cargo install --git` and the Dockerfile are available. Homebrew, npm, Scoop, WinGet, the AUR, Nix and the container registry are partial: the manifests and templates exist, but publishing waits for external accounts.
- **Method detection, latest-version lookup, `cyber upgrade`** (modified): detects the install method from the executable path, then from package-manager listings. Reads `<base>/<channel>.json` with a 10 second timeout, 2 retries, a 1 hour cache and `CYBER_OFFLINE`. Self-replaces `script` installs atomically and runs the right manager command, or prints the manual command, for the other methods.
- **Self-update integrity** (modified): an archive is installed only when its SHA-256 matches a `SHA256SUMS` whose Sigstore bundle verifies against the embedded production trusted root and the release identity. Anything else fails closed.
- **Background update policy and minimum supported version** (modified): the server checks about 5 seconds after start and then every 24 hours. It serves the result at `GET /api/v1/service/update` and publishes the live-only `installation.update_available.1` / `installation.updated.1`. The TUI shows a toast. The TUI and `exec` warn on every start when the cached manifest's `min_supported` is newer than the running version.
- **Uninstall** (modified "Uninstall command"): lists the binary, the marked PATH lines and the data, config, state and cache directories with sizes. It asks unless `--force` is given and honors `--dry-run`, `--keep-config` and `--keep-data`. It stops the background server and deletes cyber-owned keyring entries. It never follows symlinks and refuses any path outside the resolved cyber directories.
- **Container and CI usage** (modified): a `Dockerfile` (non-root `cyber`, uid 10001, `CYBER_HOME`, `git`, `ripgrep`, entrypoint `cyber`). With `CI=true` there is no TUI (exit 2 with the documented message), no auto-update and no onboarding prompt, and `exec` defaults to `--format text`.
- `cli-commands` (modified "Upgrade and uninstall commands"): adds `--channel` to `cyber upgrade`.
- `tui` (modified "Update prompt"): the toast text and cadence follow `installation-upgrade` (server check, `run "cyber upgrade"`; no `/upgrade` slash command exists).

## Non-goals (deferred)

- **Publishing to Homebrew, npm, WinGet, Scoop, the AUR, Nix, cargo-binstall and container registries.** Each needs an external account, organization or token that does not exist yet. The release workflow renders the manifests and templates it would publish and attaches them to the release. The spec marks those channels partial. Detection and the upgrade or uninstall command for each manager are still implemented, because they only run the manager.
- **Hosting `cyber-code.dev`.** The domain is not registered. The release source is a build-time constant, so moving to it later changes one value and the spec text.
- **Shell integration on install** (completion and the `cyber://` handler). It is P1 ("Shell integration on install") and is not part of this P0 change.
- **HTTP 426 rejection by hosted services.** Relay, Runners and Share are P3 services that do not exist. Only the client-side warning of "Minimum supported version" is P0. The spec tags the 426 behavior (P3).
- Signing Windows binaries with Authenticode and notarizing macOS binaries. Both need paid certificates. Sigstore covers integrity, and Windows stays labeled preview.

## Impact

- New crate `crates/cyber-install`: release source, manifest lookup and cache, method detection, SHA256SUMS and Sigstore verification, archive download and extraction, atomic self-replace, upgrade planning, uninstall planning and removal. It is a new workspace member, so the sigstore dependency tree stays out of the other crates.
- `crates/cyber-core`: `version.rs` / `build.rs` (release-source constants), `config` (typed `autoupdate` validation), `env` (a `CI` flag helper).
- `crates/cyber-cli`: new `commands/upgrade.rs` and `commands/uninstall.rs`, two variants in `cli.rs`, two dispatch arms in `commands/mod.rs`, a `CI=true` guard before the TUI, the `min_supported` warning in `exec`, and `cyber.old` cleanup at start.
- `crates/cyber-app`: a background update task started by `run_server`.
- `crates/cyber-server`: `GET /api/v1/service/update` (OpenAPI, SDK) and the live events.
- `crates/cyber-tui`: the update and unsupported-version toasts.
- New files: `.github/workflows/release.yml`, `scripts/install.sh`, `Dockerfile`, `.dockerignore`, `packaging/` (Homebrew formula, npm launcher packages, Scoop, WinGet, AUR, Nix templates).
- Docs: README install section and feature table, `docs/features.md`, `docs/local-core-guide.md` (install, upgrade, uninstall, CI, container, test hook), generated `docs/cli.md`, the OpenAPI document and TS SDK, and the ROADMAP P0 status and inventory.
