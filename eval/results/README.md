# Evaluation results

Reports written by `cyber eval run` (`harness-evaluation`). Each directory holds `report.json`
(schema version 1), `report.md` and one JSON file per trial.

## suite-coding-v1 baseline (harness 108387c, 2026-10-04)

20 tasks × 3 trials per model, bypass mode inside the workspace sandbox, no human input.
Success is decided by hidden graders on the final workspace; failures stay in the
denominator and the interval is a 95% Wilson score interval.

| Model | Passed | 95% CI | Total cost | Median cost per success | Median latency | Long task | Infra failures |
|---|---|---|---|---|---|---|---|
| `openai/gpt-6-luna` | 60/60 (100%) | 94–100% | $0.15 | $0.0020 | 27 s | 3/3, median 27 turns | 0 |
| `anthropic/claude-haiku-4-5` | 48/60 (80%) | 68–88% | $7.40 | $0.0830 | 77 s | 1/3, median 81 turns | 0 |

Notes:
- `gpt-6-luna` passed every trial, so this suite cannot detect its regressions; harder tasks
  are needed before it serves as that model's release gate.
- Claude Haiku 4.5 failures are model outcomes: edge cases (`fix-slugify`, `fix-money-rounding`),
  behavior drift in `refactor-split-function`, the shell task, one surviving mutant, and the
  long task running out of its 80-turn budget twice (it ran 81 turns, the 50-turn criterion).
- Costs come from the catalog prices at run time; no result was unpriced.
- Changed models establish a separately labeled baseline (`harness-evaluation` → Live quality
  release baseline).
