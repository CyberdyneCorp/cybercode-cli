# Evaluation: suite-coding-hard-v1 on anthropic/claude-sonnet-5-5#medium

anthropic/claude-sonnet-5-5#medium passed 52/60 (86.7%, 95% CI 75.8-93.1%), cost $21.3014, infra failures 0

Harness 0.1.0 (2e66709), 3 trials per task, started 2026-10-04T23:07:39Z.

| Task | Tags | Passed | Infra failures | Cost (USD) | Median turns |
|---|---|---|---|---|---|
| debug-billing-proration | debugging, hard, python, multi-file, decimal, dates | 3/3 | 0 | 0.3059 | 6.0 |
| debug-event-replay | debugging, hard, python, multi-file, idempotency, event-sourcing, decimal | 1/3 | 0 | 0.3864 | 8.0 |
| fix-unicode-table | bugfix, hard, python, unicode, multi-file | 3/3 | 0 | 0.4811 | 7.0 |
| feat-incremental-indexer | feature, hard, python, caching, multi-file | 3/3 | 0 | 0.4911 | 7.0 |
| impl-expr-evaluator | spec, hard, python, interpreter, parsing | 3/3 | 0 | 1.0490 | 7.0 |
| perf-sessionize | performance, hard, python, analytics | 3/3 | 0 | 0.3340 | 7.0 |
| refactor-exporter-registry | refactor, hard, python, multi-file, plugins | 3/3 | 0 | 1.0534 | 17.0 |
| tests-cron-mutation | tests, hard, python, mutation, dates | 3/3 | 0 | 2.0561 | 9.0 |
| fix-cursor-pagination | bugfix, hard, node, pagination, multi-file | 1/3 | 0 | 0.3050 | 6.0 |
| impl-patch-apply | spec, hard, node, diff, parsing | 3/3 | 0 | 0.6005 | 8.0 |
| impl-dep-resolver | spec, hard, node, semver, backtracking | 3/3 | 0 | 2.1114 | 26.0 |
| impl-template-engine | spec, hard, node, templating, parsing | 3/3 | 0 | 0.5584 | 8.0 |
| perf-leaderboard | performance, hard, node, data-structures | 3/3 | 0 | 0.4781 | 8.0 |
| refactor-callbacks-async | refactor, hard, node, async, multi-file | 3/3 | 0 | 1.1621 | 15.0 |
| tests-lru-mutation | tests, hard, node, mutation, caching | 3/3 | 0 | 1.4836 | 10.0 |
| fix-job-queue | concurrency, hard, node, async, bugfix | 3/3 | 0 | 0.3962 | 7.0 |
| fix-shell-log-report | bugfix, hard, bash, awk, text-processing | 1/3 | 0 | 0.5270 | 7.0 |
| fix-shell-kv-store | bugfix, hard, bash, quoting, cli | 1/3 | 0 | 1.2791 | 12.0 |
| build-ledger-cli | greenfield, hard, python, long, cli, decimal | 3/3 | 0 | 2.5788 | 23.0 |
| build-minimake | greenfield, hard, node, long, cli, build-system | 3/3 | 0 | 3.6642 | 19.0 |

Cost per success: 0.409642. Median cost per successful task: 0.230309. Median latency: 115.9 s. Unpriced results: false.
