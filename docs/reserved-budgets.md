# Reserved workflow budgets

`enforcement: "soft"` remains the default: it checks recorded spending at existing dispatch boundaries and can overshoot while agents run concurrently. Opt into `"reserved"` to atomically reserve a conservative request allowance across every applicable Session ancestor, workflow run, Goal, Loop, Team and daily scope before provider dispatch. Child agents inherit the captured scopes even when their parent controller ends. Reserved Team limits include the lead; the soft Team reporting exemption remains unchanged.

```json
{
  "budgets": {
    "run": {
      "enforcement": "reserved",
      "max_cost_usd": 5,
      "max_tokens": 2000000,
      "max_wall_seconds": 600,
      "max_agents": 3
    }
  }
}
```

A reservation can be larger than the eventual bill. The first supported production contract is stateless text/function requests to official OpenAI Responses at `https://api.openai.com/v1`, with a positive transmitted output cap, default service tier and disabled truncation. The declared catalog model context bounds the request's input, output and reasoning allowance; it is not an estimate of the actual input length. Input/cached input and output/reasoning use correlated envelopes, and pricing uses the highest applicable catalog tier. Responses reports reasoning within output, which the adapter normalizes into separate classes. Missing optional reasoning/cache-read prices follow the catalog's existing output/input rates. Cache-write usage is absent from this adapter's contract.

These are scheduling limits against declared protocol limits and catalog prices, **not a provider billing guarantee**. Keep catalog model identity, context and prices accurate. Actual usage above the declared estimate remains recorded as an overrun. Opaque media/files, server-side conversation state, built-in billed tools, unknown extensions, alternate routing and incompatible body/model/tier overrides refuse admission. Other production adapters currently return `BudgetBoundUnavailableError`; compatible gateways are not assumed to implement OpenAI's limits. The deterministic scripted test adapter also supports finite bounds.

ChatGPT subscription transport currently strips the output cap, so it cannot supply a reserved request bound. Token-only reserved ChatGPT requests explicitly refuse; USD reserved requests with unavailable pricing return `BudgetUnpricedError` first. Subscription users can continue using **soft** token/time limits, which do not promise strict concurrent reservations. Bundled `solve` retains its USD default: changing to token/time controls does not silently remove that ceiling. The authenticated run override/resume API accepts explicit `null` limit removal; normal workflow/config layers may only tighten ceilings.

Standalone commit/PR text generation has no Session ledger owner and explicitly refuses when a reserved daily budget applies.

Daily accounting follows **receipt commit time** in the server’s local day. A request crossing midnight keeps its outstanding remainder against the same stable daily allocation. Each newly confirmed delta is charged to the day it commits; a cumulative final receipt does not charge earlier deltas again. Uncertain holds continue across subsequent midnights until settlement or an audited release. Dashboard billing timestamps describe those receipt commits, not the original request admission day.

Every title, recap, compaction, automatic permission decision, side chat and Goal review request is covered when a reserved scope applies. A wall deadline cancels opening/stream consumption; cancellation cannot prove a remote provider stopped billing. Successful complete streams settle actual usage and release the remainder in the **same transaction** as the financial event. Partial, cancelled and lost requests retain remaining allocation across restart. A Finish event alone does not establish transport completion. Missing or malformed terminal billing evidence stays unknown instead of becoming a zero-cost receipt. Canonical transcript events keep usage for display and carry accounting linkage so replay, child charges and daily totals do not charge twice.

## Inspect and reconcile

The dashboard Work monitor shows the current Session's latest 50 request allocations, bounds, actuals and uncertainty. The owner API provides cursor pagination and scope filtering, including descendant requests charged to an ancestor scope:

```sh
cyber api v1.budgets.reservations --param open_only=true
cyber api v1.budgets.reservations --param scope_id=ses_example
cyber api v1.budgets.reservation --param id=breq_example
```

After checking the provider outcome, an authenticated owner can release an inactive allocation or supply a complete cumulative receipt. Review the current revision first and reuse the same operation ID after a lost acknowledgement:

```sh
cyber api v1.budgets.reconcile --param id=breq_example -d '{"operation_id":"review-001","expected_revision":2,"reason":"Provider confirms this request did not run","action":{"kind":"release"}}'
```

Settlement uses `action: {"kind":"settle","actual":{"tokens":{"input":100,"output":10,"reasoning":0,"cache_read":0,"cache_write":0},"turns":1,"cost_usd":0.00012,"complete":true}}`. Evidence must be cumulative and cannot erase previously confirmed usage. Live local attempts cannot be reconciled. Release removes the remaining hold without deleting confirmed charges. Revision conflicts require another review; operation IDs cannot be reused for a different decision.

The ledger retains owner reasons, timestamps, request IDs, model identity, bound source and normalized-body digest. It never stores provider credentials or request bodies. Linked usage receipts remain durable rather than folding away their request identity. Sessions with unresolved reservations cannot be deleted; transcript retention must preserve their evidence. Keep the Session metadata until billing is resolved: after an audited release followed by full Session deletion, the current owner reconciliation API cannot append a later native billing correction and returns `SessionNotFoundError` (the raw reservation audit remains retained). Historical failed/folded accounting that cannot establish spending fails closed in reserved mode. Audited release is an explicit acknowledgement of uncertainty, not proof of zero cost.

The soft-mode paths retain their earlier accounting behavior: some failed title/recap/compaction/side-chat attempts lack partial usage receipts, and Goal review's legacy token attribution remains coarse. Reserved receipts do not retroactively reconstruct those historical calls.
