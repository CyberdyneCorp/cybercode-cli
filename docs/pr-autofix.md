# PR auto-fix subscriptions

PR auto-fix watches an explicitly subscribed GitHub pull request, queues failed CI or new review feedback, and attempts a verified fix in an isolated checkout. It pushes a fix commit to the PR branch and waits for CI on that exact commit. It never merges the PR.

**Development status:** experimental implementation on the auto-fix feature branch; validation and live GitHub acceptance are pending. The broader P3 GitHub App, fork writes and cloud Runner failover remain planned.

## Requirements

- A reachable self-hosted Cyber server on macOS or Linux, with the native sandbox helper available. Windows execution is explicitly refused pending native verifier acceptance.
- GitHub CLI installed and signed in on the **server machine**: `gh auth login --hostname github.com`. The credential needs repository write access, read access to checks, commit statuses and PR reviews, and permission to post the exhaustion notice. Only github.com is supported in this release.
- An open PR whose head and base repositories are the same. Fork PRs are refused.
- A linked owner Session in the current trusted Location, an explicitly selected model, verification commands and paths, and token/time limits.
- A model adapter with strict reservation bounds. The initial production adapter is official OpenAI Responses; ChatGPT, Copilot and other adapters currently refuse this strict workflow rather than silently using soft token limits. Provider login and GitHub login are separate. The server caches its GitHub connection for its lifetime; after changing that login, restart the server before resuming. A different GitHub identity requires a new explicit subscription.
- Hooks, MCP servers and plugins disabled for the Location. Auto-fix attempts expose native file tools; the host executes pinned verification commands separately. Baseline checks run in a disposable copy and return bounded, redacted failure context before the agent edits.

The server owns subscriptions. Closing the CLI leaves the subscription enabled while the server remains available. A stopped server does no work; restart observes the current PR head. Cloud failover is not included.

## Enable a subscription

Start or select the owner Session first. Supply its `ses_…` ID explicitly; enabling auto-fix does not switch your current checkout or launch another TUI.

Choose verification commands and protect their sources from agent edits. `--autofix-check` accepts a JSON argv array; it does not split a shell command. Repeat it to run multiple checks. Pin tests, scripts, manifests and configuration that determine what those commands verify with repeated `--autofix-verification-path` options. Paths are repository relative and must already exist.

```bash
cyber pr 123 --autofix \
  --autofix-repository your-org/your-repo \
  --autofix-session ses_OWNER \
  --autofix-model openai/gpt-4.1 \
  --autofix-check '["cargo","test","--locked"]' \
  --autofix-verification-path tests \
  --autofix-verification-path Cargo.toml \
  --autofix-verification-path Cargo.lock \
  --autofix-required-ci 'check:APP_ID:Rust tests' \
  --autofix-max-tokens 2000000 \
  --autofix-max-seconds 600 \
  --autofix-max-attempts 3
```

Replace the repository, Session, paths and CI identity with actual values. A full `https://github.com/your-org/your-repo/pull/123` URL can replace the PR number and `--autofix-repository`.

Required CI identities bind the provider as well as its check name: `check:<app ID>:<name>` or `status:<creator ID>:<context>`. Inspect existing PR checks to obtain the real identities. For example, with GitHub CLI:

```bash
HEAD_SHA=$(gh pr view 123 --repo your-org/your-repo --json headRefOid --jq .headRefOid)
gh api "repos/your-org/your-repo/commits/$HEAD_SHA/check-runs" \
  --jq '.check_runs[] | "check:\(.app.id):\(.name)"'
gh api "repos/your-org/your-repo/commits/$HEAD_SHA/statuses" \
  --jq '.[] | "status:\(.creator.id):\(.context)"'
```

Repeat `--autofix-required-ci` for every result needed to accept a fix. No checks, a missing required identity, pending results or incomplete API observations remain awaiting evidence. A push alone never resets the failure counter. Additional observed pending or failed checks also prevent success.

