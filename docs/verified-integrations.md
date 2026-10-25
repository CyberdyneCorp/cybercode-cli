# Verify complementary contributions

The integration queue combines **different changes** into one isolated checkout and runs owner-selected checks against that exact combined source. Use `solve` when several agents are proposing alternative solutions to one issue; use `integrations` when their changes should ship together.

Start from a clean Git checkout whose HEAD matches the local target branch. Contributions must already exist as full local commit hashes in the same Git history. This first version does not fetch contributor URLs, materialize submodules, copy `.worktreeinclude`, or accept changes to Cyber configuration. Repository-local filters, custom merge drivers, URL rewrites, partial-clone fetchers, worktree-specific Git configuration (`extensions.worktreeConfig`) and included Git configuration are refused. Native Git/GitHub commands resolve absolute executables from the host’s launch PATH outside project and managed contribution sources; project-configured PATH entries cannot replace this publication authority. Native authority ignores `sandbox.env.set` overlays and suppresses runtime-loader/config execution overrides (including `LD_*`, `DYLD_*` and `GH_CONFIG_DIR`); ordinary verification commands retain their configured environment. Managed Git commands disable lazy fetching and replacement objects; fetch required objects explicitly before admission.

Create `integration.json`:

```json
{
  "target_branch": "main",
  "contributions": [
    { "label": "api", "commit": "REPLACE_WITH_FULL_LOCAL_COMMIT_HASH" },
    { "label": "ui", "commit": "REPLACE_WITH_ANOTHER_FULL_LOCAL_COMMIT_HASH", "depends_on": ["api"] }
  ],
  "checks": [{ "argv": ["cargo", "test"], "timeout_seconds": 300 }],
  "reviewer_model": "verify"
}
```

```sh
cyber workflows integrations submit integration.json
cyber workflows integrations list
cyber workflows integrations run intg_BATCH_ID --wait
cyber workflows integrations report intg_BATCH_ID --format json
```

Submission prepares an immutable manifest without executing checks or starting a model. A run creates a new isolated managed worktree and a separate durable workflow attempt. Dependencies determine a stable order; cycles, missing labels and duplicate commits are rejected. A target change requires a new submission. Explicitly rerunning the same batch creates a fresh attempt and budget while retaining previous run IDs.

The **Integrations** dashboard tab provides the same queue, JSON manifest submission, selected report, run/cancel controls and publication review. Refresh selected evidence after a run completes. Owner server authentication and the current Location govern all operations. Browser credentials and queue data clear on logout; contributor text, patches, logs and URLs render as inert text.

Checks use literal argv, the owner's pinned configuration, existing permission rules, hooks and OS sandbox. Contributor files cannot replace these settings. Noninteractive runs cannot answer a new permission prompt: preapprove the specific commands in your trusted global configuration or approve through an attached client. A blocked command is reported as blocked, never as passing. Checks that alter source cannot certify it; before/after fingerprints bind every settled check to the combined patch and actual prepared launch environment.

The reviewer runs in a separate Plan/read-only Session. Its recommendation is advice. A missing or failed reviewer is explicitly `review_unavailable`; it cannot manufacture host verification. Reports include source/base/combined hashes, combined diff, prefix commits, individual command outcomes and logs (16 KiB previews; full bounded native records retain their aggregate IDs), reviewer output and actual priced usage with unpriced/incomplete usage flags. Cost prediction is explicitly unavailable (`estimated_usd: null`); the configured soft budget is a ceiling, not a predicted bill. Local toolchains are not a reproducible container: environment identity documents what launched, not guaranteed reproducibility elsewhere.

Queued batches are limited to 100, each with at most 20 contributions and eight checks. Finished worktrees and evidence are retained for inspection. Cancellation blocks new native phases and preserves admitted outcomes. An intent without a settlement remains unknown; recovery never blindly repeats an uncertain check or remote mutation. A changed base, integration branch or source makes evidence stale. Create a new batch/attempt after investigating an unknown outcome.

## Optional collaboration attribution

A contribution may include `"hub": {"project":"PROJECT_ID", "task":"TASK_ID", "agent":"AGENT_ID"}`. The project must match the repository's pinned collaboration binding. Task and agent references must belong to that project, and the human principal and referenced agent owner must still be authorized. Authorization performs fresh issuer checks at admission and publication; cached membership does not override revocation. Local commits without hub attribution need no Cyber Account. Hub-linked operations fail closed when CyberdyneAuth is unavailable. Live issuer acceptance remains pending while the service is offline.

## Explicit publication

Review the combined patch, checks, limitations, costs and destination first. Authenticate your local `gh` client and permit the required Git/GitHub commands and network domains. Publication follows existing sandbox credential grants: a trusted `sandbox.readable_paths` grant for the `gh` configuration directory, or `sandbox.env.allow` for a narrowly scoped `GH_TOKEN` in the server’s launch environment, may be needed. Those profile grants also apply to its check commands; set them before submission so the pinned configuration stays stable. See [sandbox configuration](local-core-guide.md#sandbox-profiles-environment-and-escalation). Publication supports a single credential-free standard `github.com` HTTPS or SSH push destination on an existing configured remote:

```sh
cyber workflows integrations publish intg_BATCH_ID \
  --remote origin --repository Team/Repo \
  --title 'Integrate API and UI changes' --body-file pr-body.md \
  --operation-id reviewed-publication-1
```

The server rechecks source, target and optional hub authorization, pushes only the exact verified integration commit to its integration branch, and opens a PR. It never merges, updates the target branch or force-pushes. If review is unavailable, publication requires `--acknowledge-review-unavailable` after owner inspection.

Durable publication intent binds the repository, head, commit, target and PR content. After an interrupted acknowledgement, repeat the exact reviewed content to reconcile an existing PR by repository/head/commit. If no matching remote PR can be established, the result stays unresolved and no automatic mutating retry occurs. Queue show/report/list expose the current attempt’s durable publication receipt aggregate, destination and bounded phase/settlement previews even when publication is unknown. At most 16 records and 16 KiB per record are previewed; truncation points to the retained full aggregate. A failed or uncertain phase remains unknown, and reading receipts never repeats a push or PR creation. Inspect that evidence and remote state before choosing a new attempt.

API clients use `/api/v1/workflows/integrations` and `/{id}/report|run|cancel|publish`; actions include `expected_revision`. Use `Idempotency-Key` for mutation retries. The owner-only APIs are also included in OpenAPI and the generated TypeScript SDK.
