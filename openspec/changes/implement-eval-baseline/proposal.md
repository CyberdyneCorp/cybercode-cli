# Implement the evaluation runner and the first live baseline

## Why

The P0 exit criteria require a published live coding baseline: at least 20 tasks, 3 trials per task and per provider, with hidden grading, uncertainty and cost. Without it, quality regressions cannot be detected and "the agent codes well" stays a claim.

## What Changes

- `cyber eval run <manifest> --output <dir> [-m model] [--trials N] [--task id] [--jobs N] [--keep-workspaces]`:
  - Each trial runs in a disposable git copy of its fixture, through an in-process server with an in-memory database, in `bypass` mode inside the workspace sandbox, with never-ask Session rules.
  - Turn, cost, token and wall-time budgets are enforced.
  - A hidden grader decides the outcome from the final workspace.
  - It writes `report.json` (schema version 1), `report.md` and per-trial records. The report covers success with a 95% Wilson interval, failures kept in the denominator, infrastructure failures counted separately, total and per-success cost, median latency, tokens, unpriced flags, harness version and SHA, the manifest hash, and resolved model metadata.
- Manifests gain the `suite` kind: each task has its own fixture and tags, and the model is chosen at run time and recorded in the report.
- `eval/manifests/suite-coding-v1.json`: 20 tasks in Python, Node and bash, with hidden graders and reference solutions. The tasks are bug fixes, features, refactors, mutation-graded test writing, cross-module debugging, a shell fix and one long greenfield task.
- `scripts/check_eval_graders.py`, run in CI, proves each grader fails the original fixture and passes the reference solution.
- `eval/results/`: the first baseline for `openai/gpt-6-luna` and `anthropic/claude-haiku-4-5`.
- The build script now rebuilds when the git commit changes, so reports carry the correct SHA.

## Impact

No local-model baseline yet, because no local model server is installed. The suite saturates on `gpt-6-luna`, so harder tasks are needed before it can gate that model.
