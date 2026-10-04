# Evaluation: suite-coding-v1 on anthropic/claude-haiku-4-5

anthropic/claude-haiku-4-5 passed 48/60 (80.0%, 95% CI 68.2-88.2%), cost $7.4037, infra failures 0

Harness 0.1.0 (108387c), 3 trials per task, started 2026-10-04T15:41:10Z.

| Task | Tags | Passed | Infra failures | Cost (USD) | Median turns |
|---|---|---|---|---|---|
| fix-slugify | bugfix, python, edge-cases | 1/3 | 0 | 0.1220 | 14.0 |
| fix-moving-average | bugfix, python, off-by-one, multi-file | 3/3 | 0 | 0.1807 | 20.0 |
| fix-tz-daily-buckets | bugfix, python, datetime | 3/3 | 0 | 0.3303 | 21.0 |
| fix-csv-quoted-fields | bugfix, node, parsing, multi-file | 3/3 | 0 | 0.1757 | 16.0 |
| fix-mutable-defaults | bugfix, python, multi-file | 3/3 | 0 | 0.1521 | 11.0 |
| fix-async-batch | bugfix, node, async, multi-file | 2/3 | 0 | 0.1756 | 14.0 |
| fix-stable-sort | bugfix, node, sorting | 3/3 | 0 | 0.2053 | 18.0 |
| fix-money-rounding | bugfix, python, money | 1/3 | 0 | 0.2849 | 22.0 |
| feat-cli-flags | feature, python, cli | 3/3 | 0 | 0.3831 | 24.0 |
| feat-ttl-cache | feature, python, caching | 3/3 | 0 | 0.2639 | 14.0 |
| feat-paginate | feature, node, pagination | 3/3 | 0 | 0.1331 | 7.0 |
| feat-validator-rules | feature, python, validation | 3/3 | 0 | 0.2773 | 21.0 |
| feat-csv-export | feature, python, csv | 3/3 | 0 | 0.3307 | 24.0 |
| refactor-split-function | refactor, python | 1/3 | 0 | 0.7058 | 31.0 |
| refactor-rename-api | refactor, python, api | 3/3 | 0 | 0.4721 | 31.0 |
| tests-merge-intervals | tests, python, mutation | 2/3 | 0 | 0.3540 | 17.0 |
| tests-semver-compare | tests, node, mutation | 3/3 | 0 | 0.6946 | 26.0 |
| debug-cross-module | debugging, python, multi-file | 3/3 | 0 | 0.1600 | 15.0 |
| fix-shell-top-ips | shell, bash, bugfix | 1/3 | 0 | 0.3056 | 31.0 |
| build-inventory-cli | long, greenfield, python, cli | 1/3 | 0 | 1.6969 | 81.0 |

Cost per success: 0.154244. Median cost per successful task: 0.083043. Median latency: 77.2 s. Unpriced results: false.
