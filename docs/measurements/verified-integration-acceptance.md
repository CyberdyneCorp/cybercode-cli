# Verified integration queue acceptance

Validated locally on macOS on 2026-10-24 at implementation commit `69b4faed` before merging telemetry main `8ef37de7`. The subsequent main merge had one generated SDK type conflict, resolved by regeneration from the combined schema; combined SDK/dashboard type and build checks passed. Platform CI remains pending. The queue composes complementary local commits and checks the **combined** source. A passing individual contribution, model response, or reviewer recommendation cannot create host verification evidence.

## Local checks

- Workflow domain, dependency validation, bundled discovery and engine tests: 32 passed.
- Real App/server integration regressions: 18 passed. These cover independent patches passing alone but failing together, successful combined checks and a separate read-only reviewer, textual conflicts, unrelated history, protected configuration and forged claims, malicious hooks/filters/partial-clone configuration, stale target/source mutation, missing reviewer, denied worktree/Plan mode, budgets, conflicting revisions, duplicate submission after a lost HTTP receipt, restart and hostile temporary-directory replacement. A project-configured PATH cannot replace native Git authority, and a late publication receipt cannot attach to a replacement attempt.
- Existing solve-admission regressions: 20 passed after the shared Git/environment hardening.
- Dashboard unit suite, including integration model tests: 21 passed. Uncertain actions retain their original request, key, revision and target across retries; changing the selected batch cannot redirect a pending action. Publication requires an explicit reviewed destination and acknowledgement of missing independent review.
- Browser TypeScript, dashboard TypeScript, build and committed-build checks passed. Cognitive-complexity analysis of the new browser modules reported a maximum of 9 with no analyzer warnings.

- Actual frozen-binary CLI acceptance: submit and `run --wait` produced host-passing evidence with explicitly unavailable review; concurrent starts returned one success and one conflict, then a ready batch.
- Actual browser/server acceptance: 3 passed, covering inert evidence rendering and explicit publication controls, SIGKILL recovery without repeating an uncertain check, and exact lost-ack PR reconciliation without repeating Git/PR creation. GitHub transport was mocked; source preparation and checks were real.
- Generated TypeScript SDK against the real server/local Git: 1 passed. SDK unit suite: 112 passed, 9 optional process tests skipped without `CYBER_BIN`; the new integration process test was run separately with the frozen binary.
- Public hello credential regression: 1 passed, including normalized trailing slash and query forms. Publication destination/reconciliation boundary tests: 2 passed. CLI bundled-library/override tests: 2 passed; App library precedence: 1 passed.
- Current OpenAPI snapshot, generated SDK sources, strict all-target Clippy for the seven affected packages, formatting, diff whitespace, strict OpenSpec change validation and dashboard committed-build checks passed. Spec lint reported 0 errors and 23 warnings; no main-branch comparison was performed for those warnings.
- The final macOS CLI/helper build succeeded and emitted a linker compact-unwind size warning. The frozen CLI SHA-256 was `a506de13716462fff407c2699786ae0f21012374657e0e5d8d61eefbce0fb456`.

## Review and current-main revalidation (2026-10-25)

Merged main `82597aa0`, preserving the message inbox and transcript paging fixes. The first CI run exposed an overly broad native Git environment rule, a missing TUI library entry, missing local Git fixture identity and mobile tab overflow. Those were corrected without weakening integration authority.

- Existing worktree setup suite: 32 passed; concurrent workflow writer regression: 1 passed; TUI library/form test and three updated snapshots: 1 passed.
- Final App integration suite: 20 passed, adding exact reserved configuration-root symlink refusal and current-attempt orphan recovery checks. Native authority environment regression: 1 passed, proving runtime-loader/config overlays are removed for authoritative commands while ordinary verification preserves its environment.
- Merged dashboard unit suite: 34 passed. Full browser suite: 28 passed before the final read-only receipt/environment additions. The final six integration browser cases all passed against the refreshed binary, including trusted Git/GitHub executable environment probes, cancellation during GitHub lookup and hook admission, unknown publication receipt inspection with no repeated push/create, and lost-ack reconciliation.
- Current-main generated SDK process acceptance and CLI submission/run/concurrency acceptance passed. Strict all-target Clippy for tools/server/app/TUI/CLI passed after the final changes; formatting, generated SDK, CLI help reference and strict OpenSpec validation passed. All 96 specs/changes passed strict validation on the merged tree.
- A rebuild first failed at linking with `errno=28` (disk full), before tests ran. Builds stopped while obsolete generated artifacts were reclaimed; subsequent focused checks/build succeeded. No user data was removed. The successful macOS build still emitted the compact-unwind linker-size warning.
- Final frozen CLI SHA-256: `40ed5908e54e5ad7a6123acc5ad31a7089f381faa8c5c797bb30be27e2bfc84c`. Platform CI for the updated tip remains pending; the earlier failed CI run is not presented as acceptance.

## Evidence and recovery guarantees

Native check intents and settlements record the immutable argv definition, prepared launch environment hash, source fingerprint, output and outcome. Reports retain the original verified diff and check evidence after later source mutation. Completed batches and revision history survive restart. An interrupted native effect stays unknown and cannot be turned into a passing check by replaying agent JSON.

Publication has separate durable intent/settlement records. A lost command acknowledgement is reconciled only against an exact repository, branch, commit, target and non-fork PR match. An unresolved previous intent without that match is not automatically retried. No path merges a PR or force-pushes.

## Limits requiring deployment acceptance

- CyberdyneAuth is offline. Fresh issuer/membership revocation, account snapshot and replay-generation behavior are covered with signed local issuer fixtures; real login, organization/guest policy and live revocation still require authenticated deployment acceptance. Hub-linked contributions fail closed during this outage. Local commits do not require a Cyber Account.
- GitHub publication acceptance uses local Git/GitHub executable adapters. No real remote push or PR creation is performed by these tests. Real credential grants and remote publication require owner acceptance in a disposable repository.
- Local OS sandbox acceptance was run on macOS. Linux and Windows verification remains for platform CI; the POSIX publication adapter test explicitly skips Windows.
- Verification uses the owner's local toolchain and configured sandbox, not a reproducible-container claim. Cost reports distinguish actual priced usage, unpriced/incomplete usage and unavailable prediction; a budget ceiling is not an estimated bill.
- Required integration evidence and reviewer Sessions remain durable. The parallel transcript-retention implementation must retain bundled `integrate` runs; root integration will preserve this exclusion before enabling finished-agent expiry.