Token admission uses conservative adapter bounds, which can exceed typical actual usage. A cap smaller than one bounded request refuses dispatch. Actual usage and unpriced costs remain distinguishable in status. The elapsed-time cap starts when a worker claims the attempt and includes preparation, inference, verification and publication; time spent waiting for another PR’s worker does not consume it. A timeout with unresolved effects pauses for inspection.

## Inspect and control

```bash
cyber pr 123 --autofix-status --autofix-repository your-org/your-repo
cyber --format json pr 123 --autofix-status --autofix-repository your-org/your-repo
cyber pr 123 --autofix-pause --autofix-repository your-org/your-repo
cyber pr 123 --autofix-resume --autofix-repository your-org/your-repo
cyber pr 123 --autofix-off --autofix-repository your-org/your-repo
```

JSON status includes attempt stages, source and resulting commits, verification evidence, limits, usage and unknown outcomes. Resume refuses an unresolved active attempt or an exhausted/closed subscription. After an unknown outcome, inspect the retained Session, reservation and push evidence; disable the old subscription and explicitly enable a new one to authorize further work. Ordinary resume does not acknowledge or discard uncertainty. Disabling fences future work; an external action dispatched before disable can still finish and must be reconciled visibly.

In the linked TUI Session, use `/autofix status`, `/autofix pause`, `/autofix resume` or `/autofix off`. To enable from that Session, use `/autofix on` followed by one JSON object:

```text
/autofix on {"repository":"your-org/your-repo","pr":123,"checks":[["cargo","test","--locked"]],"verification_paths":["tests","Cargo.toml","Cargo.lock"],"required_ci":["check:APP_ID:Rust tests"],"max_tokens":2000000,"max_seconds":600}
```

The TUI binds the current Session and defaults the model to its selected model and consecutive failures to three. Set `model` or `max_attempts` in the object to override those defaults. A model without strict reservation support is refused before enable.

Owner API and generated SDK operations use the same contract:

| Operation | Route |
|---|---|
| List / enable | `GET` / `POST /api/v1/autofix/subscriptions` |
| Inspect | `GET /api/v1/autofix/subscriptions/{id}` |
| Pause / resume / disable | `POST /api/v1/autofix/subscriptions/{id}/control` |

Enable requires an `Idempotency-Key`. Controls require `expected_revision` and an `action` of `pause`, `resume` or `disable`. All operations require owner access and the matching Location; remote Session grants do not authorize them.

## Safety and current limits

CI logs and review text are untrusted context. Review feedback triggers attempts only for current repository writers; self and bot feedback is ignored. Historical review revisions are recorded at enable time without launching fixes. Duplicate delivery and restart do not create another attempt for the same event.

The host checks the exact PR head and current authorization before execution and publication. Pending reviews that cross a head change are revalidated against the current author permissions and revision before another attempt. It protects Git metadata, policy/configuration and explicitly pinned verification paths, requires a nonempty verified patch, and publishes with an ordinary non-force push. Branch movement pauses work. A lost push acknowledgement is reconciled against the exact intended commit before any further action.

Three consecutive failed attempts exhaust a subscription by default. Failed verification, unusable patches and failed CI count once each; unknown outcomes pause instead of triggering another fix. Closing or merging the PR stops the subscription. Exhaustion posts one notice with a stable reconciliation marker.

Pinned verification paths are read-only during verification as well as protected from model edits. For nested files, their containing directories are conservatively read-only too, preventing rename escapes. Pinned verification paths are owner-selected protection, not a complete inferred dependency graph. Select commands and paths that establish the behavior you need; local passing tests do not guarantee a correct patch. Subscription history and event batches are bounded, and reaching a capacity limit requires owner attention.

The verifier uses the existing workspace-write sandbox with known credential stores, broker IPC, custom config/state and sibling attempts hidden. It is not a fully hermetic filesystem and cannot identify arbitrary secrets in arbitrary files. Offline checks may fail when dependencies or hidden user configuration are required.

Live GitHub mutation, platform acceptance and CI evidence must be recorded before treating this feature as merge-ready. See the [OpenSpec tasks](../openspec/changes/implement-pr-autofix-subscriptions/tasks.md).
