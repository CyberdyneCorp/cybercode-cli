//! The event-to-signal state machine (`observability-costs` → OpenTelemetry export): it
//! observes the runtime's live events and starts, annotates and ends spans and records
//! metrics through the SDK handles. It never awaits; every call is in memory.
//!
//! Spans (design → Event-to-span mapping):
//! - `cyber.workflow.run`: the root of its own trace, linked to the owner's open Turn;
//! - `cyber.workflow.agent`: under its run;
//! - `cyber.session`: one activity period (a Drain) of a Session, under the workflow agent
//!   that created it, the parent's open Turn (a subagent), or nothing (a root Session);
//! - `cyber.turn`: from a promotion to the next Turn's promotion or `Idle`;
//! - `cyber.tool` and `cyber.hook`: under the open Turn (a hook under its tool call). A
//!   hook admitted outside an activity period opens the session span, which a following
//!   Turn joins and which otherwise ends with the hook.
//!
//! Span times are the times the events are observed, except hook spans, which use their
//! receipt's. Work settling after a restart records metrics and no span: unfinished
//! spans are never resurrected.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cyber_core::hooks::{HookAction, HookOutcome};
use cyber_server::runtime::{HookReceipt, LiveEvent, SessionInfo};
use opentelemetry::metrics::{Counter, Meter};
use opentelemetry::trace::{Link, Span as _, Status, TraceContextExt, Tracer as _};
use opentelemetry::{Context, KeyValue, StringValue, Value as OtelValue};
use opentelemetry_sdk::trace::SdkTracer;
use serde_json::Value;

use crate::privacy::Content;

/// Sessions whose kind and parents are remembered (design: bounded LRU).
pub const SESSION_CACHE: usize = 4096;
/// Admitted prompt texts kept for `cyber.prompt` before they are promoted.
const PROMPT_CACHE: usize = 4096;

/// What kind of Session a span belongs to (`cyber.session.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Root,
    Subagent,
    WorkflowAgent,
}

impl SessionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Subagent => "subagent",
            Self::WorkflowAgent => "workflow_agent",
        }
    }
}

/// What the mapper remembers of a Session.
#[derive(Debug, Clone)]
struct SessionMeta {
    kind: SessionKind,
    parent_id: Option<String>,
    subagent_name: Option<String>,
    agent: String,
    model: String,
    mode: String,
    title: String,
    directory: String,
}

impl SessionMeta {
    fn of(info: &SessionInfo) -> Self {
        let kind = if info.workflow_run_id.is_some() {
            SessionKind::WorkflowAgent
        } else if info.parent_id.is_some() {
            SessionKind::Subagent
        } else {
            SessionKind::Root
        };
        Self {
            kind,
            parent_id: info.parent_id.clone(),
            subagent_name: info.subagent_name.clone(),
            agent: info.agent.clone(),
            model: info.model.clone(),
            mode: info.mode.clone(),
            title: info.title.clone(),
            directory: info.directory.clone(),
        }
    }

    /// A Session the lookup could not read: a root Session with nothing known.
    fn unknown() -> Self {
        Self {
            kind: SessionKind::Root,
            parent_id: None,
            subagent_name: None,
            agent: String::new(),
            model: String::new(),
            mode: String::new(),
            title: String::new(),
            directory: String::new(),
        }
    }
}

/// A bounded map that evicts the least recently used entry.
struct Lru<V> {
    entries: HashMap<String, (V, u64)>,
    tick: u64,
    capacity: usize,
}

impl<V> Lru<V> {
    fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            tick: 0,
            capacity,
        }
    }

    fn get(&mut self, key: &str) -> Option<&V> {
        self.tick += 1;
        let tick = self.tick;
        self.entries.get_mut(key).map(|(value, used)| {
            *used = tick;
            &*value
        })
    }

    fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    fn insert(&mut self, key: String, value: V) {
        if self.entries.len() >= self.capacity && !self.entries.contains_key(&key) {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.entries.remove(&oldest);
            }
        }
        self.tick += 1;
        self.entries.insert(key, (value, self.tick));
    }
}

/// Token counts by class.
#[derive(Debug, Clone, Copy, Default)]
struct Tokens {
    input: u64,
    output: u64,
    reasoning: u64,
    cache_read: u64,
    cache_write: u64,
}

impl Tokens {
    fn of(usage: &Value) -> Self {
        let count = |key: &str| usage[key].as_u64().unwrap_or(0);
        Self {
            input: count("input"),
            output: count("output"),
            reasoning: count("reasoning"),
            cache_read: count("cache_read"),
            cache_write: count("cache_write"),
        }
    }

