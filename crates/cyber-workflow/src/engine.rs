//! The per-run QuickJS engine (`workflows` → Sandboxed QuickJS runtime).
//!
//! Each run gets its own `Runtime` and `Context` with a heap limit and an interrupt handler
//! that charges only synchronous JavaScript slices (module evaluation, promise jobs and
//! settling host results) against the script CPU budget; time spent awaiting host calls is
//! not charged. Host calls are queued by the prelude and run as Rust futures through the
//! [`WorkflowHost`] trait, so the engine has no runtime dependency and tests use a fake host.
//!
//! The engine future is `!Send` (QuickJS values are thread-bound): drive it on a local task
//! or a dedicated thread.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use rquickjs::context::EvalOptions;
use rquickjs::promise::PromiseState;
use rquickjs::{Context, Ctx, Function, Module, Object, Persistent, Promise, Runtime};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::diagnostic::{Diagnostic, DiagnosticKind, Position};
use crate::module::WorkflowModule;

const PRELUDE: &str = include_str!("prelude.js");
/// The file name scripts see in stack traces; diagnostics read positions from it.
pub const MODULE_NAME: &str = "workflow.js";
/// Host calls a script may have queued or in flight at once.
const MAX_PENDING_CALLS: usize = 10_000;
/// The run log is truncated with a marker after this many bytes (`Phases, logs and checkpoints`).
pub const MAX_LOG_BYTES: usize = 1024 * 1024;
/// The last log entry of a truncated run log.
pub const LOG_TRUNCATED: &str = "[log truncated at 1 MiB]";
/// Distinct phase titles a run may have, declared ones included (`Phases, logs and
/// checkpoints`); a new title past it fails the run.
pub const MAX_PHASES: usize = 256;
/// The longest `phase()` title, in characters.
pub const MAX_PHASE_TITLE_CHARS: usize = 200;
/// What one queued bridge entry costs besides its text, so many tiny entries are bounded
/// like a few large ones.
const OP_BYTES: usize = std::mem::size_of::<Op>();
const AGENT_OPTIONS: &[&str] = &[
    "agent",
    "model",
    "schema",
    "isolation",
    "label",
    "phase",
    "tools",
    "budget",
    "goal",
];

/// Resource limits of one run (`workflows.max_memory_mb`, `workflows.script_cpu_seconds`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_memory_bytes: usize,
    pub script_cpu: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_memory_bytes: 256 * 1024 * 1024,
            script_cpu: Duration::from_secs(60),
        }
    }
}

/// One `agent()` call as the host receives it.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentCall {
    pub prompt: String,
    /// Validated options object (only documented keys).
    pub options: Map<String, Value>,
    /// The phase the call belongs to: its `phase` option, else the current `phase()`.
    pub phase: Option<String>,
}

pub type HostFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, Diagnostic>> + 'a>>;
/// A host call tagged with the prelude's request id.
type SettledCall<'a> = Pin<Box<dyn Future<Output = (u32, Result<Value, Diagnostic>)> + 'a>>;

/// What a running script can reach. Every method is a durable, observable host call in the
/// server implementation; the engine itself is pure.
pub trait WorkflowHost {
    fn agent(&self, call: AgentCall) -> HostFuture<'_>;
    fn checkpoint(&self, name: String, value: Value) -> HostFuture<'_>;
    fn ask(&self, _question: Value) -> HostFuture<'_> {
        Box::pin(async {
            Err(Diagnostic::new(
                DiagnosticKind::NoInteractiveUser,
                "no interactive user can answer ask() in this run",
            ))
        })
    }
    /// Record the current phase. An error fails the run closed.
    fn phase(&self, title: &str) -> Result<(), Diagnostic>;
    /// Append to the run log. An error fails the run closed.
    fn log(&self, values: Vec<Value>) -> Result<(), Diagnostic>;
    /// A host failure the script must not observe or catch, such as a settled result that
    /// could not be recorded. Checked after every host call settles; `Some` ends the run.
    fn fatal(&self) -> Option<Diagnostic> {
        None
    }
    /// The `ctx.budget` value: `{ spent_tokens, spent_cost_usd, agents_started,
    /// remaining }` on the server. Refreshed before every JavaScript slice.
    fn budget(&self) -> Value {
        Value::Null
    }
    /// A direct read-only tool call (`tools.read`, `tools.glob`, `tools.grep`) the run's
    /// allowlist permits; `input` is the call's object as the script passed it.
    fn tool(&self, _name: String, _input: Value) -> HostFuture<'_> {
        Box::pin(async {
            Err(Diagnostic::new(
                DiagnosticKind::PermissionDenied,
                "direct tools are not available in this host",
            ))
        })
    }
    /// A host call the engine refused before it reached the host: an invalid payload or
    /// an unknown kind. The script observes `error` like any other call result, so a
    /// durable host sequences and records it with the others.
    fn rejected(&self, _kind: &str, _payload: Value, error: Diagnostic) -> HostFuture<'_> {
        Box::pin(async move { Err(error) })
    }
}

