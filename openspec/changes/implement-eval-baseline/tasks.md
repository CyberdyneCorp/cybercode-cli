## 1. Runner

- [x] 1.1 Suite manifest kind with per-task fixtures and validation.
- [x] 1.2 `cyber eval run`: disposable workspaces, budgets, hidden grading, concurrency.
- [x] 1.3 Versioned report with Wilson intervals, cost, latency, tokens and infrastructure failures.

## 2. Suite and baseline

- [x] 2.1 20 tasks with hidden graders and reference solutions; grader checker in CI.
- [x] 2.2 Baseline: `openai/gpt-6-luna` and `anthropic/claude-haiku-4-5`, 3 trials each.
- [ ] 2.3 Local-model baseline (needs a local OpenAI-compatible server).
- [ ] 2.4 Harder tasks so the suite does not saturate on strong models.
