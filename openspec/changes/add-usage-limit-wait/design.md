## Context

- `cyber-llm/src/error.rs` maps ChatGPT `usage_limit_reached` to `ErrorKind::QuotaExceeded` with `resets_at` (Unix seconds) and `plan_type`. `QuotaExceeded` is not retryable, so `open_with_retry` returns it at once.
- In the Drain (`cyber-server/src/runtime/drain.rs`), an error before any output goes `turn → fail_opening_step → fail_step`. `fail_step` commits `session.step.failed.1`, publishes `session.error` and returns `TurnEnd::Failed`; `pass_owned` then pauses an active Goal and runs StopFailure hooks. An error after output (`consume`) settles the Turn and is never replayed.
- Budget reservations are released when the provider call fails to open (`reservations/dispatch.rs`: `attempt.ended(false)` and `flush`), so nothing is reserved after `fail_opening_step`.
- Wall deadlines live in four places: Session and ancestor budgets (`session_budget`, `activated_ms + max_wall_seconds`), the Workflow Run budget (`workflow_run_budget`, wall time without paused segments), a Goal's budget (`activated_ms + max_wall_seconds`) and a loop's budget (`created_ms + max_wall_seconds`). `cyber exec --timeout` is a client timer.
- Drains are not restarted at boot. Goals, loops, teams and workflow runs each have their own startup recovery (`cyber-app/src/lib.rs`).
- `provider-catalog` → Model fallback chain names `QuotaExceeded` as a reason to fall back, but there is no fallback runtime yet (`docs/implementation/p1-acceptance-plan.md`: class d).

## Goals / Non-goals

**Goals:** unattended runs survive a usage limit with a known reset; the wait is durable, visible, cancellable and bounded; deadlines win; interactive users decide for themselves; restart never leaves a Session stuck.

**Non-goals:** see the proposal.

## Decisions

### 1. Where the decision sits: in `fail_step`, after the fallback chain

The wait is decided where a pre-output `QuotaExceeded` would fail the Turn: in `fail_step`, after `open_with_retry` returned. When the fallback chain is implemented it runs inside the opening of the Turn, before `fail_step`, so it keeps precedence: a fallback model that answers never reaches the wait. Until then the order is trivially right. An error after output is never waited on, as it is never replayed.

### 2. The decision is a pure function in `cyber-core`

`cyber_core::usage_limit::plan(settings, facts) -> WaitPlan` takes:
- `mode` and `max_wait` from `usage_limit`;
- `unattended`: whether the Session has an interactive owner (decision 3);
- `resets_at_ms` from the error;
- `now_ms`, from the runtime's wait clock (decision 8);
- `since_ms`: when the first wait of this streak started (decision 6);
- `deadline_ms`: the earliest wall deadline (decision 5);
- `jitter_ms`.

It returns `Wait { resume_at_ms }` or `Fail(NoWait)`. In order: `never` → `Disabled`; `unattended` mode and an interactive Session → `Interactive`; no reset → `UnknownReset`; `resets_at - since > max_wait` → `BeyondMaxWait`; `resume_at ≥ deadline` → `PastDeadline`. `resume_at = max(resets_at, now) + 10 s + jitter`. The jitter is FNV-1a of the Session ID and the reset time, modulo 30 s: deterministic per Session (testable, stable over a restart) and spread across Sessions. Every `NoWait` except `Interactive` has a note, appended to the failure message: "usage limit reached; resets at 14:20 (not waiting: the reset is more than usage_limit.max_wait (6h) away)".

### 3. Who is unattended

A Session is unattended when any of these holds:
1. the runtime is not interactive (`RuntimeOptions.interactive: false`: `cyber exec --embedded/--ephemeral`, evals);
2. it was created with `unattended: true`, which `cyber exec` sets on the Sessions it creates (a shared server is interactive, so exec needs its own marker);
3. it is a workflow agent (`workflow_run_id`);
4. it has an active Goal;
5. one of its loops has an iteration in flight (a routine runs as a Goal or loop Session, so it is covered);
6. it is a teammate (a non-lead team member works from the task list with nobody attached);
7. it is a subagent whose parent is unattended (walked up to 32 levels).

Otherwise it is interactive. `cyber exec -s <id>` on a Session made elsewhere follows that Session's context; `usage_limit.mode: always` makes it wait anyway. Presence leases are not used: on a legacy interactive server every Session counts as attended, so presence cannot tell an unwatched Session apart.

### 4. Events, state and projection

- `session.usage_limit.waiting.1 { message_id, resets_at_ms, resume_at_ms, started_ms, by }`: `by` is `runtime` or `user` (`/resume-at-reset`). It commits in the same append as the `session.step.failed.1` of the Turn, so a crash never leaves a failed step without its wait, or a wait without its failure.
- `session.usage_limit.resumed.1 { message_id, resumed_ms }`.
- `session.usage_limit.abandoned.1 { message_id, reason, abandoned_ms }`, with `reason` `interrupted`, `restarted` or `deadline`.

`session.step.failed.1` gains an optional `resets_at_ms`. The Session state folds a `pending` wait, the streak start and the last usage-limit failure (for `/resume-at-reset`). The projection `session_usage_wait(session_id, message_id, resume_at_ms)` (migration `20261030000000_usage_limit_wait`) lists pending waits for the startup recovery without loading every Session; it is rebuildable from events. The HTTP Session view gains `usage_limit` (the pending wait, else null).

### 5. Deadlines