/// Inputs of one run.
#[derive(Debug, Clone, Default)]
pub struct RunInput {
    /// The frozen `args` global.
    pub args: Value,
    /// The frozen `ctx` global.
    pub ctx: Value,
    /// Direct tools the invocation allowlists, besides `meta.tools`.
    pub tools: Vec<String>,
}

/// A completed script.
#[derive(Debug, Clone, PartialEq)]
pub struct RunOutput {
    /// The default export's resolved value (`null` for top-level-only scripts).
    pub value: Value,
    /// Declared phases followed by titles first seen in `phase()`.
    pub phases: Vec<String>,
}

/// Run `module` to completion.
pub async fn run(
    module: &WorkflowModule,
    input: RunInput,
    limits: Limits,
    host: &dyn WorkflowHost,
) -> Result<RunOutput, Diagnostic> {
    let mut engine = Engine::new(limits)?;
    engine.phases = module.meta.phases.iter().map(|p| p.title.clone()).collect();
    engine.refresh_budget(host);
    engine.start(module, &input)?;
    engine.drive(host).await
}

enum Op {
    /// A `log()` call's JSON text; `None` is the truncation marker.
    Log(Option<String>),
    Phase(String),
    Call {
        id: u32,
        kind: String,
        payload: String,
    },
}

/// The prelude's native bridge. What it queues lives outside the QuickJS heap limit, so
/// its bounds are enforced as the script calls it, before a synchronous loop can grow it:
/// at most [`MAX_PENDING_CALLS`] calls queued or in flight, their payloads and the queued
/// phase titles within the memory limit, and at most [`MAX_LOG_BYTES`] of log text. Each
/// entry is charged its text plus [`OP_BYTES`]; a phase equal to the one queued last is not
/// queued again.
struct Bridge {
    ops: RefCell<VecDeque<Op>>,
    next_id: Cell<u32>,
    /// Payload bytes of each call queued or in flight.
    calls: RefCell<HashMap<u32, usize>>,
    /// Payload bytes of calls queued or in flight plus queued phase titles.
    bytes: Cell<usize>,
    max_bytes: usize,
    /// Log text accepted so far.
    log_bytes: Cell<usize>,
    /// Why the last request or phase was refused.
    refusal: RefCell<String>,
}

impl Bridge {
    fn new(max_bytes: usize) -> Self {
        Self {
            ops: RefCell::new(VecDeque::new()),
            next_id: Cell::new(0),
            calls: RefCell::new(HashMap::new()),
            bytes: Cell::new(0),
            max_bytes,
            log_bytes: Cell::new(0),
            refusal: RefCell::new(String::new()),
        }
    }

    /// Reserve `len` bytes, or record why not.
    fn reserve(&self, len: usize) -> bool {
        let total = self.bytes.get().saturating_add(len);
        if total > self.max_bytes {
            *self.refusal.borrow_mut() = format!(
                "queued host call data exceeds the memory limit of {} MiB",
                self.max_bytes / (1024 * 1024)
            );
            return false;
        }
        self.bytes.set(total);
        true
    }

