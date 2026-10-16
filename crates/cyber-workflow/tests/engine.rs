//! Engine behavior through the public API with a fake [`WorkflowHost`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use cyber_workflow::{
    AgentCall, Diagnostic, DiagnosticKind, HostFuture, Limits, MAX_PHASE_TITLE_CHARS, MAX_PHASES,
    RunInput, RunOutput, WorkflowHost, WorkflowModule, run,
};
use futures::channel::oneshot;
use futures::executor::block_on;
use serde_json::{Value, json};

#[derive(Default)]
struct FakeHost {
    calls: RefCell<Vec<AgentCall>>,
    logs: RefCell<Vec<Value>>,
    phases: RefCell<Vec<String>>,
    checkpoints: RefCell<Vec<(String, Value)>>,
    /// Prompts held until another prompt is requested: `release_on[trigger] = held`.
    gates: RefCell<HashMap<String, oneshot::Sender<()>>>,
    held: RefCell<HashMap<String, oneshot::Receiver<()>>>,
    /// Blocking time spent inside a host call; never charged to the script.
    host_delay: Duration,
}

impl FakeHost {
    fn gate(&self, held: &str, trigger: &str) {
        let (tx, rx) = oneshot::channel();
        self.gates.borrow_mut().insert(trigger.into(), tx);
        self.held.borrow_mut().insert(held.into(), rx);
    }

    fn prompts(&self) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .map(|c| c.prompt.clone())
            .collect()
    }
}

impl WorkflowHost for FakeHost {
    fn agent(&self, call: AgentCall) -> HostFuture<'_> {
        let prompt = call.prompt.clone();
        self.calls.borrow_mut().push(call);
        if let Some(gate) = self.gates.borrow_mut().remove(&prompt) {
            let _ = gate.send(());
        }
        let held = self.held.borrow_mut().remove(&prompt);
        Box::pin(async move {
            if let Some(held) = held {
                let _ = held.await;
            }
            if self.host_delay > Duration::ZERO {
                std::thread::sleep(self.host_delay);
            }
            if prompt.starts_with("fail") {
                return Err(Diagnostic::new(
                    DiagnosticKind::AgentFailed,
                    format!("{prompt}: provider error"),
                ));
            }
            Ok(json!({ "text": format!("done: {prompt}") }))
        })
    }

    fn checkpoint(&self, name: String, value: Value) -> HostFuture<'_> {
        self.checkpoints.borrow_mut().push((name, value.clone()));
        Box::pin(async move { Ok(value) })
    }

    fn phase(&self, title: &str) -> Result<(), Diagnostic> {
        self.phases.borrow_mut().push(title.into());
        Ok(())
    }

    fn log(&self, values: Vec<Value>) -> Result<(), Diagnostic> {
        self.logs.borrow_mut().extend(values);
        Ok(())
    }

    fn budget(&self) -> Value {
        json!({ "agents_started": self.calls.borrow().len() })
    }
}

const META: &str =
    "export const meta = { name: 'test', description: 'Test', phases: [{ title: 'Scan' }] };\n";

fn execute_with(
    body: &str,
    host: &dyn WorkflowHost,
    limits: Limits,
) -> Result<RunOutput, Diagnostic> {
    let module = WorkflowModule::inline(format!("{META}{body}")).expect("module parses");
    let input = RunInput {
        args: json!({ "target": "src" }),
        ctx: json!({ "run_id": "run_test" }),
    };
    block_on(run(&module, input, limits, host))
}

fn execute(body: &str) -> Result<RunOutput, Diagnostic> {
    execute_with(body, &FakeHost::default(), Limits::default())
}

fn failure(body: &str) -> Diagnostic {
    execute(body).expect_err("the script fails")
}

