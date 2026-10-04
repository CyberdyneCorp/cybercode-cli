# Evaluation fixtures

Reproducible inputs for `harness-evaluation`. Every manifest in `manifests/` is validated by
`cargo test -p cyber-core --test eval_manifests`.

| Directory | Contents |
|---|---|
| `manifests/` | Versioned manifests (`schema_version: 1`). `kind` is `deterministic` (scripted provider), `live` (real provider, never claimed deterministic), `suite` (live tasks with one fixture each, model chosen at run time) or `recovery` (fault cases linked to requirements). |
| `fixtures/` | Task repositories copied into a disposable workspace for each run. Each manifest pins the tree with `tree_sha256`. |
| `graders/` | Hidden graders. They live outside the fixture, so they are outside every agent-writable root, and they inspect final workspace state rather than the agent's claims. |
| `solutions/` | Reference solutions for suite tasks, one directory per task id. Never shown to agents; used only to prove the graders. |

## Changing a fixture

Edit the files, run the manifest test, and copy the new `tree_sha256` it reports into the
manifest. A changed fixture is a changed evaluation input and must be visible in review.

## Suites

`suite-coding-v1.json` is the published coding baseline: 20 tasks (bug fixes, features,
refactors, test writing, cross-module debugging, a shell task and one `long` greenfield build),
in Python 3.10+, Node 22 and bash with no third-party packages and no network. Each task names
its own `fixture`, `tags`, budget and grader; the suite has no model, which is chosen at run time.

`suite-coding-hard-v1.json` exists because v1 saturated: `openai/gpt-6-luna` passed 60/60, so v1
cannot detect regressions in strong models. Its 20 tasks (all tagged `hard`) are built so that a
strong frontier model fails a meaningful fraction, aiming for roughly a 50–80% pass rate. The
difficulty comes from investigation and precision, never from ambiguity: every graded behavior is
stated in the prompt, the fixture README or an in-repo spec. The suite contains:

- symptom-only bug reports over multi-module codebases with several independent root causes
  (`debug-*`, `fix-*`);
- strict specs graded by hundreds of hidden cases (`impl-*`);
- performance limits on large hidden inputs, with at least 5x headroom over the reference
  solution (`perf-*`);
- refactors checked by randomized differential testing against a frozen copy of the original
  (`refactor-*`);
- test writing graded by 28 and 37 mutants (`tests-*`);
- concurrency and async ordering (`fix-job-queue`);
- two bash tasks;
- two `long` greenfield builds from a SPEC.md (`build-ledger-cli`, `build-minimake`).

Normal tasks have a budget of 60 turns, $2 and 1800 s; long tasks get 150 turns, $5 and 3600 s.

Grader contract: `python3 {grader_dir}/grade.py {workspace}` prints `FAIL <reason>` lines and a
final `PASS` or `FAIL`, and exits 0 only on `PASS`. Graders copy the workspace to a temporary
directory and never modify it, run code in subprocesses with timeouts, require the visible tests
to still exist and pass, and add hidden assertions. Each grader directory is self-contained.
Test-writing tasks are graded by mutation testing: the agent's tests must pass against the
reference implementation and fail against every mutant in the grader's `mutants/` directory.

## Solutions and the grader checker

`eval/solutions/<task-id>/` holds the files that, overlaid onto a fresh copy of the fixture,
solve the task; paths listed in an optional `DELETE` file are removed. Check every suite task with

```sh
python3 scripts/check_eval_graders.py            # all tasks; --task ID to narrow, --verbose for output
```

For each task it copies the fixture, runs `git init`, requires the grader to FAIL the untouched
fixture, overlays the solution and requires PASS. It also checks the output contract, that the
grader left the workspace unchanged, and the manifest's `tree_sha256`. Run it after changing a
fixture, grader or solution.

## Recovery cases

`recovery-*.json` manifests list fault cases. Each names the requirement it verifies as
`<capability>/<Requirement name>`, the fault class, and the test that executes it. Process-kill
cases are not power-loss simulations; the `fault` field keeps them distinct.

`cyber eval run` executes coding manifests from milestone M0.2, once the scripted provider exists.