    /// Queue a host call; its id, or -1 when refused.
    fn request(&self, kind: String, payload: String) -> i64 {
        if self.calls.borrow().len() >= MAX_PENDING_CALLS {
            *self.refusal.borrow_mut() =
                format!("more than {MAX_PENDING_CALLS} host calls are pending");
            return -1;
        }
        let cost = payload.len().saturating_add(OP_BYTES);
        if !self.reserve(cost) {
            return -1;
        }
        let id = self.next_id.get();
        self.next_id.set(id.wrapping_add(1));
        self.calls.borrow_mut().insert(id, cost);
        self.ops
            .borrow_mut()
            .push_back(Op::Call { id, kind, payload });
        i64::from(id)
    }

    /// A call settled: its payload is no longer held.
    fn settled(&self, id: u32) {
        if let Some(len) = self.calls.borrow_mut().remove(&id) {
            self.bytes.set(self.bytes.get().saturating_sub(len));
        }
    }

    /// Queue a log entry; text past [`MAX_LOG_BYTES`] is dropped after one marker.
    fn log(&self, text: String) {
        let before = self.log_bytes.get();
        if before >= MAX_LOG_BYTES {
            return;
        }
        let after = before.saturating_add(text.len()).saturating_add(OP_BYTES);
        self.log_bytes.set(after);
        let entry = (after < MAX_LOG_BYTES).then_some(text);
        self.ops.borrow_mut().push_back(Op::Log(entry));
    }

    /// Queue a phase change; false when refused.
    fn phase(&self, title: String) -> bool {
        if title.chars().count() > MAX_PHASE_TITLE_CHARS {
            *self.refusal.borrow_mut() =
                format!("phase() titles are at most {MAX_PHASE_TITLE_CHARS} characters");
            return false;
        }
        if matches!(self.ops.borrow().back(), Some(Op::Phase(last)) if *last == title) {
            return true;
        }
        if !self.reserve(title.len().saturating_add(OP_BYTES)) {
            return false;
        }
        self.ops.borrow_mut().push_back(Op::Phase(title));
        true
    }

    fn pop(&self) -> Option<Op> {
        let op = self.ops.borrow_mut().pop_front()?;
        if let Op::Phase(title) = &op {
            let cost = title.len().saturating_add(OP_BYTES);
            self.bytes.set(self.bytes.get().saturating_sub(cost));
        }
        Some(op)
    }
}

/// Script CPU accounting for synchronous JavaScript slices.
struct Clock {
    limit: Duration,
    spent: Cell<Duration>,
    started: Cell<Option<Instant>>,
    expired: Cell<bool>,
}

impl Clock {
    fn begin(&self) {
        self.started.set(Some(Instant::now()));
    }

    fn end(&self) {
        if let Some(start) = self.started.take() {
            self.spent.set(self.spent.get() + start.elapsed());
        }
    }

    /// The interrupt handler: true stops the script.
    fn exceeded(&self) -> bool {
        let running = self.started.get().map_or(Duration::ZERO, |s| s.elapsed());
        if self.spent.get() + running > self.limit {
            self.expired.set(true);
        }
        self.expired.get()
    }
}

/// The prelude's private hooks.
struct Hooks {
    settle: Persistent<Function<'static>>,
    describe: Persistent<Function<'static>>,
}

#[derive(Deserialize)]
struct Described {
    host: bool,
    kind: Option<String>,
    name: Option<String>,
    message: String,
    stack: String,
}

/// Field order matters: JavaScript handles drop before the context and runtime.
struct Engine {
    hooks: Option<Hooks>,
    done: Option<Persistent<Promise<'static>>>,
    context: Context,
    runtime: Runtime,
    clock: Rc<Clock>,
    bridge: Rc<Bridge>,
    /// The JSON text `ctx.budget` reads.
    budget: Rc<RefCell<String>>,
    limits: Limits,
    phases: Vec<String>,
    current_phase: Option<String>,
    /// Direct tools the run allows: `meta.tools` and the invocation's.
    tools: Vec<String>,
}

