## 1. Release workflow

- [x] 1.1 Create `channel-<channel>` with `--prerelease --latest=false`; convert an existing one with `gh release edit --prerelease --latest=false` before uploading the manifest
- [x] 1.2 `scripts/tests/test_release_workflow.py`: the channel release is never latest; only the `v<version>` release, carrying `install.sh`, can be

## 2. Specs and docs

- [x] 2.1 Sync the spec delta into `openspec/specs/installation-upgrade`
- [x] 2.2 `README.md` and `docs/local-core-guide.md` explain why the latest URL does not resolve before the first stable release
