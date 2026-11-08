# Design

## Decisions

**Fold, not tables, for decisions.** The runtime keeps each loaded Session as an in-memory fold of its events behind an async mutex. Every mutation appends with `Expected::Seq(last_seq)`, so a second writer can never interleave silently. SQL projections serve listings and diagnostics; they are updated by projectors in the same transaction as the events.

**Locks are short.** The session mutex is held only to read a snapshot or to commit. Provider streams and tool execution run outside it, which lets admissions, steer prompts and interrupts arrive mid-Turn.

**Recovery is the default path.** Every Drain pass starts by settling what is unfinished. Undispatched calls become interrupted. Dispatched read-only calls become interrupted. Dispatched mutations are reconciled through the host and otherwise become `outcome_unknown`. While any outcome is unknown, every mutating tool is refused with a message naming the call. That is conservative: equivalence between calls cannot be established generically. `resolve_unknown` lets the user record success, absence or permission to retry.

**Drains survive defects.** A pass runs inside `catch_unwind`. A panic publishes an `internal` error and always releases the Session, and the next pass recovers the abandoned calls.

**Mid-conversation context.** Context updates and runtime notices are sent as user-role messages wrapped in `<system-reminder>`, because Anthropic and several compatible servers reject system-role messages after the first turn. Updates from replaced epochs are excluded from projection.

**Task state outlives summaries.** Every promoted user prompt is recorded verbatim (up to 2,000 characters) with its message ID. After compaction the summary message carries this list outside the generated prose, so repeated compaction cannot drop a constraint and no summary can grant a permission.

**Hidden calls are billed.** Title and compaction calls add their usage and cost to Session totals, and unpriced steps are counted separately, so the total is a lower bound when any model is unpriced.

## Tradeoffs

A fold per loaded Session costs memory proportional to history; P0 Sessions are small, and the fold can be snapshotted later. Pausing all mutations on one unknown outcome is stricter than necessary, but it never lets a model repeat an effect it cannot see.