impl Engine {
    fn new(limits: Limits) -> Result<Self, Diagnostic> {
        let runtime = Runtime::new().map_err(internal)?;
        runtime.set_memory_limit(limits.max_memory_bytes);
        let clock = Rc::new(Clock {
            limit: limits.script_cpu,
            spent: Cell::new(Duration::ZERO),
            started: Cell::new(None),
            expired: Cell::new(false),
        });
        let interrupt = Rc::clone(&clock);
        runtime.set_interrupt_handler(Some(Box::new(move || interrupt.exceeded())));
        let context = Context::full(&runtime).map_err(internal)?;
        Ok(Self {
            hooks: None,
            done: None,
            context,
            runtime,
            clock,
            bridge: Rc::new(Bridge::new(limits.max_memory_bytes)),
            budget: Rc::new(RefCell::new("null".into())),
            limits,
            phases: Vec::new(),
            current_phase: None,
            tools: Vec::new(),
        })
    }

    /// Run `f` as one charged JavaScript slice.
    fn slice<R>(&self, f: impl FnOnce(Ctx<'_>) -> R) -> R {
        self.clock.begin();
        let result = self.context.with(f);
        self.clock.end();
        result
    }

    /// Install the prelude, declare and evaluate the module, and start its default export.
    fn start(&mut self, module: &WorkflowModule, input: &RunInput) -> Result<(), Diagnostic> {
        let bridge = Rc::clone(&self.bridge);
        let budget = Rc::clone(&self.budget);
        let args = json_text(&input.args);
        let run_ctx = json_text(&input.ctx);
        self.tools = module.meta.tools.clone();
        self.tools.extend(input.tools.iter().cloned());
        let started = self.slice(|ctx| {
            let api = install(&ctx, bridge, budget, &args, &run_ctx)
                .map_err(|_| self.failure(&ctx, false))?;
            let describe: Function = api.get("describe").map_err(internal)?;
            let settle: Function = api.get("settle").map_err(internal)?;
            let start: Function = api.get("start").map_err(internal)?;
            let hooks = Hooks {
                settle: Persistent::save(&ctx, settle),
                describe: Persistent::save(&ctx, describe),
            };
            let declared = Module::declare(ctx.clone(), MODULE_NAME, module.source.as_str())
                .map_err(|_| self.failure_with(&ctx, &hooks, true))?;
            let promise = declared
                .eval()
                .and_then(|(evaluated, evaluation)| {
                    let namespace = evaluated.namespace()?;
                    start.call::<_, Promise>((evaluation, namespace))
                })
                .map_err(|_| self.failure_with(&ctx, &hooks, false))?;
            Ok((hooks, Persistent::save(&ctx, promise)))
        });
        let (hooks, done) = started?;
        self.hooks = Some(hooks);
        self.done = Some(done);
        Ok(())
    }

    async fn drive(mut self, host: &dyn WorkflowHost) -> Result<RunOutput, Diagnostic> {
        let mut calls: FuturesUnordered<SettledCall<'_>> = FuturesUnordered::new();
        loop {
            self.refresh_budget(host);
            self.run_jobs()?;
            for (id, future) in self.dispatch(host)? {
                calls.push(Box::pin(async move { (id, future.await) }));
            }
            if let Some(outcome) = self.outcome() {
                return outcome.map(|value| RunOutput {
                    value,
                    phases: std::mem::take(&mut self.phases),
                });
            }
            let Some((id, result)) = calls.next().await else {
                return Err(Diagnostic::execution(
                    "the script is waiting on a promise that can never settle",
                ));
            };
            self.bridge.settled(id);
            if let Some(fatal) = host.fatal() {
                return Err(fatal);
            }
            self.refresh_budget(host);
            self.settle(id, result)?;
        }
    }

    fn refresh_budget(&self, host: &dyn WorkflowHost) {
        *self.budget.borrow_mut() = json_text(&host.budget());
    }

    fn run_jobs(&self) -> Result<(), Diagnostic> {
        self.clock.begin();
        let result = loop {
            match self.runtime.execute_pending_job() {
                Ok(true) => continue,
                Ok(false) => break Ok(()),
                Err(_) => break Err(()),
            }
        };
        // Describing the error runs script code (`toString`, getters), so it is charged too.
        let result = result.map_err(|()| self.context.with(|ctx| self.failure(&ctx, false)));
        self.clock.end();
        result
    }

    /// Deliver queued logs and phases and turn queued calls into host futures.
    fn dispatch<'h>(
        &mut self,
        host: &'h dyn WorkflowHost,
    ) -> Result<Vec<(u32, HostFuture<'h>)>, Diagnostic> {
        let mut started = Vec::new();
        loop {
            match self.bridge.pop() {
                None => return Ok(started),
                Some(Op::Log(Some(text))) => {
                    host.log(serde_json::from_str::<Vec<Value>>(&text).unwrap_or_default())?;
                }
                Some(Op::Log(None)) => host.log(vec![json!(LOG_TRUNCATED)])?,
                Some(Op::Phase(title)) => self.phase(host, title)?,
                Some(Op::Call { id, kind, payload }) => {
                    started.push((id, self.call(host, &kind, &payload)));
                }
            }
        }
    }

