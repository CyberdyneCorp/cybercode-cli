# Verified solve acceptance (2026-10-23)

Runtime source: PR [#23](https://github.com/CyberdyneCorp/cybercode-cli/pull/23), commit `c455cc319a70d7919da4f780148a36bbebf60783`. The checks below used the compiled `cyber` binary and its native sandbox helper on macOS. All model responses came from a local scripted OpenAI-compatible provider; no paid inference or model-quality comparison was performed.

## Process recovery and budgets

Each case used a fresh `CYBER_HOME`, a clean committed repository, two candidates (`fast` and `smart`) resolving to `test/main`, and a separate judge Session. `cyber workflows runs` started the registered background `cyber serve`. The fixture started the bundled workflow through the authenticated HTTP API, killed only that fixture server's registered PID with `SIGKILL` at the observed phase, restarted it through the CLI, and resumed the same run through `POST /api/v1/workflows/runs/{id}/resume`.

Checks appended one line to an ignored `build/count`; the checking case then slept so the crash occurred after launch. The owner HEAD and source contents were checked after every case. A completed run lifecycle does not imply complete verification: inspect `partial` and `verification_status` too.

| Case | Run lifecycle | Result | Evidence checked |
|---|---|---|---|
| Crash during solving | `completed` | `partial: true`, `incomplete`, no recommendation | The same two child Sessions and planned candidate branches remain; unresolved worktree activity stays fenced. Usage and cost completeness are both false. |
| Crash during checking | `completed` | `partial: true`, `incomplete`, no recommendation | The interrupted check's counter contains exactly one line after resume. Its outcome is not `passed`; native effects are not dispatched again. |
| Crash during judging | `completed` | `partial: false`, `recommended`, candidate `fast` | Both candidates retain host-verified checks, each counter still contains one line, and the run has two solvers plus one judge. |
| Agent budget of one | `budget_exceeded` | `partial: true`, `incomplete`, no recommendation | Exactly one agent starts; further dispatch stops. |

All four cases passed on the source commit above. The scripted models have no pricing, so every case reports `cost_complete: false`; zero recorded spend does not establish free execution. The default $5 inference limit remains a soft ceiling and is independent of resources consumed by check commands.

## Automated regression coverage

- `crates/cyber-cli/tests/bundled_workflows.rs` runs the real CLI with three competing patches and two model identities. Host checks reject one patch, the judge recommends a passing patch, usage and incomplete pricing remain visible, and the owner checkout stays unchanged.
- `crates/cyber-app/tests/support/solve_admission.rs` exercises the production App over HTTP: admission, source/check identity, isolation and denied writes, interactive approval, bounded logs, timeout, unknown settlement and restarts during solving/checking/judging.
- `crates/cyber-server/tests/workflow_runs.rs` includes regressions that failed before their fixes: an interrupted isolated admission retains its original worktree plan when no child exists yet, and failed agents preserve incomplete usage/pricing in both immediate and subsequent durable budget snapshots.
- The final focused integration run passes all 148 tests. Workspace all-target Clippy with warnings denied, formatting, generated SDK consistency and strict OpenSpec validation pass. Platform CI is recorded in the solve change's task evidence.

Native Windows builds, lint and curated tests are covered by CI. The full solve Git/process integration fixtures are Unix-only; native Windows solve acceptance and opt-in live multi-model quality acceptance have not been run. Passing scripted checks establishes protocol behavior, not comparative model quality or proof of patch correctness.
