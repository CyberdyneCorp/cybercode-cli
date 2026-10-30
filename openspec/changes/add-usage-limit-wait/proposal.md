## Why

A subscription plan (ChatGPT Plus/Pro through `chatgpt/*`) answers 429 `usage_limit_reached` with the time the limit resets. Since `improve-model-protocol-fidelity` this is a typed `QuotaExceeded` error with `resets_at`, and the TUI and `exec` say "usage limit reached; resets at 14:20". The Turn still fails.

That is right for a person at the keyboard, but wrong for work nobody watches. A nightly `cyber exec`, a Goal, a loop iteration, a workflow agent or a teammate stops for good at 02:10 although the plan resets at 02:40, and the run has to be restarted by hand in the morning. None of the `goals`, `loops-scheduling`, `workflows`, `exec-mode` or `session-runtime` specs says to wait for the reset and continue.

## What Changes

- **Usage limit wait** (`session-runtime`, added "Usage limit wait"). When a Turn fails with `QuotaExceeded` before any output, and the failure would end the Turn, an unattended run waits until the limit resets and then retries the Turn:
  - it waits only when the provider said when the limit resets and the reset lies within `usage_limit.max_wait` (default 6 h);
  - it resumes a 10 s margin after the reset, plus up to 30 s of per-Session jitter so many runs do not retry at the same instant;
  - otherwise it fails as today, and the failure says why it did not wait.
- **Who waits.** `usage_limit.mode: unattended` (the default) waits in non-interactive contexts: `cyber exec`, a Session with an active Goal or a running loop iteration (and so routines, which run as those), workflow agents, teammates, and subagents whose owner is one of these. `always` also waits in interactive Sessions; `never` never waits.
- **Visible and durable.** The wait is recorded as `session.usage_limit.waiting.1` with `resets_at_ms` and `resume_at_ms`, and ends with `session.usage_limit.resumed.1` or `session.usage_limit.abandoned.1`. The Session view carries the pending wait. The normal interrupt cancels it.
- **Restart** (`session-runtime`, added "Usage limit wait recovery"). After a restart a top-level Session's wait resumes for the time left. A child, a workflow agent or a teammate ends its wait cleanly (`abandoned`, reason `restarted`), because its owner's own recovery decides what runs next. A Session is never left waiting with no Drain.
- **Deadlines.** The wait is wall time. When the planned resume falls past a wall deadline (a Session or Workflow Run `max_wall_seconds`, a Goal's or a loop's `max_wall_seconds`, `cyber exec --timeout`), the run does not wait and ends with that deadline's outcome.
- **Configuration** (`configuration`, added "Usage limit settings"). New top-level key `usage_limit` with `mode` and `max_wait`.
- **`exec`** (`exec-mode`, added "Usage limit waits in exec"). A `usage_limit` stream-json line for each change, a stderr line in text mode, and `--timeout` ends the run at once when the planned resume falls after it.
- **TUI** (`tui`, added "Resume at usage limit reset"). An interactive Session still fails and says when the limit resets, and the notice offers `/resume-at-reset`. That command schedules this Session to retry its Turn after the reset. The footer shows a pending wait.
- **Web dashboard** (`web-client`, added "Usage limit wait notice"). A notice with the resume time while a wait is pending.
- **Goals, loops and workflows** (added one requirement each). The wait does not pause a Goal, counts against its wall budget, and the same for loops. A workflow agent keeps its slot while it waits (see design).
- **API.** `POST /api/v1/sessions/{id}/usage-limit/resume` (`v1.session.resumeAtReset`) schedules the wait for an idle Session whose last Turn failed on a usage limit; `POST /sessions` accepts `unattended: true`, which `cyber exec` sets on the Sessions it creates.

## Non-goals

- **The model fallback chain.** `provider-catalog` → Model fallback chain runs first when it is implemented; it is not implemented yet (`fallback_models` is only a config key), so this change waits wherever a `QuotaExceeded` failure ends the Turn today.
- **Waiting on rate limits.** `RateLimit` (429 without a usage limit) keeps the transient retry policy.
- **Waiting without a known reset.** A provider that does not say when the limit resets fails as today.
- **Freeing workflow agent and subagent slots during a wait** (see design for why).
- **A countdown UI.** The TUI and web show the resume time, not a live countdown.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `session-runtime`: added "Usage limit wait" and "Usage limit wait recovery".
- `configuration`: added "Usage limit settings".
- `exec-mode`: added "Usage limit waits in exec".
- `tui`: added "Resume at usage limit reset".
- `web-client`: added "Usage limit wait notice".
- `goals`: added "Usage limit waits under a Goal".
- `loops-scheduling`: added "Usage limit waits in loop iterations".
- `workflows`: added "Usage limit waits of workflow agents".

Only requirements are added, so this change cannot conflict with the open changes that modify "Top-level keys" or "Stream JSON event schema"; "Usage limit settings" adds the key, and "Usage limit waits in exec" adds the stream-json type as the schema's additive rule allows.

## Impact

- **Code:**
  - `cyber-core`: `usage_limit.rs` (settings, the pure wait decision), config validation and known keys;
  - `cyber-llm`: `LlmError::usage_limit`, the reset-time text, a scripted usage-limit step;
  - `cyber-server`: `runtime/usage_wait.rs` (events, state, clock, decision inputs, wait, restart recovery, `resume_at_reset`), the Drain (`drain.rs`), `StepFailed.resets_at_ms`, `SessionInfo.unattended`, the Session view, the HTTP route and OpenAPI;
  - `cyber-store`: migration `session_usage_wait`;
  - `cyber-app`, `cyber-tools`: settings from configuration, startup recovery;
  - `cyber-cli`: exec output and `--timeout`;
  - `cyber-tui`: `/resume-at-reset`, the error hint, the footer badge;
  - `web/`: the notice; `sdk/`: regenerated types.
- **Docs:** `docs/local-core-guide.md` (Usage limits), `docs/features.md`, `ROADMAP.md`.
- **Dependencies:** none.