#[test]
fn agents_phases_logs_and_checkpoints_reach_the_host() {
    let host = FakeHost::default();
    let output = execute_with(
        "export default async function () {\n  phase('Scan');\n  const a = await agent('list ' + args.target, { label: 'scan' });\n  phase('Verify');\n  log('found', a.text, undefined, { n: 1 });\n  const saved = await checkpoint('cursor', { at: 3 });\n  return { a: a.text, saved, run: ctx.run_id };\n}\n",
        &host,
        Limits::default(),
    )
    .unwrap();
    assert_eq!(
        output.value,
        json!({ "a": "done: list src", "saved": { "at": 3 }, "run": "run_test" })
    );
    assert_eq!(output.phases, ["Scan", "Verify"]);
    assert_eq!(*host.phases.borrow(), ["Scan", "Verify"]);
    assert_eq!(
        *host.logs.borrow(),
        [
            json!("found"),
            json!("done: list src"),
            Value::Null,
            json!({ "n": 1 })
        ]
    );
    assert_eq!(
        *host.checkpoints.borrow(),
        [("cursor".to_string(), json!({ "at": 3 }))]
    );
    let call = &host.calls.borrow()[0];
    assert_eq!(call.phase.as_deref(), Some("Scan"));
    assert_eq!(call.options["label"], "scan");
}

#[test]
fn top_level_code_runs_with_top_level_await() {
    let host = FakeHost::default();
    let output = execute_with(
        "const r = await agent('one');\nlog(r.text);\n",
        &host,
        Limits::default(),
    )
    .unwrap();
    assert_eq!(output.value, Value::Null);
    assert_eq!(*host.logs.borrow(), [json!("done: one")]);
}

#[test]
fn args_and_host_globals_are_frozen() {
    let output = execute(
        "export default () => {\n  try { args.target = 'x'; } catch (e) { log(e.name); }\n  try { agent = null; } catch (e) { log(e.name); }\n  return [args.target, typeof agent, typeof parallel, typeof pipeline, typeof ask, typeof tools];\n}\n",
    )
    .unwrap();
    assert_eq!(
        output.value,
        json!([
            "src",
            "function",
            "function",
            "function",
            "function",
            "undefined"
        ])
    );
}

#[test]
fn host_capabilities_and_code_generation_are_undefined() {
    let output = execute(
        "export default () => [typeof eval, typeof Function, typeof setTimeout, typeof WebAssembly, typeof fetch, typeof require, typeof process, typeof WeakRef, typeof FinalizationRegistry, typeof SharedArrayBuffer, typeof Atomics, (function () {}).constructor, (async () => {}).constructor];\n",
    )
    .unwrap();
    let mut expected = vec![json!("undefined"); 11];
    expected.extend([Value::Null, Value::Null]);
    assert_eq!(output.value, Value::Array(expected));
    let fetch =
        failure("export default async () => {\n  await fetch('https://example.com');\n};\n");
    assert_eq!(fetch.kind, DiagnosticKind::ExecutionFailure);
    assert_eq!(fetch.message, "fetch is not defined");
    assert_eq!(fetch.position.unwrap().line, 3);
    assert!(
        fetch
            .to_string()
            .starts_with("ExecutionFailure: fetch is not defined (")
    );
    assert_eq!(
        failure("new Function('return 1')();").message,
        "Function is not defined"
    );
    assert_eq!(failure("eval('1');").message, "eval is not defined");
    assert_eq!(
        failure("setTimeout(() => {}, 1);").message,
        "setTimeout is not defined"
    );
    assert_eq!(
        failure("WebAssembly.compile(new Uint8Array(0));").message,
        "WebAssembly is not defined"
    );
    let back_door = failure("const F = (function () {}).constructor; F('return 1');");
    assert_eq!(back_door.kind, DiagnosticKind::ExecutionFailure);
}

#[test]
fn non_deterministic_apis_throw_and_explicit_dates_work() {
    for call in [
        "Date.now()",
        "Math.random()",
        "performance.now()",
        "new Date()",
        "Date()",
    ] {
        let error = failure(&format!("export default () => {{\n  return {call};\n}}\n"));
        assert_eq!(error.kind, DiagnosticKind::ExecutionFailure, "{call}");
        assert_eq!(
            error.message,
            "non-deterministic API; pass values through args"
        );
        // QuickJS reports the column of the called member or constructor.
        assert_eq!(error.position.unwrap().line, 3, "{call}");
    }
    let output = execute(
        "export default () => [new Date(0).toISOString(), new Date('2026-01-02T00:00:00Z').getUTCDate(), Date.UTC(2020, 0, 1), new Date(0) instanceof Date];\n",
    )
    .unwrap();
    assert_eq!(
        output.value,
        json!(["1970-01-01T00:00:00.000Z", 2, 1577836800000_i64, true])
    );
    let rebound = failure("Math.random = () => 4;\nexport default () => Math.random();\n");
    assert_eq!(rebound.kind, DiagnosticKind::ExecutionFailure);
}