    /// A phase change reaches the host; repeating the current phase is not a change.
    fn phase(&mut self, host: &dyn WorkflowHost, title: String) -> Result<(), Diagnostic> {
        if self.current_phase.as_ref() == Some(&title) {
            return Ok(());
        }
        if !self.phases.contains(&title) {
            if self.phases.len() >= MAX_PHASES {
                return Err(too_many_phases());
            }
            self.phases.push(title.clone());
        }
        host.phase(&title)?;
        self.current_phase = Some(title);
        Ok(())
    }

    fn call<'h>(&self, host: &'h dyn WorkflowHost, kind: &str, payload: &str) -> HostFuture<'h> {
        let payload: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
        let request = match kind {
            "agent" => {
                agent_call(&payload, self.current_phase.as_ref()).map(|call| host.agent(call))
            }
            "checkpoint" => {
                checkpoint_call(&payload).map(|(name, value)| host.checkpoint(name, value))
            }
            "ask" => Ok(host.ask(payload["question"].clone())),
            "tool" => tool_call(&payload, &self.tools).map(|(name, input)| host.tool(name, input)),
            other => Err(Diagnostic::execution(format!("unknown host call {other}"))),
        };
        request.unwrap_or_else(|error| host.rejected(kind, payload, error))
    }

    /// The script's result once its main promise settled. Describing a rejection runs
    /// script code, so this is a charged slice.
    fn outcome(&self) -> Option<Result<Value, Diagnostic>> {
        let done = self.done.clone()?;
        self.slice(|ctx| {
            let promise = done.restore(&ctx).ok()?;
            match promise.state() {
                PromiseState::Pending => None,
                PromiseState::Resolved => Some(
                    promise
                        .result::<String>()?
                        .ok()
                        .and_then(|text| serde_json::from_str(&text).ok())
                        .ok_or_else(|| Diagnostic::execution("the return value is not JSON-safe")),
                ),
                PromiseState::Rejected => {
                    let _ = promise.result::<rquickjs::Value>();
                    Some(Err(self.failure(&ctx, false)))
                }
            }
        })
    }

    fn settle(&self, id: u32, result: Result<Value, Diagnostic>) -> Result<(), Diagnostic> {
        let (ok, text) = match result {
            Ok(value) => (true, json_text(&value)),
            Err(error) => (
                false,
                json_text(&json!({"kind": error.kind, "message": error.message})),
            ),
        };
        let settle = self
            .hooks
            .as_ref()
            .map(|h| h.settle.clone())
            .ok_or_else(|| internal("engine not started"))?;
        self.slice(|ctx| {
            let settle = settle.restore(&ctx).map_err(internal)?;
            settle
                .call::<_, ()>((id, ok, text))
                .map_err(|_| self.failure(&ctx, false))
        })
    }

    fn failure(&self, ctx: &Ctx<'_>, parsing: bool) -> Diagnostic {
        match &self.hooks {
            Some(hooks) => self.failure_with(ctx, hooks, parsing),
            None => self.classify(None, parsing),
        }
    }

    /// The diagnostic for the pending exception of `ctx`.
    fn failure_with(&self, ctx: &Ctx<'_>, hooks: &Hooks, parsing: bool) -> Diagnostic {
        let exception = ctx.catch();
        if self.clock.expired.get() {
            return self.classify(None, parsing);
        }
        ctx.run_gc();
        let described = hooks
            .describe
            .clone()
            .restore(ctx)
            .and_then(|describe| describe.call::<_, String>((exception,)))
            .ok()
            .and_then(|text| serde_json::from_str::<Described>(&text).ok());
        self.classify(described, parsing)
    }

    fn classify(&self, described: Option<Described>, parsing: bool) -> Diagnostic {
        if self.clock.expired.get() {
            return Diagnostic::new(
                DiagnosticKind::Timeout,
                format!(
                    "script CPU time exceeded {}s",
                    self.limits.script_cpu.as_secs_f64()
                ),
            );
        }
        let Some(described) = described else {
            return self.memory_or(Diagnostic::execution(
                "the script failed without a describable error",
            ));
        };
        let position = position(&described.stack);
        let kind = described
            .kind
            .as_deref()
            .filter(|_| described.host)
            .and_then(DiagnosticKind::parse);
        let kind = match kind {
            Some(kind) => kind,
            None if parsing && described.name.as_deref() == Some("SyntaxError") => {
                DiagnosticKind::ParseError
            }
            None if described.message.contains("could not load module") => {
                DiagnosticKind::UnsupportedSyntax
            }
            None => DiagnosticKind::ExecutionFailure,
        };
        self.memory_or(Diagnostic::new(kind, described.message).at(position))
    }

    fn memory_or(&self, diagnostic: Diagnostic) -> Diagnostic {
        if !diagnostic.message.contains("out of memory")
            && !diagnostic.message.contains("without a describable error")
        {
            return diagnostic;
        }
        Diagnostic::execution(format!(
            "memory limit of {} MiB exceeded",
            self.limits.max_memory_bytes / (1024 * 1024)
        ))
        .at(diagnostic.position)
    }
}

