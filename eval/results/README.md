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

## suite-coding-hard-v1 calibration (harness ddd3b4f, 2026-10-04)

Harder tasks so strong models do not saturate: multi-cause debugging from symptoms, strict
specs with hundreds of hidden cases, performance limits, differential refactors, mutation
testing and two long greenfield builds.

| Model | Passed | 95% CI | Total cost | Median cost per success | Median latency | Infra failures |
|---|---|---|---|---|---|---|
| `openai/gpt-6-luna` | 40/60 (67%) | 54–77% | $0.69 | $0.0094 | 146 s | 0 |
| `anthropic/claude-haiku-4-5` | 17/60 (28%) | 19–41% | $34.17 | $0.4447 | 309 s | 0 |
| `anthropic/claude-sonnet-5-5#medium` | 52/60 (87%) | 76–93% | $21.30 | $0.2303 | 116 s | 0 |

Failures were spread across mutation testing, spec details, shell quoting, concurrency and
both long builds, which ended early and failed differential tests.

Anthropic runs used harness 2e66709 plus the adaptive-thinking fix for effort-only Claude
models (earlier builds sent `thinking.type: enabled`, which Sonnet 5.5 rejects).

- Claude Sonnet 5.5 failed only `debug-event-replay`, `fix-cursor-pagination` and the two
  shell tasks (2/3 each). Median 8.5 turns per trial, none hit the turn limit.
- Claude Haiku 4.5 ran without thinking (its catalog entry takes budgets, and no variant was
  chosen). 42 of 60 trials hit the 61-turn limit and 13 tasks failed in every trial; the
  median trial used every turn.
- Total cost per pass: luna $0.017, Sonnet 5.5 $0.41, Haiku 4.5 $2.01.
