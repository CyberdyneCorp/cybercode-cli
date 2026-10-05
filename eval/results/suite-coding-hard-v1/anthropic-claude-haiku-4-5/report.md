# Evaluation: suite-coding-hard-v1 on anthropic/claude-haiku-4-5

anthropic/claude-haiku-4-5 passed 17/60 (28.3%, 95% CI 18.5-40.8%), cost $34.1718, infra failures 0

Harness 0.1.0 (ddd3b4f), 3 trials per task, started 2026-10-04T22:08:43Z.

| Task | Tags | Passed | Infra failures | Cost (USD) | Median turns |
|---|---|---|---|---|---|
| debug-billing-proration | debugging, hard, python, multi-file, decimal, dates | 1/3 | 0 | 1.1403 | 61.0 |
| debug-event-replay | debugging, hard, python, multi-file, idempotency, event-sourcing, decimal | 2/3 | 0 | 1.3591 | 61.0 |
| fix-unicode-table | bugfix, hard, python, unicode, multi-file | 3/3 | 0 | 1.4480 | 61.0 |
| feat-incremental-indexer | feature, hard, python, caching, multi-file | 3/3 | 0 | 1.3187 | 61.0 |
| impl-expr-evaluator | spec, hard, python, interpreter, parsing | 0/3 | 0 | 1.5095 | 61.0 |
| perf-sessionize | performance, hard, python, analytics | 3/3 | 0 | 1.0180 | 61.0 |
| refactor-exporter-registry | refactor, hard, python, multi-file, plugins | 0/3 | 0 | 1.0860 | 61.0 |
| tests-cron-mutation | tests, hard, python, mutation, dates | 2/3 | 0 | 1.3455 | 42.0 |
| fix-cursor-pagination | bugfix, hard, node, pagination, multi-file | 0/3 | 0 | 1.1041 | 60.0 |
| impl-patch-apply | spec, hard, node, diff, parsing | 0/3 | 0 | 1.7468 | 61.0 |
| impl-dep-resolver | spec, hard, node, semver, backtracking | 0/3 | 0 | 1.4562 | 61.0 |
| impl-template-engine | spec, hard, node, templating, parsing | 0/3 | 0 | 2.1628 | 61.0 |
| perf-leaderboard | performance, hard, node, data-structures | 0/3 | 0 | 1.7641 | 61.0 |
| refactor-callbacks-async | refactor, hard, node, async, multi-file | 0/3 | 0 | 1.4421 | 47.0 |
| tests-lru-mutation | tests, hard, node, mutation, caching | 0/3 | 0 | 1.6169 | 49.0 |
| fix-job-queue | concurrency, hard, node, async, bugfix | 3/3 | 0 | 1.1578 | 48.0 |
| fix-shell-log-report | bugfix, hard, bash, awk, text-processing | 0/3 | 0 | 0.8647 | 61.0 |
| fix-shell-kv-store | bugfix, hard, bash, quoting, cli | 0/3 | 0 | 1.5024 | 61.0 |
| build-ledger-cli | greenfield, hard, python, long, cli, decimal | 0/3 | 0 | 4.4031 | 132.0 |
| build-minimake | greenfield, hard, node, long, cli, build-system | 0/3 | 0 | 4.7256 | 127.0 |

Cost per success: 2.010104. Median cost per successful task: 0.444703. Median latency: 309.2 s. Unpriced results: false.
