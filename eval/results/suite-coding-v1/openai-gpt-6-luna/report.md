# Evaluation: suite-coding-v1 on openai/gpt-6-luna

openai/gpt-6-luna passed 60/60 (100.0%, 95% CI 94.0-100.0%), cost $0.1544, infra failures 0

Harness 0.1.0 (108387c), 3 trials per task, started 2026-10-04T15:41:11Z.

| Task | Tags | Passed | Infra failures | Cost (USD) | Median turns |
|---|---|---|---|---|---|
| fix-slugify | bugfix, python, edge-cases | 3/3 | 0 | 0.0029 | 6.0 |
| fix-moving-average | bugfix, python, off-by-one, multi-file | 3/3 | 0 | 0.0046 | 9.0 |
| fix-tz-daily-buckets | bugfix, python, datetime | 3/3 | 0 | 0.0030 | 6.0 |
| fix-csv-quoted-fields | bugfix, node, parsing, multi-file | 3/3 | 0 | 0.0099 | 8.0 |
| fix-mutable-defaults | bugfix, python, multi-file | 3/3 | 0 | 0.0025 | 5.0 |
| fix-async-batch | bugfix, node, async, multi-file | 3/3 | 0 | 0.0080 | 9.0 |
| fix-stable-sort | bugfix, node, sorting | 3/3 | 0 | 0.0034 | 5.0 |
| fix-money-rounding | bugfix, python, money | 3/3 | 0 | 0.0062 | 7.0 |
| feat-cli-flags | feature, python, cli | 3/3 | 0 | 0.0053 | 7.0 |
| feat-ttl-cache | feature, python, caching | 3/3 | 0 | 0.0065 | 6.0 |
| feat-paginate | feature, node, pagination | 3/3 | 0 | 0.0041 | 5.0 |
| feat-validator-rules | feature, python, validation | 3/3 | 0 | 0.0055 | 7.0 |
| feat-csv-export | feature, python, csv | 3/3 | 0 | 0.0073 | 9.0 |
| refactor-split-function | refactor, python | 3/3 | 0 | 0.0137 | 9.0 |
| refactor-rename-api | refactor, python, api | 3/3 | 0 | 0.0063 | 8.0 |
| tests-merge-intervals | tests, python, mutation | 3/3 | 0 | 0.0053 | 5.0 |
| tests-semver-compare | tests, node, mutation | 3/3 | 0 | 0.0072 | 6.0 |
| debug-cross-module | debugging, python, multi-file | 3/3 | 0 | 0.0040 | 6.0 |
| fix-shell-top-ips | shell, bash, bugfix | 3/3 | 0 | 0.0109 | 10.0 |
| build-inventory-cli | long, greenfield, python, cli | 3/3 | 0 | 0.0379 | 27.0 |

Cost per success: 0.002574. Median cost per successful task: 0.001964. Median latency: 27.2 s. Unpriced results: false.