`deadline_ms` is the earliest of:
- each Session budget scope of the Session and its ancestors: `activated_ms + max_wall_seconds` (now, when not yet activated);
- each Workflow Run scope joined through `workflow_run_id`: its effective start (paused time excluded) `+ max_wall_seconds`;
- the active Goal: `activated_ms + max_wall_seconds`;
- each active loop with an iteration in flight: `created_ms + max_wall_seconds`.

When `PastDeadline` wins, the run ends with that deadline's own outcome, in the same append as the failed step:
- a budget scope: the Drain fails with `BudgetExceeded { scope, limit: max_wall_seconds }`, as when the wall budget expires (`exec` exits 4, a workflow agent fails with its budget error);
- a Goal: `session.goal.ended.1` with `budget_exceeded`, and the Goal notification;
- a loop: `loop.stopped.1` with `budget_exceeded`.

`cyber exec --timeout` is a client timer the server cannot see. Exec reads `resume_at_ms` from the `waiting` event; when it falls after the timeout it stops the run at once with `stop_reason: timeout` (exit 4), and the interrupt abandons the wait.

### 6. The wait and the retry

`fail_step` returns `TurnEnd::UsageLimit` instead of `Failed`: no `session.error`, no Goal pause, no StopFailure. `pass_owned` then awaits the wait (`await_usage_reset`):
- the Drain's cancel token (interrupt) → `abandoned { reason: interrupted }`, the Goal pauses as for any interrupted Turn, the Drain ends;
- shutdown (the runtime's `closed` token is cancelled before Drains are) → nothing is written, the wait stays pending for the restart;
- the clock reaches `resume_at_ms` → `resumed`, and the pass continues as a forced Turn (`first = true`, `continue_tools` kept).

The retried Turn is built by `prepare_turn` from the same durable history, so it sends the same request: the failed step adds nothing the model sees, and the replay rules of `retry.rs` and `view.rs` apply unchanged. This is a deliberate refinement of "resend the same request object": rebuilding is what makes the retry survive a restart, and it differs only when the user steered during the wait, where promoting that input at the Safe Boundary is the documented behavior. The retry is one new provider attempt through a new `open_with_retry`, with a fresh 4-retry transient budget. Budget and wall checks run again before it (`check_budget`).

A second `QuotaExceeded` after a wait decides again; `max_wait` counts from the streak start (`since_ms`), which a successful step or an abandoned wait clears. A Session can therefore never wait more than `max_wait` in total for one Turn.

### 7. Slots and reservations during a wait

- **Budget reservations:** none is held: the failed open settles its reservation before `fail_step`. The retry reserves anew.
- **Workflow agent slots, `agents.max_concurrent` slots and run slots are kept.** The wait is part of the agent's (or subagent's) execution: the `agent()` call stays unsettled, so its slot stays taken. Releasing it would need a new suspended state in the fair scheduler (it never cancels or preempts a running agent) and re-queuing on resume, and it would buy nothing: a usage limit belongs to the subscription account, so any other agent granted the freed slot on the same provider credential hits the same limit at once and starts waiting too. A run whose other agents use another provider keeps those agents' own slots. This is the precise reason it must hold the slot.

### 8. The wait clock

`Inner` owns a `WaitClock` (`now_ms`, `sleep`): the system clock by default, replaced through `Runtime::configure_wait_clock` in tests. Debug builds also read `CYBER_TEST_USAGE_WAIT_SCALE=<n>`, which divides every wait by `n`, so the real-binary `exec` test waits a 40-minute reset in a few hundred milliseconds. Release builds never read it.

### 9. Restart

Startup (`cyber-app`, before goal recovery so a resumed Session is already running when goal recovery looks) calls `Runtime::recover_usage_limit_waits`. For each pending wait:
- a top-level Session (no parent, workflow run or team) is resumed with a forced Drain. The pass sees the pending wait first, waits for the time left (none if it passed) and retries. The deadline is checked again first; a wait now past it is abandoned (`deadline`) and ends with the deadline outcome.
- a subagent, workflow agent or teammate is abandoned (`restarted`) with a `session.error`, because its owner's recovery decides: `recover_jobs` fails the delegated Job, a running workflow run comes back paused and its resume relaunches the agent, and `recover_teams` reports the teammate idle to its lead.

### 10. Interactive TUI

An interactive Session fails as today. The TUI's error notice for a `quota_exceeded` failure with a known reset ends with `/resume-at-reset to continue then`. `/resume-at-reset` calls `POST /sessions/{id}/usage-limit/resume`, which:
- refuses a running Session (`SessionBusyError`) and a Session whose last finished step is not a usage-limit failure with a known reset (`InvalidRequestError`);
- plans with mode `always` (the user asked) but keeps `max_wait` and deadlines, refusing with the note;
- commits `waiting { by: user }` and starts a forced Drain, which waits like any other.

The footer shows `⏸ usage limit · resumes 14:31` while a wait is pending, and the interrupt key cancels it. `/resume-at-reset` is a command rather than a key in the toast, because toasts carry no actions in this TUI.

### 11. Configuration placement

`usage_limit` is a top-level key, not under `providers`: `providers` is keyed by provider ID and trust-gated, and the policy is about how runs react, not about an endpoint. It is not sensitive (it can only make a run wait, bounded by `max_wait` ≤ 7 d), so project configuration may set it.

## Risks / Trade-offs

- **A Session stays `running` during a wait.** That is intended: it holds its Drain, so new input queues behind the retry and the interrupt cancels the wait. Clients show the wait so a long-running Session is explained.
- **Clock skew** between the provider and the host: the 10 s margin covers small skew; a retry that still hits the limit decides again within `max_wait`.
- **Stale projection rows** (a crash between append and projection cannot happen: the projector runs in the append's transaction). The recovery still checks the folded state and skips a row whose Session has no pending wait.
