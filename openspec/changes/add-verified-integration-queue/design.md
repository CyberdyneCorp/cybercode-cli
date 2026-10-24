# Design

## Owner and repository boundaries

The integration coordinator runs on the owner's existing server under a Location. Inputs are repository-local commit objects selected by the owner, or explicitly fetched contributions from an approved Git remote using existing credential/network/permission rules. The queue never interprets a task message as a shell command or fetches a transcript-supplied URL automatically. Canonical repository identity, full commit IDs and optional hub owner/task/publication identifiers are recorded independently.

Local-only batches need no account. A hub-linked submission requires current membership and repository binding; it must be refreshed on sensitive actions and cannot fall back to a local assertion when the issuer is unavailable. Fixture-backed multi-user tests remain separate from live issuer acceptance.

## Immutable attempts and ordering

Use `intg_` IDs for batches, with a monotonic attempt/revision. A manifest pins the exact base commit, target branch tip, contributions (full commits, author/source attribution, declared dependencies), deterministic order, literal-argv check definitions/timeouts, model roles and budget. Branch names are display metadata, never source identity. Reject duplicate/cyclic contributions, unrelated histories and unsupported nested/submodule content explicitly. Bound the first release to 20 contributions per batch and one active writer per batch; queue capacity defaults to 100 pending batches per server and rejects overflow before acceptance.

Dependencies define a stable topological order; otherwise preserve submission order. Prepare each contribution against the previous combined commit, retaining prefix outcomes so a failed combination is explainable. At minimum the final combined tree runs the full declared verification set even when every contribution passed separately. Required per-contribution/prefix checks may also be requested. Changed dependencies, base, checks or budget bounds produce a new attempt rather than rewriting old evidence.

## Composition and checks

Create a managed worktree and branch without changing the owner's checkout or target branch. Reuse existing safe Git execution and worktree lifecycle. `git merge-tree --write-tree` can discover conflicts without mutating the owner index/worktree; produce commits only for conflict-free trees. Disable Git hooks, external diff/merge helpers, automatic submodule execution and signing prompts in host-owned plumbing. Repository code executes only as explicit check commands under the existing sandbox, trust, network and approval ceilings. The owner chooses checks; contributions cannot replace them or weaken their environment.

Host records bind the complete combined source fingerprint, exact command/environment identity (including explicit toolchain/container reference when available), exit outcome, bounded output digest, timestamps and usage. Missing environment reproducibility is a disclosed limitation, never an implied reproducible-build guarantee. Recheck the source identity after commands so self-modifying checks cannot certify a different tree. Preserve individual and combination failures separately; a failed combination does not prove one author's patch was incorrect.

## Review and evidence

A separate reviewer Session receives the frozen combined diff, relevant source and host check summaries under Plan/read-only ceilings. Contributors cannot mark themselves independent reviewers. Reviewer text is advice, not evidence authority. Missing review produces `review_unavailable`, not a fabricated approval. Present included/excluded contributions, exact base/combined commit, dependency/conflict outcomes, tests/logs, reviewer limitations and actual/estimated/unpriced cost as distinct fields.

Verification status is derived from durable server records. Arbitrary agent JSON and remote reports cannot mint it. Reuse the verified-workflow evidence services without relabeling an integration batch as a solve candidate. Keep records sufficient to audit admission, source identity, checks, review and publication. Transcript retention must preserve required integration evidence references.

## Lifecycle and recovery

Persist batch states queued, preparing, checking, reviewing, ready, conflicted, failed, unknown, cancelled and stale. Keep phase progress separate from verification/review outcomes. Atomically claim queue slots and use optimistic revisions for actions. Cancellation prevents new dispatch and preserves already admitted native outcomes; interruption without settlement becomes unknown, not an automatic rerun. Recovery reconstructs queue state but never repeats an uncertain check or GitHub publication blindly. Explicit rerun creates an attempt with fresh admission and budget.

If the target branch advances, mark the result stale against its pinned target; rebuilding/rechecking creates a new attempt. Passing checks on an old base must not appear current. Do not automatically reorder contributions to hide a failing combination.

## Public surfaces and publication

CLI: `cyber workflows integrations submit|list|show|run|cancel|report|publish`. Submission accepts bounded JSON from a file/stdin for commits/dependencies/check argv, avoiding shell interpolation. The existing workflow API group owns `/api/v1/workflows/integrations` and attempt/action subresources, with normal owner authentication, Location routing, idempotency and generated SDK types. The dashboard presents a queue, manifest/diff review, conflicts, tests, reviewer and cost with explicit action acknowledgements.

Publish requires an owner-reviewed destination repository, integration branch and exact verified attempt. It may push only the prepared branch and open a PR using existing Git/GitHub permission rules. Recheck the target/source and authorization before publication. Persist publication intent and returned remote IDs; after uncertain acknowledgement, reconcile by exact repository/head/attempt identity before retry. Never force-push, update a protected target, auto-merge or publish merely because a model recommends it.

## Validation

Use local Git fixtures with independent patches that pass alone but fail together, textual conflicts, dependency cycles, target advancement, malicious hooks/config and tracked/untracked source mutation. Cover concurrent claims, deterministic ordering, budget stops, failed reviewer, crash during check and lost publication acknowledgement. Real-server CLI/SDK/browser acceptance must use host evidence; a fake service response does not prove verification or authorization. Live hub authorization is pending until the issuer returns.

Git behavior reference: [git-merge-tree](https://git-scm.com/docs/git-merge-tree), [git-worktree](https://git-scm.com/docs/git-worktree).
