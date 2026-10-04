# Evaluation: suite-coding-hard-v1 on openai/gpt-6-luna

openai/gpt-6-luna passed 40/60 (66.7%, 95% CI 54.1-77.3%), cost $0.6941, infra failures 0

Harness 0.1.0 (ddd3b4f), 3 trials per task, started 2026-10-04T20:36:58Z.

| Task | Tags | Passed | Infra failures | Cost (USD) | Median turns |
|---|---|---|---|---|---|
| debug-billing-proration | debugging, hard, python, multi-file, decimal, dates | 3/3 | 0 | 0.0184 | 12.0 |
| debug-event-replay | debugging, hard, python, multi-file, idempotency, event-sourcing, decimal | 3/3 | 0 | 0.0324 | 25.0 |
| fix-unicode-table | bugfix, hard, python, unicode, multi-file | 1/3 | 0 | 0.0268 | 14.0 |
| feat-incremental-indexer | feature, hard, python, caching, multi-file | 3/3 | 0 | 0.0246 | 17.0 |
| impl-expr-evaluator | spec, hard, python, interpreter, parsing | 3/3 | 0 | 0.0433 | 21.0 |
| perf-sessionize | performance, hard, python, analytics | 3/3 | 0 | 0.0167 | 18.0 |
| refactor-exporter-registry | refactor, hard, python, multi-file, plugins | 3/3 | 0 | 0.0368 | 20.0 |
| tests-cron-mutation | tests, hard, python, mutation, dates | 3/3 | 0 | 0.0197 | 11.0 |
| fix-cursor-pagination | bugfix, hard, node, pagination, multi-file | 2/3 | 0 | 0.0222 | 13.0 |
| impl-patch-apply | spec, hard, node, diff, parsing | 1/3 | 0 | 0.0312 | 13.0 |
| impl-dep-resolver | spec, hard, node, semver, backtracking | 1/3 | 0 | 0.0474 | 24.0 |
| impl-template-engine | spec, hard, node, templating, parsing | 3/3 | 0 | 0.0282 | 14.0 |
| perf-leaderboard | performance, hard, node, data-structures | 3/3 | 0 | 0.0383 | 17.0 |
| refactor-callbacks-async | refactor, hard, node, async, multi-file | 3/3 | 0 | 0.0541 | 24.0 |
| tests-lru-mutation | tests, hard, node, mutation, caching | 0/3 | 0 | 0.0192 | 11.0 |
| fix-job-queue | concurrency, hard, node, async, bugfix | 1/3 | 0 | 0.0250 | 14.0 |
| fix-shell-log-report | bugfix, hard, bash, awk, text-processing | 3/3 | 0 | 0.0434 | 29.0 |
| fix-shell-kv-store | bugfix, hard, bash, quoting, cli | 0/3 | 0 | 0.0397 | 18.0 |
| build-ledger-cli | greenfield, hard, python, long, cli, decimal | 1/3 | 0 | 0.0499 | 21.0 |
| build-minimake | greenfield, hard, node, long, cli, build-system | 0/3 | 0 | 0.0767 | 35.0 |

Cost per success: 0.017353. Median cost per successful task: 0.009391. Median latency: 145.5 s. Unpriced results: false.
