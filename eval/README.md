# Evaluation fixtures

Reproducible inputs for `harness-evaluation`. Every manifest in `manifests/` is validated by
`cargo test -p cyber-core --test eval_manifests`.

| Directory | Contents |
|---|---|
| `manifests/` | Versioned manifests (`schema_version: 1`). `kind` is `deterministic` (scripted provider), `live` (real provider, never claimed deterministic), `suite` (live tasks with one fixture each, model chosen at run time) or `recovery` (fault cases linked to requirements). |
| `fixtures/` | Task repositories copied into a disposable workspace for each run. Each manifest pins the tree with `tree_sha256`. |
| `graders/` | Hidden graders. They live outside the fixture, so they are outside every agent-writable root, and they inspect final workspace state rather than the agent's claims. |
| `solutions/` | Reference solutions for suite tasks, one directory per task id. Never shown to agents; used only to prove the graders. |
| `repositories/` | Pinned public source catalogs for separate real-repository pilots. Downloads stay outside Git. |

## Changing a fixture

Edit the files, run the manifest test, and copy the new `tree_sha256` it reports into the
manifest. A changed fixture is a changed evaluation input and must be visible in review.

## Suites

New evaluation reports include all five provider-neutral token classes, including a separate
`tokens.reasoning` field. Main-step and successful-compaction usage and cost both count toward
trial totals and budget interruption; compaction does not increment the main Turn count.
Unpriced work keeps the reported dollar total a lower bound. Reports from before this accounting
fix omit reasoning and successful-compaction totals; retain them as historical evidence and use
the recorded harness revision when comparing measurements. Do not fill missing usage with
inferred values.

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

## Real repositories and verified solve

`repositories/real-repositories-v1.json` describes four historical regressions from ripgrep
(Rust) and Click (Python). It pins full base/fix commits, archive and source hashes, licenses,
prompts and public check commands. Reference patches, independent tests and license notices
live in the corresponding `graders/repo-*` directories. This retrospective pilot is separate
from the twenty-task release baseline. Live results are pending.

Build `cyber` and `cyber-sandbox-exec`, then import into a **new** private directory:

```sh
cargo build -p cyber-cli -p cyber-sandbox --bins
cyber eval prepare eval/repositories/real-repositories-v1.json --output /tmp/cyber-repo-inputs
```

Import downloads HTTPS archives, checks their hashes and base-revision metadata, bounds
extraction and validates source paths. It retains complete source with fresh base-only Git
history per attempt. It executes no repository setup commands. Install the catalog's Python,
pytest and Rust versions, and prefetch the pinned Cargo dependencies before disabling command
network access. An incomplete cache is a setup failure, not a coding outcome.

Use an isolated global configuration with `sandbox.network: "off"`, no personal hooks or
instructions, and the intended provider login store. Keep inputs, references, other reports
and private setup material outside agent-visible roots; configure `sandbox.deny_read` for
additional private directories. Input roots and current grading reports are automatically
hidden from model file tools and command execution. Command isolation remains enforced even
though evaluation mode bypasses interactive permission questions.

```sh
cyber eval run /tmp/cyber-repo-inputs/eval/manifests/real-repositories-v1.json \
  --model 'chatgpt/gpt-6.1-sol#medium' --strategy single \
  --trials 3 --jobs 1 --output /tmp/cyber-repo-single --keep-workspaces
cyber eval run /tmp/cyber-repo-inputs/eval/manifests/real-repositories-v1.json \
  --model 'chatgpt/gpt-6.1-sol#medium' --strategy solve \
  --trials 3 --jobs 1 --output /tmp/cyber-repo-solve --keep-workspaces
```

`single` remains the default. `solve` runs two independent candidates and a separate judge
using the requested model. It requires public literal-argv `checks` on each task. Only the
host-verified recommendation is independently graded; completion without a recommendation
does not pass. Grading copies tracked and non-ignored source into an isolated network-off
scratch directory, preserving the candidate and excluding ignored compiler products.
Windows verification fails closed pending native command isolation (issue #9).

For paired measurements, freeze an interleaved order before the first call and run one task
at a time with `--task ID --trials 1 --trial-start N`. Retain failures and infrastructure
outcomes rather than replacing them with retries. Reports add repository/grader provenance,
selected-source evidence, end-to-end and verification timing, plus scoped solver/judge usage.
Solve turn/tool totals are null when aggregate counts are unavailable. Subscription dollar
amounts are lower bounds, not proof of free usage. Current budgets are soft accounting limits;
reserved enforcement is separate roadmap work.

Blocked/unknown host checks without a verified survivor are infrastructure outcomes. Click's
catalog uses the `pytest` entry point with `--override-ini pythonpath=src`; the current
conservative interpreter guard refuses inline imports and `python -m` host checks. The
scripted reference controls exercise the same authorization path as live solve checks.

Run network-off base/reference/tampering controls with an installed sandbox helper:

```sh
python3 scripts/check_eval_graders.py --root /tmp/cyber-repo-inputs \
  --sandbox-cyber "$(command -v cyber)" --jobs 1
```

The [pilot protocol](../openspec/changes/add-real-repository-evaluations/design.md)
defines the trial counts, budgets, exclusions and interpretation limits. Live calls are opt-in
and never part of ordinary PR CI. Preserve historical benchmark files unchanged.
