## Why

`https://github.com/CyberdyneCorp/cybercode-cli/releases/latest/download/install.sh` returns 404. The release workflow creates the rolling `channel-beta` release with `--latest=false`, but not as a prerelease. Every tagged release so far is a beta prerelease, and no release was ever marked latest explicitly, so GitHub falls back to the newest non-prerelease, non-draft release: `channel-beta`. That release holds only `beta.json`, so the latest download URL redirects to `releases/download/channel-beta/install.sh`, which does not exist.

## What Changes

- `release.yml` creates each rolling `channel-<channel>` release with `--prerelease --latest=false`. GitHub never treats a prerelease as latest, explicitly or by fallback.
- On every channel update it also runs `gh release edit "channel-<channel>" --prerelease --latest=false`, so the existing `channel-beta` (created before this rule) is converted by the next beta tag, without manual release edits.
- Tagged releases keep their rule: a newer stable `v<version>` is made latest explicitly (`--latest`), an older stable one `--latest=false`, a non-stable one is a prerelease. Only a `v<version>` release, which carries `install.sh`, can be latest.
- `scripts/tests/test_release_workflow.py` asserts the channel release is created and edited as a non-latest prerelease and never gets `--latest`, and that the only release that can be latest is the `v<version>` one shipping `install.sh`.

Alternatives rejected:
- Attaching `install.sh` to the channel release would make the `latest` URL work today, but it keeps a rolling, manifest-only release as GitHub's latest, and the fallback would flip to whichever channel release was created last.
- Pointing the install link at `main` (`raw.githubusercontent.com`) installs an unreleased script that no release workflow signed.

Until the first stable release, the `latest` URL does not resolve (GitHub has no latest release at all once `channel-beta` is a prerelease). The docs already give a tagged release's script with `--channel beta`; that stays.

## Impact

- Spec: `installation-upgrade` → Install script (modified: the latest release is only ever a `v<version>` release; channel releases are non-latest prereleases).
- CI: `.github/workflows/release.yml`, `scripts/tests/test_release_workflow.py`.
- Docs: `README.md` (Install a release), `docs/local-core-guide.md` (Install script).