    fn add(&mut self, other: Self) {
        self.input += other.input;
        self.output += other.output;
        self.reasoning += other.reasoning;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }

    fn classes(self) -> [(&'static str, u64); 5] {
        [
            ("input", self.input),
            ("output", self.output),
            ("reasoning", self.reasoning),
            ("cache_read", self.cache_read),
            ("cache_write", self.cache_write),
        ]
    }
}

/// An open Turn span and what its steps add up to.
struct Turn {
    cx: Context,
    started: Instant,
    stepped: bool,
    /// How its last step failed: `Some(true)` when it was interrupted.
    failed: Option<bool>,
    /// `cyber.agent`, `cyber.mode` and `cyber.model`, from its latest step.
    agent: String,
    mode: String,
    model: String,
    tokens: Tokens,
    cost: Option<f64>,
    finish: Option<String>,
    prompts: Vec<String>,
    responses: Vec<String>,
}

/// The open spans of one Session's activity period.
#[derive(Default)]
struct Activity {
    session: Option<Context>,
    turn: Option<Turn>,
    tools: HashMap<String, Context>,
    /// Dispatch attempts of the open tool calls, set on their spans when they settle.
    attempts: HashMap<String, i64>,
    /// The provider and model of the step in progress.
    step: Option<(String, String)>,
}

/// An open workflow agent span.
struct AgentSpan {
    cx: Context,
    run_id: String,
}

/// The metric instruments (design → Metrics). Attribute values are bounded sets: no
/// Session, call or run ids.
struct Metrics {
    sessions: Counter<u64>,
    turns: Counter<u64>,
    tokens: Counter<u64>,
    cost: Counter<f64>,
    tool_calls: Counter<u64>,
    tool_errors: Counter<u64>,
    permissions: Counter<u64>,
    active_time: Counter<f64>,
}

impl Metrics {
    fn new(meter: &Meter) -> Self {
        let count = |name: &'static str, unit: &'static str| {
            meter.u64_counter(name).with_unit(unit).build()
        };
        Self {
            sessions: count("cyber.session.count", "{session}"),
            turns: count("cyber.turn.count", "{turn}"),
            tokens: count("cyber.token.usage", "{token}"),
            cost: meter
                .f64_counter("cyber.cost.usage")
                .with_unit("USD")
                .build(),
            tool_calls: count("cyber.tool.calls", "{call}"),
            tool_errors: count("cyber.tool.errors", "{call}"),
            permissions: count("cyber.permission.decisions", "{decision}"),
            active_time: meter
                .f64_counter("cyber.active_time")
                .with_unit("s")
                .build(),
        }
    }

    /// Tokens by class and the cost, which an unpriced step leaves out.
    fn usage(&self, tokens: Tokens, cost: Option<f64>, model: &str, provider: &str) {
        for (class, count) in tokens.classes() {
            if count > 0 {
                self.tokens.add(
                    count,
                    &[
                        KeyValue::new("type", class),
                        KeyValue::new("cyber.model", model.to_string()),
                        KeyValue::new("cyber.provider", provider.to_string()),
                    ],
                );
            }
        }
        if let Some(cost) = cost.filter(|cost| *cost > 0.0) {
            self.cost
                .add(cost, &[KeyValue::new("cyber.model", model.to_string())]);
        }
    }
}

/// The event-to-signal mapper.
pub struct Mapper {
    tracer: SdkTracer,
    metrics: Metrics,
    content: Content,
    sessions: Lru<SessionMeta>,
    activity: HashMap<String, Activity>,
    runs: HashMap<String, Context>,
    agents: HashMap<String, AgentSpan>,
    /// Admitted hook receipts: their parent span, chosen when they were admitted.
    hooks: HashMap<String, Context>,
    /// Pending permission requests' actions, by request id.
    asked: HashMap<String, String>,
    /// Admitted prompt text by message id, kept only while `log_prompts` is on.
    prompts: HashMap<String, String>,
    /// Tool names by call id, for calls settling without a span.
    calls: Lru<String>,
}

fn attribute(key: &'static str, value: impl Into<StringValue>) -> KeyValue {
    KeyValue::new(key, OtelValue::String(value.into()))
}

fn text(data: &Value, key: &str) -> String {
    data[key].as_str().unwrap_or_default().to_string()
}

fn at_ms(ms: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(u64::try_from(ms).unwrap_or(0))
}

impl Mapper {
    pub fn new(tracer: SdkTracer, meter: &Meter, content: Content) -> Self {
        Self {
            tracer,
            metrics: Metrics::new(meter),
            content,
            sessions: Lru::new(SESSION_CACHE),
            activity: HashMap::new(),
            runs: HashMap::new(),
            agents: HashMap::new(),
            hooks: HashMap::new(),
            asked: HashMap::new(),
            prompts: HashMap::new(),
            calls: Lru::new(SESSION_CACHE),
        }
    }