#[test]
fn busy_loops_time_out_but_host_waits_are_not_charged() {
    let limits = Limits {
        script_cpu: Duration::from_millis(300),
        ..Limits::default()
    };
    let timeout = execute_with(
        "export default () => { while (true) {} };\n",
        &FakeHost::default(),
        limits,
    )
    .unwrap_err();
    assert_eq!(timeout.kind, DiagnosticKind::Timeout);
    let async_loop = execute_with(
        "export default async () => { await agent('first'); for (;;) {} };\n",
        &FakeHost::default(),
        limits,
    )
    .unwrap_err();
    assert_eq!(async_loop.kind, DiagnosticKind::Timeout);
    let slow_host = FakeHost {
        host_delay: Duration::from_millis(200),
        ..FakeHost::default()
    };
    let output = execute_with(
        "export default async () => { for (const p of ['a', 'b', 'c']) await agent(p); return 'ok'; };\n",
        &slow_host,
        limits,
    )
    .unwrap();
    assert_eq!(output.value, "ok");
}

#[test]
fn describing_a_hostile_error_is_charged_to_the_script() {
    let limits = Limits {
        script_cpu: Duration::from_millis(300),
        ..Limits::default()
    };
    for body in [
        // The rejected main promise is described after the last slice.
        "export default async () => { throw { toString() { for (;;) {} } }; };\n",
        // A rejected top-level evaluation.
        "throw { toString() { for (;;) {} } };\n",
        // A rejection observed after a host call settled.
        "export default async () => { await agent('one'); throw { toString() { for (;;) {} } }; };\n",
    ] {
        let started = std::time::Instant::now();
        let error = execute_with(body, &FakeHost::default(), limits).unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::Timeout, "{body}: {error}");
        assert!(started.elapsed() < Duration::from_secs(10), "{body}");
    }
}

#[test]
fn synchronous_call_loops_are_bounded_by_the_bridge() {
    let limits = Limits {
        max_memory_bytes: 64 * 1024 * 1024,
        script_cpu: Duration::from_secs(20),
    };
    // Each call queues a 1 MiB payload outside the QuickJS heap: refused at the limit.
    let error = execute_with(
        "const s = 'x'.repeat(1 << 20);\nexport default async () => { for (;;) agent(s); };\n",
        &FakeHost::default(),
        limits,
    )
    .unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::ExecutionFailure, "{error}");
    assert!(
        error.message.contains("exceeds the memory limit of 64 MiB"),
        "{error}"
    );
    // A refusal is catchable; pending calls are bounded too.
    let host = FakeHost::default();
    let output = execute_with(
        "export default async () => { let n = 0; try { for (;;) { agent('a'); n++; } } catch (e) { return [n, e.kind, e.message]; } };\n",
        &host,
        limits,
    )
    .unwrap();
    assert_eq!(
        output.value,
        json!([
            10_000,
            "ExecutionFailure",
            "more than 10000 host calls are pending"
        ])
    );
    // Logs past 1 MiB are dropped as the script makes them, not when it yields.
    let host = FakeHost::default();
    let output = execute_with(
        "const s = 'x'.repeat(1 << 20);\nexport default async () => { for (let i = 0; i < 100; i++) log(s); return 'logged'; };\n",
        &host,
        limits,
    )
    .unwrap();
    assert_eq!(output.value, "logged");
    assert_eq!(*host.logs.borrow(), [json!("[log truncated at 1 MiB]")]);
}

