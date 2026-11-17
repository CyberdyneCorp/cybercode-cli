## Why

Windows confinement is not finished: sandboxed Git Bash and PowerShell cannot start in the AppContainer ([issue #9](https://github.com/CyberdyneCorp/cybercode-cli/issues/9)). Publishing Windows binaries in the first release, even labeled preview, would invite use without the protection the other platforms have.

## What Changes

- Releases publish the six macOS and Linux targets only. CI keeps building and testing both Windows targets on every pull request.
- `scripts/release_assets.py` gains `WINDOWS_RELEASED = False`. It removes the Windows archives, the `win32` npm packages and the Scoop and WinGet manifests from the release; setting it to true restores them.
- The npm `@cyber-code/cli` package lists only the platform packages that are published.
- Signing in `release.yml` tolerates a release with no `.zip` archive.
- `install.sh` says Windows builds are not released yet.

## Impact

- Spec: `installation-upgrade` (Release targets, Package channels).
- Code: `.github/workflows/release.yml`, `scripts/release_assets.py`, `scripts/install.sh`, `packaging/npm/cli/package.json` and their tests.
- Docs: README, CHANGELOG, local core guide.