    /// The Session `event` is about when the mapper has never seen it (it was created
    /// before a restart): the caller reads it through a [`crate::SessionLookup`] and
    /// passes it to [`Self::learn`] before [`Self::observe`].
    pub fn unknown_session<'a>(&self, event: &'a LiveEvent) -> Option<&'a str> {
        let id = match event {
            LiveEvent::Durable {
                session_id, kind, ..
            } if kind.starts_with("session.") && kind != "session.created.1" => session_id,
            LiveEvent::Idle { session_id }
            | LiveEvent::Retry { session_id, .. }
            | LiveEvent::HookReceipt { session_id, .. } => session_id,
            _ => return None,
        };
        (!self.sessions.contains(id)).then_some(id.as_str())
    }

    /// Remember a Session read through the lookup; `None` when it could not be read.
    pub fn learn(&mut self, id: &str, info: Option<&SessionInfo>) {
        let meta = info.map_or_else(SessionMeta::unknown, SessionMeta::of);
        self.sessions.insert(id.to_string(), meta);
    }

    pub fn observe(&mut self, event: &LiveEvent) {
        match event {
            LiveEvent::Durable {
                session_id,
                kind,
                data,
                ..
            } => self.durable(session_id, kind, data),
            LiveEvent::Idle { session_id } => self.idle(session_id),
            LiveEvent::Retry {
                session_id,
                attempt,
                delay_ms,
                ..
            } => self.retry(session_id, *attempt, *delay_ms),
            LiveEvent::HookReceipt {
                session_id,
                receipt,
            } => self.hook(session_id, receipt),
            _ => {}
        }
    }

    /// End every open span with `cyber.interrupted=true` and no status (shutdown).
    pub fn shutdown(&mut self) {
        let ids: Vec<String> = self.activity.keys().cloned().collect();
        for id in ids {
            self.end_activity(&id, Ending::Interrupted);
        }
        for (_, agent) in self.agents.drain() {
            interrupt(&agent.cx);
        }
        for (_, run) in self.runs.drain() {
            interrupt(&run);
        }
        self.hooks.clear();
    }

    fn durable(&mut self, id: &str, kind: &str, data: &Value) {
        match kind {
            "session.created.1" => self.created(id, data),
            "session.prompt.admitted.1" => self.admitted(data),
            "session.prompt.promoted.1" => self.promoted(id, data),
            "session.step.started.1" => self.step_started(id, data),
            "session.step.ended.1" => self.step_ended(id, data),
            "session.step.failed.1" => self.step_failed(id, data),
            "session.text.ended.1" => self.text_ended(id, data),
            "usage.recorded.1" | "session.compaction.completed.1" => self.call_usage(id, data),
            "session.title.generated.1" if data["usage"].is_object() => self.call_usage(id, data),
            _ => self.durable_more(id, kind, data),
        }
    }

    fn durable_more(&mut self, id: &str, kind: &str, data: &Value) {
        match kind {
            "session.tool.called.1" => self.tool_called(id, data),
            "session.tool.dispatched.1" => self.tool_dispatched(id, data),
            "session.tool.settled.1" => self.tool_settled(id, data),
            "permission.asked.1" => {
                self.asked.insert(text(data, "id"), text(data, "action"));
            }
            "permission.replied.1" => self.permission_replied(id, data),
            "permission.auto_decided.1" => self.auto_decided(id, data),
            "sandbox.denied.1" => self.sandbox(id, data, "sandbox.denied"),
            "sandbox.escalated.1" => self.sandbox(id, data, "sandbox.escalated"),
            _ => self.workflow(id, kind, data),
        }
    }

    fn workflow(&mut self, run_id: &str, kind: &str, data: &Value) {
        match kind {
            "workflow.run.started.1" => self.run_started(run_id, data),
            "workflow.run.completed.1" | "workflow.run.failed.1" | "workflow.run.stopped.1" => {
                self.run_ended(run_id, kind, data);
            }
            "workflow.agent.started.1" => self.agent_started(run_id, data),
            "workflow.agent.settled.1" => self.agent_settled(data),
            _ => {}
        }
    }

    // Sessions and Turns.

    fn meta(&mut self, id: &str) -> SessionMeta {
        self.sessions
            .get(id)
            .cloned()
            .unwrap_or_else(SessionMeta::unknown)
    }

    fn created(&mut self, id: &str, data: &Value) {
        let meta = serde_json::from_value::<SessionInfo>(data["info"].clone())
            .map_or_else(|_| SessionMeta::unknown(), |info| SessionMeta::of(&info));
        self.metrics
            .sessions
            .add(1, &[attribute("cyber.session.kind", meta.kind.as_str())]);
        self.sessions.insert(id.to_string(), meta);
    }

    fn admitted(&mut self, data: &Value) {
        if !self.content.enabled() {
            return;
        }
        if self.prompts.len() >= PROMPT_CACHE {
            self.prompts.clear();
        }
        let prompt = data["parts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        self.prompts.insert(text(data, "message_id"), prompt);
    }

    fn promoted(&mut self, id: &str, data: &Value) {
        let prompt = self.prompts.remove(&text(data, "message_id"));
        let joins = self
            .activity
            .get(id)
            .and_then(|activity| activity.turn.as_ref())
            .is_some_and(|turn| !turn.stepped);
        if !joins {
            self.end_turn(id, Ending::Settled);
            self.start_turn(id);
        }
        if let Some(turn) = self.activity.get_mut(id).and_then(|a| a.turn.as_mut()) {
            turn.prompts.extend(prompt);
        }
    }

    fn start_turn(&mut self, id: &str) {
        let meta = self.meta(id);
        if self.activity.get(id).is_none_or(|a| a.session.is_none()) {
            let session = self.start_session(id, &meta, None);
            self.activity.entry(id.to_string()).or_default().session = Some(session);
        }
        let parent = self.activity[id].session.clone().unwrap_or_default();
        let span = self
            .tracer
            .span_builder("cyber.turn")
            .with_attributes(vec![
                attribute("cyber.session.id", id.to_string()),
                attribute("cyber.session.kind", meta.kind.as_str()),
            ])
            .start_with_context(&self.tracer, &parent);
        let model = split_model(&meta.model).1;
        let turn = Turn {
            cx: parent.with_span(span),
            started: Instant::now(),
            stepped: false,
            failed: None,
            agent: meta.agent,
            mode: meta.mode,
            model,
            tokens: Tokens::default(),
            cost: None,
            finish: None,
            prompts: Vec::new(),
            responses: Vec::new(),
        };
        self.activity.entry(id.to_string()).or_default().turn = Some(turn);
    }

    /// The session span of an activity period, from `start` (else now).
    fn start_session(
        &mut self,
        id: &str,
        meta: &SessionMeta,
        start: Option<SystemTime>,
    ) -> Context {
        let parent = self.session_parent(meta);
        let mut attributes = vec![
            attribute("cyber.session.id", id.to_string()),
            attribute("cyber.session.kind", meta.kind.as_str()),
        ];
        if let Some(title) = self.content.value(&meta.title) {
            attributes.push(attribute("cyber.session.title", title));
        }
        if let Some(directory) = self.content.value(&meta.directory) {
            attributes.push(attribute("cyber.directory", directory));
        }
        let mut builder = self
            .tracer
            .span_builder("cyber.session")
            .with_attributes(attributes);
        builder.start_time = start;
        let span = builder.start_with_context(&self.tracer, &parent);
        parent.with_span(span)
    }

    /// A workflow child's agent span, a subagent's parent Turn (else its parent's session
    /// span), or none: a root Session starts a new trace.
    fn session_parent(&self, meta: &SessionMeta) -> Context {
        let found = match meta.kind {
            SessionKind::WorkflowAgent => meta
                .subagent_name
                .as_deref()
                .and_then(|agent| self.agents.get(agent))
                .map(|agent| agent.cx.clone()),
            SessionKind::Subagent => meta
                .parent_id
                .as_deref()
                .and_then(|parent| self.activity.get(parent))
                .and_then(|activity| {
                    activity
                        .turn
                        .as_ref()
                        .map(|turn| turn.cx.clone())
                        .or_else(|| activity.session.clone())
                }),
            SessionKind::Root => None,
        };
        found.unwrap_or_default()
    }

    fn step_started(&mut self, id: &str, data: &Value) {
        // A child's first input is in its creation, with no promotion.
        if self.activity.get(id).is_none_or(|a| a.turn.is_none()) {
            self.start_turn(id);
        }
        let (provider, model) = (text(data, "provider"), text(data, "model"));
        let Some(activity) = self.activity.get_mut(id) else {
            return;
        };
        if let Some(turn) = activity.turn.as_mut() {
            if let Some(agent) = data["agent"].as_str() {
                turn.agent = agent.to_string();
            }
            if let Some(mode) = data["mode"].as_str() {
                turn.mode = mode.to_string();
            }
            turn.model.clone_from(&model);
            turn.stepped = true;
            turn.failed = None;
        }
        activity.step = Some((provider, model));
    }

    fn step_ended(&mut self, id: &str, data: &Value) {
        let tokens = Tokens::of(&data["usage"]);
        let cost = data["cost"].as_f64();
        let (provider, model) = self.step_model(id);
        self.metrics.usage(tokens, cost, &model, &provider);
        let Some(turn) = self.activity.get_mut(id).and_then(|a| a.turn.as_mut()) else {
            return;
        };
        turn.tokens.add(tokens);
        if let Some(cost) = cost {
            turn.cost = Some(turn.cost.unwrap_or(0.0) + cost);
        }
        turn.finish = data["finish"].as_str().map(str::to_string);
    }

    /// The provider and model of the Session's step in progress, else its own model.
    fn step_model(&mut self, id: &str) -> (String, String) {
        if let Some(step) = self.activity.get(id).and_then(|a| a.step.clone()) {
            return step;
        }
        split_model(&self.meta(id).model)
    }

    fn step_failed(&mut self, id: &str, data: &Value) {
        if let Some(turn) = self.activity.get_mut(id).and_then(|a| a.turn.as_mut()) {
            turn.failed = Some(data["kind"] == "interrupted");
        }
    }

    fn text_ended(&mut self, id: &str, data: &Value) {
        if !self.content.enabled() {
            return;
        }
        if let Some(turn) = self.activity.get_mut(id).and_then(|a| a.turn.as_mut()) {
            turn.responses.push(text(data, "text"));
        }
    }

    /// A model call billed outside a step (an auxiliary call, a title, a compaction or an
    /// auto-mode review), with the model the usage ledger gives it: the event's, else
    /// the Session's.
    fn call_usage(&mut self, id: &str, data: &Value) {
        let reference = match data["model"].as_str().or(data["evaluator"].as_str()) {
            Some(reference) => reference.to_string(),
            None => self.meta(id).model,
        };
        let (provider, model) = call_model(data["provider"].as_str(), &reference);
        self.metrics.usage(
            Tokens::of(&data["usage"]),
            data["cost"].as_f64(),
            &model,
            &provider,
        );
    }

    fn retry(&mut self, id: &str, attempt: u32, delay_ms: u64) {
        if let Some(turn) = self.activity.get(id).and_then(|a| a.turn.as_ref()) {
            turn.cx.span().add_event(
                "retry",
                vec![
                    KeyValue::new("attempt", i64::from(attempt)),
                    KeyValue::new("delay_ms", i64::try_from(delay_ms).unwrap_or(i64::MAX)),
                ],
            );
        }
    }

    fn idle(&mut self, id: &str) {
        self.end_activity(id, Ending::Settled);
    }

    fn end_activity(&mut self, id: &str, ending: Ending) {
        self.end_turn(id, ending);
        let Some(mut activity) = self.activity.remove(id) else {
            return;
        };
        for (_, tool) in activity.tools.drain() {
            interrupt(&tool);
        }
        if let Some(session) = activity.session {
            match ending {
                Ending::Settled => session.span().end(),
                Ending::Interrupted => interrupt(&session),
            }
        }
    }

    fn end_turn(&mut self, id: &str, ending: Ending) {
        let Some(turn) = self.activity.get_mut(id).and_then(|a| a.turn.take()) else {
            return;
        };
        let kind = self.meta(id).kind;
        // A step the stop interrupted ends its Turn as interrupted too.
        let ending = if turn.failed == Some(true) {
            Ending::Interrupted
        } else {
            ending
        };
        let outcome = match ending {
            Ending::Interrupted => "interrupted",
            Ending::Settled if turn.failed.is_some() => "failed",
            Ending::Settled => "completed",
        };
        self.metrics.turns.add(
            1,
            &[
                KeyValue::new("outcome", outcome),
                attribute("cyber.agent", turn.agent.clone()),
            ],
        );
        self.metrics.active_time.add(
            turn.started.elapsed().as_secs_f64(),
            &[attribute("cyber.session.kind", kind.as_str())],
        );
        self.close_turn(turn, ending);
    }

    fn close_turn(&self, turn: Turn, ending: Ending) {
        let span = turn.cx.span();
        span.set_attribute(attribute("cyber.agent", turn.agent));
        span.set_attribute(attribute("cyber.mode", turn.mode));
        span.set_attribute(attribute("cyber.model", turn.model));
        for (class, count) in turn.tokens.classes() {
            let key = format!("cyber.tokens.{class}");
            span.set_attribute(KeyValue::new(key, i64::try_from(count).unwrap_or(i64::MAX)));
        }
        if let Some(cost) = turn.cost {
            span.set_attribute(KeyValue::new("cyber.cost.usd", cost));
        }
        if let Some(finish) = turn.finish {
            span.set_attribute(attribute("cyber.finish_reason", finish));
        }
        if let Some(prompt) = self.content.value(&turn.prompts.join("\n")) {
            span.set_attribute(attribute("cyber.prompt", prompt));
        }
        if let Some(response) = self.content.value(&turn.responses.join("\n")) {
            span.set_attribute(attribute("cyber.response", response));
        }
        match ending {
            Ending::Interrupted => interrupt(&turn.cx),
            Ending::Settled => {
                if turn.failed.is_some() {
                    span.set_status(Status::error("the Turn failed"));
                }
                span.end();
            }
        }
    }

    // Tools, permissions and sandbox events.

    fn tool_called(&mut self, id: &str, data: &Value) {
        let (call_id, name) = (text(data, "call_id"), text(data, "name"));
        self.calls.insert(call_id.clone(), name.clone());
        let kind = self.meta(id).kind;
        let Some(parent) = self
            .activity
            .get(id)
            .and_then(|a| a.turn.as_ref())
            .map(|turn| turn.cx.clone())
        else {
            return;
        };
        let mut attributes = vec![
            attribute("cyber.session.id", id.to_string()),
            attribute("cyber.session.kind", kind.as_str()),
            attribute("cyber.tool.name", name),
            attribute("cyber.call.id", call_id.clone()),
        ];
        let input = match &data["input"] {
            Value::Null => self.content.value(data["arguments"].as_str().unwrap_or("")),
            input => self.content.json(input),
        };
        if let Some(input) = input {
            attributes.push(attribute("cyber.tool.input", input));
        }
        let span = self
            .tracer
            .span_builder("cyber.tool")
            .with_attributes(attributes)
            .start_with_context(&self.tracer, &parent);
        let cx = parent.with_span(span);
        if let Some(activity) = self.activity.get_mut(id) {
            activity.tools.insert(call_id, cx);
        }
    }

    fn tool_dispatched(&mut self, id: &str, data: &Value) {
        if let Some(activity) = self.activity.get_mut(id) {
            let attempt = data["attempt"].as_i64().unwrap_or(1);
            activity.attempts.insert(text(data, "call_id"), attempt);
        }
    }

    fn tool(&self, id: &str, call_id: &str) -> Option<&Context> {
        self.activity.get(id)?.tools.get(call_id)
    }

    fn tool_settled(&mut self, id: &str, data: &Value) {
        let call_id = text(data, "call_id");
        let status = text(data, "status");
        let name = self
            .calls
            .get(&call_id)
            .cloned()
            .unwrap_or_else(|| "unknown".into());
        let failed = status == "error";
        self.metrics.tool_calls.add(
            1,
            &[
                attribute("cyber.tool.name", name.clone()),
                attribute("outcome", status.clone()),
            ],
        );
        if failed {
            self.metrics
                .tool_errors
                .add(1, &[attribute("cyber.tool.name", name)]);
        }
        let Some(activity) = self.activity.get_mut(id) else {
            return;
        };
        let attempt = activity.attempts.remove(&call_id).unwrap_or(1);
        let Some(tool) = activity.tools.remove(&call_id) else {
            return;
        };
        let span = tool.span();
        // A stop or the server's shutdown cut the call off: it ends as an open span does
        // at shutdown, and a restarted server never settles it again.
        if status == "interrupted" {
            span.set_attribute(KeyValue::new("cyber.interrupted", true));
        }
        span.set_attribute(KeyValue::new("cyber.tool.attempt", attempt));
        span.set_attribute(attribute("cyber.tool.outcome", status));
        if let Some(output) = self.content.value(data["output"].as_str().unwrap_or("")) {
            span.set_attribute(attribute("cyber.tool.output", output));
        }
        if failed {
            span.set_status(Status::error("the tool call failed"));
        }
        span.end();
    }

    fn permission_replied(&mut self, id: &str, data: &Value) {
        let action = self
            .asked
            .remove(&text(data, "request_id"))
            .unwrap_or_default();
        // The runtime withdrew the request (a stop, an abandoned call): nobody decided.
        if data["cancelled"] == true {
            return;
        }
        let decision = match data["reply"]["reply"].as_str() {
            Some("once" | "always") => "allow",
            Some("reject") => "deny",
            _ => return,
        };
        self.decision(id, &text(data, "call_id"), decision, "user", action);
    }

    fn auto_decided(&mut self, id: &str, data: &Value) {
        // The evaluator's own model call, when it made one.
        if data["usage"].is_object() {
            self.call_usage(id, data);
        }
        let decision = match data["decision"].as_str() {
            Some("allow") => "allow",
            Some("block") => "deny",
            _ => return,
        };
        self.decision(
            id,
            &text(data, "call_id"),
            decision,
            "auto",
            text(data, "action"),
        );
    }

    fn decision(
        &self,
        id: &str,
        call_id: &str,
        decision: &'static str,
        source: &'static str,
        action: String,
    ) {
        self.metrics.permissions.add(
            1,
            &[
                KeyValue::new("decision", decision),
                KeyValue::new("source", source),
                attribute("cyber.permission.action", action),
            ],
        );
        if let Some(tool) = self.tool(id, call_id) {
            tool.span().add_event(
                "permission.decision",
                vec![
                    KeyValue::new("decision", decision),
                    KeyValue::new("source", source),
                ],
            );
        }
    }

    fn sandbox(&self, id: &str, data: &Value, name: &'static str) {
        let Some(tool) = self.tool(id, &text(data, "call_id")) else {
            return;
        };
        let mut attributes = Vec::new();
        if let Some(operation) = data["operation"].as_str() {
            attributes.push(attribute("operation", operation.to_string()));
        }
        let target = data["target"].as_str().or(data["command"].as_str());
        if let Some(target) = target.and_then(|target| self.content.value(target)) {
            attributes.push(attribute("target", target));
        }
        tool.span().add_event(name, attributes);
    }

    // Hooks.

    fn hook(&mut self, id: &str, receipt: &HookReceipt) {
        if !receipt.settled() {
            self.hook_admitted(id, receipt);
            return;
        }
        // A receipt admitted before a restart has no span.
        let Some(parent) = self.hooks.remove(&receipt.id) else {
            return;
        };
        let kind = self.meta(id).kind;
        let start = at_ms(receipt.started_ms);
        let end = start + Duration::from_millis(receipt.duration_ms.unwrap_or(0));
        let outcome = receipt.outcome.map_or_else(|| "unknown".into(), name);
        let mut attributes = vec![
            attribute("cyber.session.id", id.to_string()),
            attribute("cyber.session.kind", kind.as_str()),
            attribute("cyber.hook.id", receipt.hook_id.clone()),
            attribute("cyber.hook.event", receipt.event.clone()),
            attribute("cyber.hook.scope", name(receipt.scope)),
            attribute("cyber.hook.outcome", outcome),
        ];
        if let Some(decision) = receipt.decision {
            attributes.push(attribute("cyber.hook.decision", name(decision)));
        }
        let mut span = self
            .tracer
            .span_builder("cyber.hook")
            .with_start_time(start)
            .with_attributes(attributes)
            .start_with_context(&self.tracer, &parent);
        if matches!(
            receipt.outcome,
            Some(HookOutcome::Error | HookOutcome::Timeout)
        ) {
            span.set_status(Status::error("the hook failed"));
        }
        span.end_with_timestamp(end);
        // A period the hook opened ends with it unless a Turn follows.
        let opened_only = self
            .activity
            .get(id)
            .is_some_and(|a| a.turn.is_none() && a.tools.is_empty());
        if opened_only && !leads_to_turn(receipt) {
            self.end_activity(id, Ending::Settled);
        }
    }

    /// A hook admitted while the Session has no activity period (`UserPromptSubmit` at
    /// admission, `SessionStart` before the first promotion, a configuration hook) opens
    /// its session span, from the receipt's start, so the hook is under its Session.
    fn hook_admitted(&mut self, id: &str, receipt: &HookReceipt) {
        if !self.activity.contains_key(id) {
            let meta = self.meta(id);
            let start = at_ms(receipt.started_ms);
            let session = self.start_session(id, &meta, Some(start));
            self.activity.entry(id.to_string()).or_default().session = Some(session);
        }
        let parent = self.hook_parent(id, receipt.call_id.as_deref());
        self.hooks.insert(receipt.id.clone(), parent);
    }

    /// The hook's tool span, else the open Turn, else the session span.
    fn hook_parent(&self, id: &str, call_id: Option<&str>) -> Context {
        let Some(activity) = self.activity.get(id) else {
            return Context::new();
        };
        call_id
            .and_then(|call_id| activity.tools.get(call_id).cloned())
            .or_else(|| activity.turn.as_ref().map(|turn| turn.cx.clone()))
            .or_else(|| activity.session.clone())
            .unwrap_or_default()
    }

    // Workflow runs and agents.

    fn run_started(&mut self, run_id: &str, data: &Value) {
        let owner = text(data, "session_id");
        let links: Vec<Link> = self
            .activity
            .get(&owner)
            .and_then(|a| a.turn.as_ref())
            .map(|turn| Link::with_context(turn.cx.span().span_context().clone()))
            .into_iter()
            .collect();
        let span = self
            .tracer
            .span_builder("cyber.workflow.run")
            .with_links(links)
            .with_attributes(vec![
                attribute("cyber.workflow.run_id", run_id.to_string()),
                attribute("cyber.workflow.name", text(data, "name")),
                attribute("cyber.session.id", owner),
            ])
            .start_with_context(&self.tracer, &Context::new());
        self.runs
            .insert(run_id.to_string(), Context::new().with_span(span));
    }

    fn run_ended(&mut self, run_id: &str, kind: &str, data: &Value) {
        let agents: Vec<String> = self
            .agents
            .iter()
            .filter(|(_, agent)| agent.run_id == run_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in agents {
            if let Some(agent) = self.agents.remove(&id) {
                interrupt(&agent.cx);
            }
        }
        let Some(run) = self.runs.remove(run_id) else {
            return;
        };
        let status = data["status"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| kind.split('.').nth(2).unwrap_or_default().to_string());
        let span = run.span();
        if kind == "workflow.run.failed.1" {
            span.set_status(Status::error("the run failed"));
        }
        span.set_attribute(attribute("cyber.workflow.status", status));
        span.end();
    }

    fn agent_started(&mut self, run_id: &str, data: &Value) {
        let agent_id = text(data, "id");
        if self.agents.contains_key(&agent_id) {
            return;
        }
        // An agent of a run started before a restart has no span.
        let Some(run) = self.runs.get(run_id).cloned() else {
            return;
        };
        let mut attributes = vec![
            attribute("cyber.workflow.run_id", run_id.to_string()),
            attribute("cyber.workflow.agent_id", agent_id.clone()),
        ];
        if let Some(phase) = data["phase"].as_str() {
            attributes.push(attribute("cyber.workflow.phase", phase.to_string()));
        }
        if let Some(model) = data["model"].as_str() {
            attributes.push(attribute("cyber.model", model.to_string()));
        }
        let span = self
            .tracer
            .span_builder("cyber.workflow.agent")
            .with_attributes(attributes)
            .start_with_context(&self.tracer, &run);
        let cx = run.with_span(span);
        self.agents.insert(
            agent_id,
            AgentSpan {
                cx,
                run_id: run_id.to_string(),
            },
        );
    }

    fn agent_settled(&mut self, data: &Value) {
        let Some(agent) = self.agents.remove(&text(data, "id")) else {
            return;
        };
        let span = agent.cx.span();
        let status = text(data, "status");
        if status == "failed" {
            span.set_status(Status::error("the agent failed"));
        }
        span.set_attribute(attribute("cyber.workflow.status", status));
        span.set_attribute(KeyValue::new(
            "cyber.cost.usd",
            data["cost"].as_f64().unwrap_or(0.0),
        ));
        span.set_attribute(KeyValue::new(
            "cyber.tokens.total",
            data["tokens"].as_i64().unwrap_or(0),
        ));
        span.end();
    }
}

/// A Session's `provider/model` reference as `(provider, model)`.
fn split_model(reference: &str) -> (String, String) {
    match reference.split_once('/') {
        Some((provider, model)) => (provider.to_string(), model.to_string()),
        None => (String::new(), reference.to_string()),
    }
}

/// Whether a Turn follows the settled `receipt` in its Session: `SessionStart` runs before
/// the first promotion, and an allowed `UserPromptSubmit` admits its prompt.
fn leads_to_turn(receipt: &HookReceipt) -> bool {
    match receipt.event.as_str() {
        "SessionStart" => true,
        "UserPromptSubmit" => {
            !matches!(receipt.decision, Some(HookAction::Deny | HookAction::Block))
        }
        _ => false,
    }
}

/// The `(provider, model)` of a billed call's model reference, as the usage ledger splits
/// it: a known provider is stripped from the reference, else its first segment is.
fn call_model(provider: Option<&str>, reference: &str) -> (String, String) {
    let Some(provider) = provider else {
        return split_model(reference);
    };
    let model = reference
        .strip_prefix(provider)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(reference);
    (provider.to_string(), model.to_string())
}

/// How open spans end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    /// Their work settled.
    Settled,
    /// The server stopped first.
    Interrupted,
}

/// End the span of `cx` as interrupted: `cyber.interrupted=true` and no status.
fn interrupt(cx: &Context) {
    let span = cx.span();
    span.set_attribute(KeyValue::new("cyber.interrupted", true));
    span.end();
}

/// The serde name of a unit enum value, such as `skipped`.
fn name(value: impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "mapper_tests.rs"]
mod tests;