#[test]
fn phase_loops_are_bounded_and_repeats_do_not_reach_the_host() {
    let limits = Limits {
        max_memory_bytes: 16 * 1024 * 1024,
        script_cpu: Duration::from_secs(20),
    };
    // Repeating the current phase is not a change: one record, and the CPU limit ends it.
    let host = FakeHost::default();
    let output = execute_with(
        "export default async () => { for (let i = 0; i < 200000; i++) phase('a'); await checkpoint('x', 1); phase('a'); return 'ok'; };\n",
        &host,
        limits,
    )
    .unwrap();
    assert_eq!(output.value, "ok");
    assert_eq!(*host.phases.borrow(), ["a"]);
    // Alternating titles are charged per entry and refused within the memory limit.
    let error = execute_with(
        "export default async () => { for (let i = 0; ; i++) phase(i % 2 ? 'a' : 'b'); };\n",
        &FakeHost::default(),
        limits,
    )
    .unwrap_err();
    assert!(
        error.message.contains("exceeds the memory limit of 16 MiB"),
        "{error}"
    );
    // New titles across slices stop at the distinct-title cap, failing the run closed.
    let host = FakeHost::default();
    let error = execute_with(
        "export default async () => { for (let i = 0; ; i++) { try { phase('p' + i); } catch (e) { return e.message; } await checkpoint('c', i); } };\n",
        &host,
        limits,
    )
    .unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::ExecutionFailure);
    assert!(
        error
            .message
            .contains("more than 256 distinct phase titles"),
        "{error}"
    );
    assert_eq!(host.phases.borrow().len(), MAX_PHASES - 1);
    // Overlong titles are refused like an empty one.
    let output = execute(&format!(
        "export default () => {{ try {{ phase('{}'); }} catch (e) {{ return [e.kind, e.message]; }} }};\n",
        "t".repeat(MAX_PHASE_TITLE_CHARS + 1)
    ))
    .unwrap();
    assert_eq!(
        output.value,
        json!([
            "ExecutionFailure",
            "phase() titles are at most 200 characters"
        ])
    );
}

#[test]
fn deeply_nested_scripts_fail_without_crashing() {
    let deep = |levels: usize| format!("{}{}", "[".repeat(levels), "]".repeat(levels));
    let meta = WorkflowModule::inline(format!(
        "export const meta = {{ name: 'a', description: 'd', x: {} }};",
        deep(400_000)
    ))
    .unwrap_err();
    assert_eq!(meta.kind, DiagnosticKind::ParseError);
    // Nesting outside `meta` reaches QuickJS, whose parser has its own stack limit.
    let error = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || failure(&format!("export default () => {};\n", deep(400_000))))
        .unwrap()
        .join()
        .unwrap();
    assert_ne!(error.kind, DiagnosticKind::Timeout, "{error}");
}

#[test]
fn large_allocations_hit_the_memory_limit() {
    let limits = Limits {
        max_memory_bytes: 16 * 1024 * 1024,
        ..Limits::default()
    };
    let error = execute_with(
        "export default () => { const parts = []; for (let i = 0; ; i++) parts.push('x'.repeat(1 << 20) + i); };\n",
        &FakeHost::default(),
        limits,
    )
    .unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::ExecutionFailure);
    assert_eq!(error.message, "memory limit of 16 MiB exceeded");
}

#[test]
fn diagnostics_carry_line_and_column() {
    let syntax = failure("export default async function () {\n  if (true) {\n    log(1);\n\n");
    assert_eq!(syntax.kind, DiagnosticKind::ParseError);
    assert!(syntax.position.is_some(), "{syntax:?}");
    let thrown = failure("export default () => {\n  throw new Error('boom');\n};\n");
    assert_eq!(thrown.to_string(), "ExecutionFailure: boom (3:13)");
    let value = failure("throw 'plain';");
    assert_eq!(value.message, "plain");
}

#[test]
fn agent_failures_reject_with_a_catchable_kind_that_scripts_cannot_forge() {
    let caught = execute(
        "export default async () => {\n  try { await agent('fail one'); } catch (e) { return [e.kind, e.message]; }\n};\n",
    )
    .unwrap();
    assert_eq!(
        caught.value,
        json!(["AgentFailed", "fail one: provider error"])
    );
    let uncaught = failure("export default async () => {\n  await agent('fail two');\n};\n");
    assert_eq!(uncaught.kind, DiagnosticKind::AgentFailed);
    let forged = failure("const e = new Error('nope'); e.kind = 'BudgetExceeded'; throw e;");
    assert_eq!(forged.kind, DiagnosticKind::ExecutionFailure);
    let ask = failure("export default async () => ask('Proceed?');\n");
    assert_eq!(ask.kind, DiagnosticKind::NoInteractiveUser);
}

