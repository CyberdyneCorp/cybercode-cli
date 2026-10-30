## 1. Settings and decision

- [x] 1.1 Add `cyber-core/src/usage_limit.rs`: `UsageLimitSettings::from_config` (`mode`, `max_wait`), `plan`, `NoWait::note`, `jitter_ms`. Register `usage_limit` in the known keys and the settings validation.
  - Unit tests: defaults, every field error, and the decision table over mode × context × `resets_at` × `max_wait` × deadline, including the streak start, a reset in the past, the margin and the jitter range; the wait fold and the unattended markers (`usage_wait.rs`).

## 2. Provider error and scripted step

- [x] 2.1 Add `LlmError::usage_limit(resets_at)` and a public reset-time text; add `ScriptStep::UsageLimit { resets_at }` to the scripted adapter.

## 3. Runtime

- [x] 3.1 Add `runtime/usage_wait.rs`: the three events and their payloads, their fold into the Session state (pending wait, streak start, last usage-limit failure), the `session_usage_wait` projection and migration, and `StepFailed.resets_at_ms`.
- [x] 3.2 Add the wait clock (`WaitClock`, `Runtime::configure_wait_clock`, the debug-only `CYBER_TEST_USAGE_WAIT_SCALE`).
- [x] 3.3 Decide in `fail_step`: the unattended context, the wall deadline (Session/run budgets, Goal, loop), the plan; commit the wait with the failed step, or the failure with its note and the deadline outcome.
- [x] 3.4 Wait in `pass_owned`: `TurnEnd::UsageLimit` and a pending wait at the start of a pass; resume as a forced Turn, abandon on interrupt, keep pending on shutdown.
- [x] 3.5 `Runtime::recover_usage_limit_waits` at startup; `Runtime::resume_at_reset` and `POST /sessions/{id}/usage-limit/resume`; `unattended` on Session creation; `usage_limit` on the Session view; OpenAPI and the SDK.
- [x] 3.6 `ToolHost::usage_limit_settings` from configuration (`cyber-tools`, `cyber-app`).
  - Integration tests (`cyber-server/tests/usage_limit.rs`, injected clock): wait and resume with the same request, interactive refusal, beyond `max_wait`, unknown reset, interrupt during the wait, shutdown and restart resuming the wait, a child's wait abandoned on restart, the Session budget, Goal and loop deadlines, a Goal and a loop iteration kept going through a wait, `resume_at_reset` and its refusals, a second limit after a wait.

## 4. Clients

- [x] 4.1 `cyber exec`: create Sessions `unattended`, the `usage_limit` stream-json line, the text-mode lines, `--timeout` before the resume.
  - Real-binary tests over the ChatGPT HTTP fixture: a waited run in stream-json, the `max_wait` note, the timeout.
- [x] 4.2 TUI: the error hint, `/resume-at-reset` (command, palette, built-in catalog), the toasts, the footer badge.
  - Screen tests: the hint, the badge, the command posting and its refusal.
- [x] 4.3 Web dashboard: the wait notice; rebuild `dist`.

## 5. Docs and checks

- [x] 5.1 Document usage limit waits in `docs/local-core-guide.md`, add a row to `docs/features.md`, add the ROADMAP entry and refresh the requirement inventory; sync the added requirements into `openspec/specs`.
- [x] 5.2 Run `cargo fmt`, workspace clippy with `-D warnings`, `cargo nextest run --workspace --profile ci`, the OpenAPI and SDK checks, `docs/cli.md`, `web` tests and build check, `openspec validate --all --strict` and `scripts/spec_lint.py` (no new warnings).
