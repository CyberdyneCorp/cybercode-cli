## 1. Core

- [x] 1.1 `cyber-core::loops`: `/loop` grammar (interval, durations, flags, `list`, `stop`), `LoopSpec` and errors; `loops.*` config defaults (`max_iterations` 500, `max_per_session` 50, `until_timeout_seconds` 60, `overlap`, `missed`); unit tests for every grammar rule and error.

## 2. Runtime (`cyber-server::runtime::loops`)

- [x] 2.1 Events, registration, fold into `SessionState.loops`, `session_loop` migration and projector.
- [x] 2.2 Create/stop/list on `Runtime`; per-Session limit; stop all.
- [x] 2.3 Sweep: stop conditions, budget, overlap skip/coalesce, `--until` check, iteration admission with `loopi_` IDs atomically with `loop.iteration.started`; background task with injectable `now_ms`.
- [x] 2.4 Iteration end at `drain_ended`: outcome, noop from `loop.wakeup`, cost/tokens/turns deltas, next due (interval tick alignment, self-paced delay, completion without wakeup), coalesced tick, 3-failure stop.
- [x] 2.5 `loop_wakeup` (clamp, last call wins) and the `core/loop` Context Source shown only during an iteration.
- [x] 2.6 `recover_loops` (in-flight → interrupted; skip vs run-once; self-paced once) and `spawn_loop_sweep`, wired in `cyber-app` startup.
- [x] 2.7 Runtime tests (scripted model): five-minute loop, too-short interval, self-paced delay and clamp, no wakeup completes, overlap skip and coalesce, max/for/until stops, budget stop, failures stop, restart with skip and run-once, wakeup survives restart.

## 3. Tools

- [x] 3.1 `ToolHost::check` returning exit code, timeout and tail through the sandboxed user path; test.
- [x] 3.2 `schedule_wakeup` tool offered only during an iteration; tool tests.

## 4. Surfaces

- [x] 4.1 Routes `GET|POST /sessions/{id}/loops`, `DELETE /sessions/{id}/loops[/{loop_id}]`; OpenAPI, `sdk/openapi.json`, regenerated SDK; HTTP tests.
- [x] 4.2 TUI `/loop` dispatch, `/loop list`, iteration and quiet-check lines, footer indicator, `/help` and palette; screen tests.

## 5. Docs and verification

- [x] 5.1 README, local core guide, ROADMAP M2.4 status.
- [x] 5.2 `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, touched-crate tests, `just specs`, `python3 scripts/generate_sdk.py --check`.
- [ ] 5.3 Live: a 1-minute loop and a self-paced loop against a real model in the TUI.