#[test]
fn invalid_host_calls_fail_closed() {
    let unknown = failure("export default async () => agent('x', { sandbox: 'off' });\n");
    assert_eq!(unknown.message, "agent() has no option \"sandbox\"");
    let empty = failure("export default async () => agent('');\n");
    assert_eq!(empty.kind, DiagnosticKind::ExecutionFailure);
    let stuck = failure("export default () => new Promise(() => {});\n");
    assert!(stuck.message.contains("never settle"), "{stuck}");
    let cyclic = failure("export default () => { const a = {}; a.a = a; return a; };\n");
    assert_eq!(cyclic.kind, DiagnosticKind::ExecutionFailure);
}

#[test]
fn parallel_keeps_order_limits_concurrency_and_settles() {
    let host = FakeHost::default();
    let output = execute_with(
        "export default async () => {\n  const items = ['a', 'b', 'c', 'd'];\n  const all = await parallel(items.map((x) => () => agent(x)), { concurrency: 2 });\n  const settled = await parallel([() => agent('ok'), () => agent('fail x'), async () => { throw new Error('y'); }], { settle: true });\n  let rejected = null;\n  try { await parallel([() => agent('fail z')]); } catch (e) { rejected = e.kind; }\n  return { all: all.map((r) => r.text), settled: settled.map((s) => s.status), rejected };\n};\n",
        &host,
        Limits::default(),
    )
    .unwrap();
    assert_eq!(
        output.value,
        json!({
            "all": ["done: a", "done: b", "done: c", "done: d"],
            "settled": ["fulfilled", "rejected", "rejected"],
            "rejected": "AgentFailed"
        })
    );
    assert_eq!(&host.prompts()[..4], ["a", "b", "c", "d"]);
    let bad = failure("export default () => parallel([1]);\n");
    assert_eq!(bad.message, "parallel() expects an array of functions");
}

#[test]
fn pipeline_starts_the_next_stage_without_waiting_for_slow_items() {
    let host = FakeHost::default();
    // Reviewing item 1 only finishes once verifying item 0 has started.
    host.gate("review 1", "verify 0");
    let output = execute_with(
        "export default async () => pipeline([0, 1],\n  (i) => agent('review ' + i).then(() => i),\n  (i) => agent('verify ' + i).then((r) => r.text));\n",
        &host,
        Limits::default(),
    )
    .unwrap();
    assert_eq!(output.value, json!(["done: verify 0", "done: verify 1"]));
    assert_eq!(
        host.prompts(),
        ["review 0", "review 1", "verify 0", "verify 1"]
    );
}

#[test]
fn logs_are_truncated_with_a_marker() {
    let host = FakeHost::default();
    execute_with(
        "export default () => { const chunk = 'x'.repeat(64 * 1024); for (let i = 0; i < 40; i++) log(chunk); };\n",
        &host,
        Limits::default(),
    )
    .unwrap();
    let logs = host.logs.borrow();
    assert_eq!(logs.last().unwrap(), &json!("[log truncated at 1 MiB]"));
    assert!(logs.len() < 40);
}

/// Counts agent calls in flight; each settles only after the script yields twice.
#[derive(Default)]
struct CountingHost {
    in_flight: std::cell::Cell<usize>,
    peak: std::cell::Cell<usize>,
    /// The recorded result fails after this many settled calls.
    fail_recording_after: Option<usize>,
    settled: std::cell::Cell<usize>,
    fatal: RefCell<Option<Diagnostic>>,
}

impl WorkflowHost for CountingHost {
    fn agent(&self, call: AgentCall) -> HostFuture<'_> {
        self.in_flight.set(self.in_flight.get() + 1);
        self.peak.set(self.peak.get().max(self.in_flight.get()));
        Box::pin(async move {
            // Yield so that every call the script can start is started first.
            for _ in 0..4 {
                yield_now().await;
            }
            self.in_flight.set(self.in_flight.get() - 1);
            self.settled.set(self.settled.get() + 1);
            if self.fail_recording_after == Some(self.settled.get()) {
                *self.fatal.borrow_mut() = Some(Diagnostic::execution("store is unavailable"));
            }
            Ok(json!({ "text": call.prompt }))
        })
    }

    fn checkpoint(&self, _name: String, value: Value) -> HostFuture<'_> {
        Box::pin(async move { Ok(value) })
    }

    fn phase(&self, _title: &str) -> Result<(), Diagnostic> {
        Ok(())
    }

    fn log(&self, _values: Vec<Value>) -> Result<(), Diagnostic> {
        Ok(())
    }

    fn fatal(&self) -> Option<Diagnostic> {
        self.fatal.borrow().clone()
    }
}

