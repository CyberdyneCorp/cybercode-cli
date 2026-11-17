## 1. CI

- [x] 1.1 `.github/build-targets.json` as the single CI build list; `ci.yml` `changes` job filters it on pull requests and `platforms` uses it via `fromJSON`
- [x] 1.2 Exclude `macos-15-intel` on pull requests in the proof workflows; add a `push` to `main` trigger to `non-git-snapshot-proof.yml`
- [x] 1.3 `scripts/tests/test_release_workflow.py`: list equals `ALL_TARGETS`, pull requests drop only `x86_64-apple-darwin`

## 2. Specs and docs

- [x] 2.1 Sync the spec delta into `openspec/specs/installation-upgrade`
- [x] 2.2 `CONTRIBUTING.md` What CI checks
