# PR auto-fix implementation evidence (2026-10-26)

The experimental self-hosted implementation uses owner-authenticated GitHub polling, durable opt-in subscriptions and queued Session admission. It requires the reserved-budget dependency, explicit token/time caps, exact CI producer identities and owner-pinned verification sources. The agent receives native file tools; a host broker creates isolated checkouts, runs offline verification and performs a non-force push only after retaining the exact candidate and check evidence.

The implementation was rebased onto main `89bd4e49` (PR49) and the reserved-budget branch at `7971b5cc`. Published benchmark reports, frozen manifests and historical private acceptance binaries were not changed.

## Local verification

| Check | Result |
|---|---|
| `cargo test -p cyber-core --lib autofix` | 11 passed: durable dedupe, stale heads, review reauthorization after push/restart, failure limits, CI commit matching, uncertain pushes and stopped controls |
| `cargo test -p cyber-server --lib autofix` | 3 passed: concurrent revision claims, binding/dedupe after database reopen and real projection replay, rollback of failed transitions |
| `cargo test -p cyber-server --test http autofix` | 3 passed: owner authentication/Origin, Location/revision fences, enable key and linked Session validation |
| `cargo test -p cyber-tools --test autofix_restrictions` | 5 passed: canonical path confinement, metadata/policy/pin protection, no extension tools or hooks, no LSP/formatter side effects |
| `cargo test -p cyber-app --lib autofix` | 39 passed; four native acceptance cases excluded by their explicit ignore annotations and run separately below |
| Explicit `cargo test -p cyber-app --lib autofix -- --ignored --test-threads=1` | 4 passed on this macOS host: credential/write confinement; pinned-file rewrite and parent-rename refusal; real offline Rust baseline failure → corrected patch passing pinned tests; controller fix/verify/non-force push to a disposable local Git remote with no repeat |
| CLI PR parsing | 3 passed; enable controls preserve argv and require explicit repository/limits |
| Generated CLI reference | 2 passed after the main merge; `docs/cli.md` matches Clap |
| TUI auto-fix command parsing | 3 passed |
| OpenAPI snapshot and SDK generator consistency | Passed; public types include configuration, status, candidate/test evidence, accounting and backoff |
| TypeScript SDK typecheck/tests | Passed; 119 passed, nine opt-in process tests skipped |
| Strict Clippy | Passed with `-D warnings` for cyber-app, cyber-cli, cyber-core, cyber-server, cyber-tools and cyber-tui, all targets |
| OpenSpec, formatting and whitespace | Strict change/all validation and spec lint run; lint reports no errors and 23 warnings, without assigning their origin. The feature diff passes whitespace checks |

The application tests use finite scripted model streams and an authenticated local fake GitHub service. They cover pagination/ETags, bounded annotations, current-writer review admission, changed identities, retryable rate limits, missing/wrong-producer/pending CI, duplicate events, interrupted recovery, lost push acknowledgements, a live-push/poller race, worker queue timing and backoff. Native process tests use disposable fixture files and repositories; no live GitHub, provider, CyberdyneAuth or desktop action was used.

## Review corrections

Independent review found and corrected live push/poll acknowledgement races, loss of queued reviews after head changes, writable verification sources, and elapsed limits starting before a worker was available. Native verification sources and their containing directories below the checkout are now read-only. A separate verification checkout supplies bounded, redacted baseline failure context; failed candidates retain their commit/check evidence and contribute bounded retry feedback.

Review of the budget prerequisite also corrected immutable channel/MCP attribution and cumulative usage counters. Its own acceptance and limitations are recorded in [reserved budgets](reserved-budgets.md).

## Remaining acceptance and limits

GitHub Actions results belong to the current PR head; local checks are not evidence of remote CI success. Live GitHub mutation acceptance remains pending. Linux native verifier acceptance has not been run here; Windows execution explicitly refuses. The feature remains experimental, with fork writes, GitHub App provisioning, hosted Runner failover and general Channels deferred.

Strict initial production bounds support official stateless text/function OpenAI Responses. ChatGPT/Copilot and other adapters refuse strict mode; these local fixtures do not establish live provider billing or latency. The verifier uses the existing workspace-write sandbox, with known/private credential and IPC paths hidden, rather than a fully hermetic filesystem. Commands must work offline with the available dependencies. Unknown attempts require inspection and a new explicit subscription after disabling the old one; ordinary resume cannot acknowledge uncertain effects.