/// Evaluate the prelude and call it with the native bridge.
fn install<'js>(
    ctx: &Ctx<'js>,
    bridge: Rc<Bridge>,
    budget: Rc<RefCell<String>>,
    args: &str,
    run_ctx: &str,
) -> rquickjs::Result<Object<'js>> {
    let mut options = EvalOptions::default();
    options.global = true;
    options.strict = true;
    options.filename = Some("prelude.js".into());
    let install: Function = ctx.eval_with_options(PRELUDE, options)?;
    let native = Object::new(ctx.clone())?;
    let queue = Rc::clone(&bridge);
    native.set(
        "request",
        Function::new(ctx.clone(), move |kind: String, payload: String| {
            queue.request(kind, payload) as f64
        })?,
    )?;
    native.set(
        "budget",
        Function::new(ctx.clone(), move || budget.borrow().clone())?,
    )?;
    let queue = Rc::clone(&bridge);
    native.set(
        "log",
        Function::new(ctx.clone(), move |text: String| queue.log(text))?,
    )?;
    let queue = Rc::clone(&bridge);
    native.set(
        "phase",
        Function::new(ctx.clone(), move |title: String| queue.phase(title))?,
    )?;
    native.set(
        "refusal",
        Function::new(ctx.clone(), move || bridge.refusal.borrow().clone())?,
    )?;
    install.call((native, args, run_ctx))
}

fn agent_call(payload: &Value, current: Option<&String>) -> Result<AgentCall, Diagnostic> {
    let prompt = payload["prompt"]
        .as_str()
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| Diagnostic::execution("agent() expects a non-empty prompt string"))?;
    let options = payload["options"]
        .as_object()
        .ok_or_else(|| Diagnostic::execution("agent() options must be an object"))?;
    if let Some(unknown) = options
        .keys()
        .find(|k| !AGENT_OPTIONS.contains(&k.as_str()))
    {
        return Err(Diagnostic::execution(format!(
            "agent() has no option {unknown:?}"
        )));
    }
    let phase = match options.get("phase") {
        None => current.cloned(),
        Some(Value::String(phase)) => Some(phase.clone()),
        Some(_) => {
            return Err(Diagnostic::execution(
                "agent() option phase must be a string",
            ));
        }
    };
    Ok(AgentCall {
        prompt: prompt.into(),
        options: options.clone(),
        phase,
    })
}

