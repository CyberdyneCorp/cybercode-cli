## Why

GitHub's macOS Intel runners (`macos-15-intel`) queue for one to two hours, so the `x86_64-apple-darwin` build and the Intel legs of the native proof workflows dominate the time every pull request waits for CI. The other targets finish in minutes. An Intel-only build failure is rare, and finding it right after merge, still before any tag is pushed, keeps the guarantee the requirement exists for.

## What Changes

- `ci.yml` reads the CI build matrix from one checked-in list, `.github/build-targets.json` (the same 8 targets as `scripts/release_assets.py` `ALL_TARGETS`). The `changes` job emits it as an output; on pull requests it drops the entries marked `post_merge_only` (only `x86_64-apple-darwin`). Pushes to `main` build all 8.
- The native proof workflows exclude `macos-15-intel` from their matrices on `pull_request` and keep it on `push` to `main` and `workflow_dispatch`:
  - `plugin-bootstrap-environment.yml`, `plugin-native-launcher.yml`, `plugin-package-copy.yml`, `plugin-admission-proof.yml` (still runs `macos-15` on pull requests);
  - `plugin-confinement-proof.yml` (`native`, `macos-signing-candidate`, `macos-signing-runtime`; `macos-native-gap` already runs only on `workflow_dispatch`);
  - `non-git-snapshot-proof.yml`, which ran only on pull requests and `workflow_dispatch`, gains a `push` to `main` trigger with the same paths, so its Intel leg still runs after merge.
- `release.yml` is unchanged: a release builds every released target, Intel included.
- `scripts/tests/test_release_workflow.py` checks the list matches `ALL_TARGETS`, that only `x86_64-apple-darwin` is post-merge-only, and runs the workflow's own `jq` filter for both events.

## Impact

- Spec: `installation-upgrade` → Release targets (modified: Intel macOS builds on each push to `main` instead of each pull request).
- CI: `.github/build-targets.json`, `.github/workflows/ci.yml` and the six proof workflows above.
- Docs: `CONTRIBUTING.md` (What CI checks).
