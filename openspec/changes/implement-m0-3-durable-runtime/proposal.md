# Implement M0.3: durable runtime

## Why

Milestone M0.3 (ROADMAP) turns the adapters into an agent that survives crashes. It covers inbox admission with steer and queue delivery, Drains, interrupt, retry and error classification, usage and cost, compaction, and Context Epochs. Tools arrive in M0.4, so the runtime defines the tool host contract and the recovery rules now.

## What Changes

- `cyber-server::runtime`:
  - Session state is a fold of durable events. Projections for the session list, the inbox and tool calls are written in the same transaction. Every append uses an expected sequence number.
  - Admission is idempotent: an exact retry returns the original receipt and a conflict is rejected. `steer`, `queue` and `hold` delivery are supported. The inbox supports edit, remove, release and refuse.
  - Drains: one per Session, coalesced wakes, forced resume, and Safe Boundary promotion in spec order.
  - Turns: base prompt plus a stable baseline, projected history and the advertised tools. Reasoning is sent natively to the same model and lowered to text otherwise. Each complete tool call is recorded before dispatch. Unknown, stale, invalid and case-repaired calls settle without running. Concurrency-safe neighbours run in parallel. A final tool-less Turn applies at the step limit. Usage, cost, and live delta, usage and retry events are published.
  - Tool recovery contract: dispatch records carry the input digest and operation key. Interrupt settles undispatched calls as interrupted and dispatched mutations as `outcome_unknown`. After a crash, read-only reconciliation runs. Mutating tools pause while any outcome is unknown, until the user resolves it. Tool crashes never reach the model.
  - Context Epochs: date, environment and instruction sources. A blocked first Turn keeps the prompt. Updates are reconciled at Safe Boundaries. New epochs start after compaction, after a provider-family switch and on repair.
  - Compaction: automatic, overflow-triggered (once) and manual, deferred to the Safe Boundary. The verbatim tail never splits tool pairs. The summary carries durable task state with source IDs. Media is stripped on overflow, automatic compaction continues the work, and failure is reported clearly.
  - Title generation, rename, archive, fork, cascading delete, list paging, and model, agent and mode switches.
- The store gains the `session`, `session_input` and `tool_call` projections and a public read API.
- An evaluation fault class `task_abort` and four new recovery cases linked to their requirements.
- Spec clarification: Mid-Conversation System Messages are user-role messages wrapped in `<system-reminder>`.

## Impact

No HTTP surface yet (M0.5). Several items are deferred:
- User shell commands (`!`) need the shell tool and sandbox (M0.4).
- Durable task state records the objective and every user instruction with its source. Model-proposed decisions, open questions and verification evidence need the M0.4 tool surface.
- Retention GC, backup bundles and logging remain for the end of P0.
