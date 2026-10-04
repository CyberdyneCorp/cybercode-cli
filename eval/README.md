# Evaluation fixtures

Reproducible inputs for `harness-evaluation`. Every manifest in `manifests/` is validated by
`cargo test -p cyber-core --test eval_manifests`.

| Directory | Contents |
|---|---|
| `manifests/` | Versioned manifests (`schema_version: 1`). `kind` is `deterministic` (scripted provider), `live` (real provider, never claimed deterministic) or `recovery` (fault cases linked to requirements). |
| `fixtures/` | Task repositories copied into a disposable workspace for each run. Each manifest pins the tree with `tree_sha256`. |
| `graders/` | Hidden graders. They live outside the fixture, so they are outside every agent-writable root, and they inspect final workspace state rather than the agent's claims. |

## Changing a fixture

Edit the files, run the manifest test, and copy the new `tree_sha256` it reports into the
manifest. A changed fixture is a changed evaluation input and must be visible in review.

## Recovery cases

`recovery-*.json` manifests list fault cases. Each names the requirement it verifies as
`<capability>/<Requirement name>`, the fault class, and the test that executes it. Process-kill
cases are not power-loss simulations; the `fault` field keeps them distinct.

`cyber eval run` executes coding manifests from milestone M0.2, once the scripted provider exists.