fn run_counting(body: &str, host: &CountingHost) -> Result<RunOutput, Diagnostic> {
    execute_with(body, host, Limits::default())
}

/// Return `Pending` once after asking to be polled again.
async fn yield_now() {
    let mut yielded = false;
    futures::future::poll_fn(|cx| {
        if yielded {
            return std::task::Poll::Ready(());
        }
        yielded = true;
        cx.waker().wake_by_ref();
        std::task::Poll::Pending
    })
    .await;
}

#[test]
fn parallel_group_concurrency_bounds_calls_in_flight() {
    let host = CountingHost::default();
    let output = run_counting(
        "export default async () => {\n  const items = [1, 2, 3, 4, 5, 6, 7];\n  const limited = await parallel(items.map((i) => () => agent('a' + i)), { concurrency: 2 });\n  return limited.map((r) => r.text);\n};\n",
        &host,
    )
    .unwrap();
    assert_eq!(
        output.value,
        json!(["a1", "a2", "a3", "a4", "a5", "a6", "a7"])
    );
    assert_eq!(host.peak.get(), 2);

    let unlimited = CountingHost::default();
    run_counting(
        "export default () => parallel([1, 2, 3, 4].map((i) => () => agent('b' + i)));\n",
        &unlimited,
    )
    .unwrap();
    assert_eq!(unlimited.peak.get(), 4);
}

#[test]
fn a_fatal_host_failure_ends_the_run_before_the_script_sees_the_result() {
    let host = CountingHost {
        fail_recording_after: Some(1),
        ..CountingHost::default()
    };
    let error = run_counting(
        "export default async () => {\n  try { await agent('one'); } catch (e) { return 'caught'; }\n  log('observed');\n  return 'observed';\n};\n",
        &host,
    )
    .expect_err("the run fails closed");
    assert_eq!(error.kind, DiagnosticKind::ExecutionFailure);
    assert_eq!(error.message, "store is unavailable");
}

#[test]
fn a_failed_phase_or_log_record_fails_the_run() {
    struct Refusing;
    impl WorkflowHost for Refusing {
        fn agent(&self, _call: AgentCall) -> HostFuture<'_> {
            Box::pin(async { Ok(Value::Null) })
        }
        fn checkpoint(&self, _name: String, value: Value) -> HostFuture<'_> {
            Box::pin(async move { Ok(value) })
        }
        fn phase(&self, title: &str) -> Result<(), Diagnostic> {
            Err(Diagnostic::execution(format!(
                "cannot record phase {title}"
            )))
        }
        fn log(&self, _values: Vec<Value>) -> Result<(), Diagnostic> {
            Err(Diagnostic::execution("cannot record log"))
        }
    }
    let phase = execute_with(
        "export default async () => { phase('Scan'); return 1; };\n",
        &Refusing,
        Limits::default(),
    )
    .unwrap_err();
    assert_eq!(phase.message, "cannot record phase Scan");
    let log = execute_with(
        "export default async () => { log('x'); return 1; };\n",
        &Refusing,
        Limits::default(),
    )
    .unwrap_err();
    assert_eq!(log.message, "cannot record log");
}

#[test]
fn ctx_budget_is_a_live_frozen_host_snapshot() {
    let output = execute(
        "export default async function () {\n  const before = ctx.budget.agents_started;\n  await agent('one');\n  await agent('two');\n  const after = ctx.budget;\n  'use strict';\n  let frozen = true;\n  try { after.agents_started = 99; frozen = after.agents_started !== 99; } catch (_) {}\n  return { before, after: after.agents_started, frozen, keys: Object.keys(ctx) };\n}\n",
    )
    .unwrap();
    assert_eq!(
        output.value,
        json!({ "before": 0, "after": 2, "frozen": true, "keys": ["run_id", "budget"] })
    );
}
