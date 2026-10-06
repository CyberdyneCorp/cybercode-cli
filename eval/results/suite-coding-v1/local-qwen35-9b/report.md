# Evaluation: suite-coding-v1 on local/qwen3.5:9b

local/qwen3.5:9b passed 29/60 (48.3%, 95% CI 36.2-60.7%), cost $0.0000, infra failures 3

Harness 0.1.0 (1d8d1e4), 3 trials per task, started 2026-10-05T22:43:17Z.

| Task | Tags | Passed | Infra failures | Cost (USD) | Median turns |
|---|---|---|---|---|---|
| fix-slugify | bugfix, python, edge-cases | 2/3 | 0 | 0.0000 | 11.0 |
| fix-moving-average | bugfix, python, off-by-one, multi-file | 3/3 | 0 | 0.0000 | 10.0 |
| fix-tz-daily-buckets | bugfix, python, datetime | 3/3 | 0 | 0.0000 | 11.0 |
| fix-csv-quoted-fields | bugfix, node, parsing, multi-file | 0/3 | 0 | 0.0000 | 21.0 |
| fix-mutable-defaults | bugfix, python, multi-file | 3/3 | 0 | 0.0000 | 12.0 |
| fix-async-batch | bugfix, node, async, multi-file | 0/3 | 0 | 0.0000 | 31.0 |
| fix-stable-sort | bugfix, node, sorting | 3/3 | 0 | 0.0000 | 9.0 |
| fix-money-rounding | bugfix, python, money | 1/3 | 0 | 0.0000 | 26.0 |
| feat-cli-flags | feature, python, cli | 0/3 | 0 | 0.0000 | 31.0 |
| feat-ttl-cache | feature, python, caching | 1/3 | 0 | 0.0000 | 16.0 |
| feat-paginate | feature, node, pagination | 3/3 | 0 | 0.0000 | 7.0 |
| feat-validator-rules | feature, python, validation | 0/3 | 0 | 0.0000 | 30.0 |
| feat-csv-export | feature, python, csv | 3/3 | 0 | 0.0000 | 24.0 |
| refactor-split-function | refactor, python | 0/3 | 0 | 0.0000 | 24.0 |
| refactor-rename-api | refactor, python, api | 3/3 | 0 | 0.0000 | 31.0 |
| tests-merge-intervals | tests, python, mutation | 0/3 | 1 | 0.0000 | 7.0 |
| tests-semver-compare | tests, node, mutation | 0/3 | 0 | 0.0000 | 9.0 |
| debug-cross-module | debugging, python, multi-file | 3/3 | 0 | 0.0000 | 10.0 |
| fix-shell-top-ips | shell, bash, bugfix | 1/3 | 0 | 0.0000 | 20.0 |
| build-inventory-cli | long, greenfield, python, cli | 0/3 | 2 | 0.0000 | 27.0 |

Cost per success: 0.0. Median cost per successful task: 0.0. Median latency: 536.9 s. Unpriced results: true.