/// A direct tool call: its name must be in the run's allowlist.
fn tool_call(payload: &Value, allowed: &[String]) -> Result<(String, Value), Diagnostic> {
    let name = payload["tool"]
        .as_str()
        .filter(|name| crate::meta::DIRECT_TOOLS.contains(name))
        .ok_or_else(|| Diagnostic::execution("unknown direct tool"))?;
    if !allowed.iter().any(|tool| tool == name) {
        return Err(Diagnostic::new(
            DiagnosticKind::PermissionDenied,
            format!("tool not allowed by meta.tools: {name}"),
        ));
    }
    let input = payload["input"]
        .as_object()
        .ok_or_else(|| Diagnostic::execution(format!("tools.{name}() expects an object")))?;
    Ok((name.into(), Value::Object(input.clone())))
}

fn checkpoint_call(payload: &Value) -> Result<(String, Value), Diagnostic> {
    let name = payload["name"]
        .as_str()
        .filter(|n| !n.trim().is_empty())
        .ok_or_else(|| Diagnostic::execution("checkpoint() expects a non-empty name"))?;
    Ok((name.into(), payload["value"].clone()))
}

/// A run past [`MAX_PHASES`] distinct titles fails closed.
pub fn too_many_phases() -> Diagnostic {
    Diagnostic::execution(format!(
        "the run has more than {MAX_PHASES} distinct phase titles"
    ))
}

fn json_text(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".into())
}

fn internal(error: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::execution(format!("workflow engine error: {error}"))
}

/// The first `workflow.js:<line>:<column>` frame of a stack trace.
fn position(stack: &str) -> Option<Position> {
    let marker = format!("{MODULE_NAME}:");
    stack.match_indices(&marker).find_map(|(at, _)| {
        let rest = &stack[at + marker.len()..];
        let mut parts = rest.splitn(3, |c: char| !c.is_ascii_digit());
        let line = parts.next()?.parse().ok()?;
        let column = parts.next()?.parse().ok()?;
        Some(Position { line, column })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bridge_bounds_queued_logs_calls_and_bytes() {
        let bridge = Bridge::new(8 * 1024 * 1024);
        let chunk = "x".repeat(1024 * 1024);
        for _ in 0..1000 {
            bridge.log(chunk.clone());
        }
        // One accepted entry and the marker; the rest was never queued.
        assert_eq!(bridge.ops.borrow().len(), 1);
        assert!(matches!(bridge.pop(), Some(Op::Log(None))));
        let accepted: Vec<i64> = (0..100)
            .map(|_| bridge.request("agent".into(), chunk.clone()))
            .collect();
        // Each call is charged its payload plus the entry itself.
        assert_eq!(accepted.iter().filter(|id| **id >= 0).count(), 7);
        assert!(bridge.refusal.borrow().contains("memory limit of 8 MiB"));
        assert!(!bridge.phase(chunk.clone()));
        // Settling a call frees its payload.
        bridge.settled(0);
        assert!(bridge.phase("next".into()));
        assert_eq!(bridge.ops.borrow().len(), 8);

        let small = Bridge::new(usize::MAX);
        for _ in 0..MAX_PENDING_CALLS {
            assert!(small.request("agent".into(), "{}".into()) >= 0);
        }
        assert_eq!(small.request("agent".into(), "{}".into()), -1);
        assert!(small.refusal.borrow().contains("host calls are pending"));
    }

    #[test]
    fn tiny_phases_are_charged_per_entry_and_repeats_are_not_queued() {
        let bridge = Bridge::new(1024 * 1024);
        // A synchronous `for (;;) phase('a')` queues one entry.
        for _ in 0..100_000 {
            assert!(bridge.phase("a".into()));
        }
        assert_eq!(bridge.ops.borrow().len(), 1);
        // Alternating titles are bounded by the memory limit, entry overhead included.
        let accepted = (0..1_000_000)
            .take_while(|n| bridge.phase(if n % 2 == 0 { "b" } else { "a" }.into()))
            .count();
        assert!(accepted < 1024 * 1024 / OP_BYTES, "{accepted}");
        assert!(bridge.refusal.borrow().contains("memory limit"));
        assert!(!Bridge::new(usize::MAX).phase("t".repeat(MAX_PHASE_TITLE_CHARS + 1)));
        assert!(Bridge::new(usize::MAX).phase("é".repeat(MAX_PHASE_TITLE_CHARS)));
    }
}
