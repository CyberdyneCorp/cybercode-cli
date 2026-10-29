//! The mapper on scripted `LiveEvent` sequences with the SDK in-memory exporters.

use cyber_core::hooks::{HookAction, HookOutcome, HookScope};
use cyber_server::runtime::{HookReceipt, LiveEvent};
use opentelemetry::trace::{SpanId, Status};
use opentelemetry::{KeyValue, Value as OtelValue};
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};
use serde_json::{Value, json};

use super::Mapper;
use crate::pipeline::Providers;
use crate::privacy::Content;

struct Fixture {
    mapper: Mapper,
    spans: InMemorySpanExporter,
    metrics: InMemoryMetricExporter,
    meters: SdkMeterProvider,
}

fn fixture(content: Content) -> Fixture {
    let spans = InMemorySpanExporter::default();
    let traces = SdkTracerProvider::builder()
        .with_simple_exporter(spans.clone())
        .build();
    let metrics = InMemoryMetricExporter::default();
    let meters = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(metrics.clone()).build())
        .build();
    let mapper = Providers::of(traces, meters.clone()).mapper(content);
    Fixture {
        mapper,
        spans,
        metrics,
        meters,
    }
}

impl Fixture {
    fn feed(&mut self, events: impl IntoIterator<Item = LiveEvent>) {
        for event in events {
            self.mapper.observe(&event);
        }
    }

    fn spans(&self) -> Vec<SpanData> {
        self.spans.get_finished_spans().unwrap()
    }

    fn span(&self, name: &str) -> SpanData {
        self.named(name)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("no {name} span"))
    }

    fn named(&self, name: &str) -> Vec<SpanData> {
        self.spans()
            .into_iter()
            .filter(|span| span.name == name)
            .collect()
    }

    /// Every data point of the last collection: name, value and attributes.
    fn points(&self) -> Vec<Point> {
        self.meters.force_flush().unwrap();
        let exported = self.metrics.get_finished_metrics().unwrap();
        let Some(last) = exported.last() else {
            return Vec::new();
        };
        let mut points = Vec::new();
        for scope in last.scope_metrics() {
            for metric in scope.metrics() {
                points.extend(sum_points(metric.name(), metric.data()));
            }
        }
        points
    }

    fn total(&self, name: &str, attributes: &[(&str, &str)]) -> f64 {
        self.points()
            .iter()
            .filter(|point| point.name == name)
            .filter(|point| {
                attributes
                    .iter()
                    .all(|(key, value)| point.attribute(key) == Some(*value))
            })
            .map(|point| point.value)
            .sum()
    }
}

#[derive(Debug)]
struct Point {
    name: String,
    value: f64,
    attributes: Vec<(String, String)>,
}

impl Point {
    fn attribute(&self, key: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, value)| value.as_str())
    }
}

fn sum_points(name: &str, data: &AggregatedMetrics) -> Vec<Point> {
    let point = |value: f64, attributes: Vec<&KeyValue>| Point {
        name: name.to_string(),
        value,
        attributes: attributes
            .into_iter()
            .map(|kv| (kv.key.to_string(), kv.value.to_string()))
            .collect(),
    };
    match data {
        AggregatedMetrics::U64(MetricData::Sum(sum)) => sum
            .data_points()
            .map(|p| point(p.value() as f64, p.attributes().collect()))
            .collect(),
        AggregatedMetrics::F64(MetricData::Sum(sum)) => sum
            .data_points()
            .map(|p| point(p.value(), p.attributes().collect()))
            .collect(),
        _ => Vec::new(),
    }
}

fn attr<'a>(span: &'a SpanData, key: &str) -> Option<&'a OtelValue> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| &kv.value)
}

fn attr_text(span: &SpanData, key: &str) -> Option<String> {
    attr(span, key).map(|value| value.to_string())
}

fn is_child(child: &SpanData, parent: &SpanData) -> bool {
    child.parent_span_id == parent.span_context.span_id()
        && child.span_context.trace_id() == parent.span_context.trace_id()
}

fn durable(session: &str, kind: &str, data: Value) -> LiveEvent {
    LiveEvent::Durable {
        session_id: session.into(),
        seq: 1,
        kind: kind.into(),
        data,
    }
}

fn created(id: &str, parent: Option<&str>, agent: Option<&str>, run: Option<&str>) -> LiveEvent {
    durable(
        id,
        "session.created.1",
        json!({"info": {
            "id": id, "title": "Fix the build", "directory": "/work/repo",
            "parent_id": parent, "subagent_name": agent, "workflow_run_id": run,
            "agent": "build", "model": "test/main", "mode": "default", "created_ms": 0,
        }}),
    )
}

fn admitted(id: &str, message: &str, text: &str) -> LiveEvent {
    durable(
        id,
        "session.prompt.admitted.1",
        json!({"message_id": message, "parts": [{"type": "text", "text": text}]}),
    )
}

fn promoted(id: &str, message: &str) -> LiveEvent {
    durable(
        id,
        "session.prompt.promoted.1",
        json!({"message_id": message}),
    )
}

fn step(id: &str) -> LiveEvent {
    durable(
        id,
        "session.step.started.1",
        json!({"message_id": "msg_a", "provider": "test", "model": "main", "agent": "build",
            "mode": "default", "tools": true}),
    )
}

fn step_ended(id: &str, cost: Option<f64>) -> LiveEvent {
    durable(
        id,
        "session.step.ended.1",
        json!({"message_id": "msg_a", "finish": "stop", "cost": cost,
            "usage": {"input": 10, "output": 5, "reasoning": 0, "cache_read": 2, "cache_write": 0}}),
    )
}

fn called(id: &str, call: &str, name: &str, input: Value) -> LiveEvent {
    durable(
        id,
        "session.tool.called.1",
        json!({"message_id": "msg_a", "call_id": call, "name": name,
            "arguments": input.to_string(), "input": input, "retry_safety": "safe"}),
    )
}

fn settled(id: &str, call: &str, status: &str) -> LiveEvent {
    durable(
        id,
        "session.tool.settled.1",
        json!({"call_id": call, "status": status, "output": "tool output text"}),
    )
}

fn idle(id: &str) -> LiveEvent {
    LiveEvent::Idle {
        session_id: id.into(),
    }
}

fn receipt(call: Option<&str>, outcome: Option<HookOutcome>) -> HookReceipt {
    HookReceipt {
        id: "hke_1".into(),
        hook_id: "guard".into(),
        event: "PreToolUse".into(),
        scope: HookScope::Global,
        call_id: call.map(str::to_string),
        tool_name: Some("bash".into()),
        started_ms: 1_700_000_000_000,
        duration_ms: outcome.map(|_| 250),
        outcome,
        decision: outcome.map(|_| HookAction::Deny),
    }
}

/// The content attributes no default export may carry.
const CONTENT: [&str; 7] = [
    "cyber.prompt",
    "cyber.response",
    "cyber.tool.input",
    "cyber.tool.output",
    "cyber.session.title",
    "cyber.directory",
    "target",
];

#[test]
fn turn_and_tool_spans_are_parented_under_the_session_span() {
    let mut f = fixture(Content::default());
    f.feed([
        created("ses_a", None, None, None),
        admitted("ses_a", "msg_1", "please fix it"),
        promoted("ses_a", "msg_1"),
        step("ses_a"),
        called("ses_a", "call_1", "bash", json!({"command": "ls"})),
        settled("ses_a", "call_1", "ok"),
        step_ended("ses_a", Some(0.25)),
        idle("ses_a"),
    ]);
    let (session, turn, tool) = (
        f.span("cyber.session"),
        f.span("cyber.turn"),
        f.span("cyber.tool"),
    );
    assert_eq!(
        session.parent_span_id,
        SpanId::INVALID,
        "a root Session starts a trace"
    );
    assert!(is_child(&turn, &session));
    assert!(is_child(&tool, &turn));
    assert_eq!(attr_text(&tool, "cyber.tool.name").as_deref(), Some("bash"));
    assert_eq!(
        attr_text(&tool, "cyber.tool.outcome").as_deref(),
        Some("ok")
    );
    assert_eq!(
        attr_text(&turn, "cyber.session.kind").as_deref(),
        Some("root")
    );
    assert_eq!(attr_text(&turn, "cyber.model").as_deref(), Some("main"));
    assert_eq!(
        attr_text(&turn, "cyber.tokens.input").as_deref(),
        Some("10")
    );
    assert_eq!(attr_text(&turn, "cyber.cost.usd").as_deref(), Some("0.25"));
    assert_eq!(
        attr_text(&turn, "cyber.finish_reason").as_deref(),
        Some("stop")
    );
    for span in f.spans() {
        for key in CONTENT {
            assert!(attr(&span, key).is_none(), "{key} on {}", span.name);
        }
        assert!(attr(&span, "cyber.interrupted").is_none());
    }
}

#[test]
fn a_subagent_session_is_a_child_of_its_parents_turn() {
    let mut f = fixture(Content::default());
    f.feed([
        created("ses_p", None, None, None),
        promoted("ses_p", "msg_1"),
        step("ses_p"),
        called("ses_p", "call_1", "agent", json!({})),
        created("ses_c", Some("ses_p"), Some("general"), None),
        promoted("ses_c", "msg_c"),
        step("ses_c"),
        idle("ses_c"),
        settled("ses_p", "call_1", "ok"),
        idle("ses_p"),
    ]);
    let sessions = f.named("cyber.session");
    let turns = f.named("cyber.turn");
    let kind = |span: &SpanData| attr_text(span, "cyber.session.kind").unwrap();
    let child = sessions.iter().find(|s| kind(s) == "subagent").unwrap();
    let parent_turn = turns.iter().find(|t| kind(t) == "root").unwrap();
    let child_turn = turns.iter().find(|t| kind(t) == "subagent").unwrap();
    assert!(is_child(child, parent_turn));
    assert!(is_child(child_turn, child));
}

#[test]
fn a_workflow_run_roots_its_own_trace_with_agents_and_their_sessions_under_it() {
    let mut f = fixture(Content::default());
    f.feed([
        created("ses_o", None, None, None),
        promoted("ses_o", "msg_1"),
        step("ses_o"),
        durable(
            "run_1",
            "workflow.run.started.1",
            json!({"id": "run_1", "session_id": "ses_o", "name": "pair", "status": "running"}),
        ),
        durable(
            "run_1",
            "workflow.agent.started.1",
            json!({"id": "agt_1", "run_id": "run_1", "phase": "Fan out", "status": "running"}),
        ),
        created("ses_w", Some("ses_o"), Some("agt_1"), Some("run_1")),
        promoted("ses_w", "msg_w"),
        step("ses_w"),
        idle("ses_w"),
        durable(
            "run_1",
            "workflow.agent.settled.1",
            json!({"id": "agt_1", "run_id": "run_1", "status": "completed", "cost": 0.5, "tokens": 120}),
        ),
        durable(
            "run_1",
            "workflow.run.completed.1",
            json!({"id": "run_1", "status": "completed"}),
        ),
        idle("ses_o"),
    ]);
    let run = f.span("cyber.workflow.run");
    let agent = f.span("cyber.workflow.agent");
    let kind = |span: &SpanData| attr_text(span, "cyber.session.kind").unwrap();
    let sessions = f.named("cyber.session");
    let child = sessions
        .iter()
        .find(|s| kind(s) == "workflow_agent")
        .unwrap();
    let owner_turn = f
        .named("cyber.turn")
        .into_iter()
        .find(|t| kind(t) == "root")
        .unwrap();
    assert_eq!(
        run.parent_span_id,
        SpanId::INVALID,
        "the run is a trace root"
    );
    assert_ne!(
        run.span_context.trace_id(),
        owner_turn.span_context.trace_id()
    );
    let links: Vec<_> = run.links.iter().collect();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].span_context, owner_turn.span_context);
    assert!(is_child(&agent, &run));
    assert!(is_child(child, &agent));
    assert_eq!(
        attr_text(&agent, "cyber.workflow.status").as_deref(),
        Some("completed")
    );
    assert_eq!(
        attr_text(&run, "cyber.workflow.name").as_deref(),
        Some("pair")
    );
}

#[test]
fn a_promotion_before_the_first_step_joins_the_open_turn() {
    let mut f = fixture(Content::new(true, []));
    f.feed([
        created("ses_a", None, None, None),
        admitted("ses_a", "msg_1", "first"),
        admitted("ses_a", "msg_2", "second"),
        admitted("ses_a", "msg_3", "third"),
        promoted("ses_a", "msg_1"),
        promoted("ses_a", "msg_2"),
        step("ses_a"),
        step_ended("ses_a", None),
        promoted("ses_a", "msg_3"),
        step("ses_a"),
        idle("ses_a"),
    ]);
    let turns = f.named("cyber.turn");
    assert_eq!(turns.len(), 2, "{turns:?}");
    let prompts: Vec<String> = turns
        .iter()
        .map(|turn| attr_text(turn, "cyber.prompt").unwrap())
        .collect();
    assert!(
        prompts.contains(&"first\nsecond".to_string()),
        "{prompts:?}"
    );
    assert!(prompts.contains(&"third".to_string()), "{prompts:?}");
    assert_eq!(f.named("cyber.session").len(), 1);
    assert_eq!(
        f.total("cyber.turn.count", &[("outcome", "completed")]),
        2.0
    );
}

/// A Turn with a permitted call, an auto-blocked failed call, a priced and an unpriced
/// step, and an unpriced auxiliary model call.
fn scripted_usage() -> Fixture {
    let mut f = fixture(Content::default());
    f.feed([
        created("ses_a", None, None, None),
        promoted("ses_a", "msg_1"),
        step("ses_a"),
        called("ses_a", "call_1", "bash", json!({})),
        durable(
            "ses_a",
            "permission.asked.1",
            json!({"id": "per_1", "call_id": "call_1", "action": "bash"}),
        ),
        durable(
            "ses_a",
            "permission.replied.1",
            json!({"request_id": "per_1", "call_id": "call_1", "reply": {"reply": "once"}}),
        ),
        settled("ses_a", "call_1", "ok"),
        called("ses_a", "call_2", "edit", json!({})),
        durable(
            "ses_a",
            "permission.auto_decided.1",
            json!({"call_id": "call_2", "action": "edit", "decision": "block", "reason": "r"}),
        ),
        settled("ses_a", "call_2", "error"),
        step_ended("ses_a", Some(0.25)),
        step("ses_a"),
        step_ended("ses_a", None),
        durable(
            "ses_a",
            "usage.recorded.1",
            json!({"provider": "test", "model": "small", "purpose": "title", "duration_ms": 3,
                "usage": {"input": 7, "output": 1, "reasoning": 0, "cache_read": 0, "cache_write": 0},
                "cost": null}),
        ),
        idle("ses_a"),
    ]);
    f
}

#[test]
fn metrics_count_settled_work_and_unpriced_steps_add_no_cost() {
    let f = scripted_usage();
    assert_eq!(
        f.total("cyber.session.count", &[("cyber.session.kind", "root")]),
        1.0
    );
    assert_eq!(
        f.total(
            "cyber.turn.count",
            &[("outcome", "completed"), ("cyber.agent", "build")]
        ),
        1.0
    );
    let input = [
        ("type", "input"),
        ("cyber.model", "main"),
        ("cyber.provider", "test"),
    ];
    assert_eq!(f.total("cyber.token.usage", &input), 20.0);
    assert_eq!(f.total("cyber.token.usage", &[("type", "cache_read")]), 4.0);
    assert_eq!(
        f.total("cyber.token.usage", &[("cyber.model", "small")]),
        8.0
    );
    assert_eq!(
        f.total("cyber.cost.usage", &[("cyber.model", "main")]),
        0.25
    );
    assert_eq!(
        f.total("cyber.cost.usage", &[("cyber.model", "small")]),
        0.0
    );
    assert_eq!(
        f.total(
            "cyber.tool.calls",
            &[("cyber.tool.name", "bash"), ("outcome", "ok")]
        ),
        1.0
    );
    assert_eq!(
        f.total("cyber.tool.errors", &[("cyber.tool.name", "edit")]),
        1.0
    );
    assert_eq!(
        f.total("cyber.tool.errors", &[("cyber.tool.name", "bash")]),
        0.0
    );
    let allow = [
        ("decision", "allow"),
        ("source", "user"),
        ("cyber.permission.action", "bash"),
    ];
    assert_eq!(f.total("cyber.permission.decisions", &allow), 1.0);
    let deny = [
        ("decision", "deny"),
        ("source", "auto"),
        ("cyber.permission.action", "edit"),
    ];
    assert_eq!(f.total("cyber.permission.decisions", &deny), 1.0);
    assert!(f.total("cyber.active_time", &[("cyber.session.kind", "root")]) >= 0.0);
}

#[test]
fn metrics_carry_no_ids_and_tool_spans_carry_decisions() {
    let f = scripted_usage();
    let points = f.points();
    assert!(points.iter().any(|p| p.name == "cyber.active_time"));
    for point in &points {
        for (key, value) in &point.attributes {
            assert!(!key.ends_with(".id"), "{point:?}");
            assert!(
                !value.starts_with("ses_") && !value.starts_with("call_"),
                "{point:?}"
            );
        }
    }
    let tool = f
        .named("cyber.tool")
        .into_iter()
        .find(|t| attr_text(t, "cyber.tool.name").as_deref() == Some("bash"))
        .unwrap();
    let events: Vec<_> = tool.events.iter().map(|e| e.name.to_string()).collect();
    assert_eq!(events, ["permission.decision"]);
    let failed = f
        .named("cyber.tool")
        .into_iter()
        .find(|t| attr_text(t, "cyber.tool.name").as_deref() == Some("edit"))
        .unwrap();
    assert!(matches!(failed.status, Status::Error { .. }));
}

#[test]
fn work_settling_after_a_restart_records_metrics_and_no_span() {
    let mut f = fixture(Content::default());
    // A Session created before the restart, read once through the lookup.
    let event = settled("ses_old", "call_9", "ok");
    assert_eq!(f.mapper.unknown_session(&event), Some("ses_old"));
    f.mapper.learn("ses_old", None);
    assert_eq!(f.mapper.unknown_session(&event), None);
    f.feed([
        event,
        step_ended("ses_old", Some(0.1)),
        durable(
            "run_9",
            "workflow.agent.settled.1",
            json!({"id": "agt_9", "run_id": "run_9", "status": "completed", "cost": 0.0, "tokens": 0}),
        ),
        durable("run_9", "workflow.run.completed.1", json!({"status": "completed"})),
        LiveEvent::HookReceipt {
            session_id: "ses_old".into(),
            receipt: receipt(None, Some(HookOutcome::Ok)),
        },
        idle("ses_old"),
    ]);
    assert!(f.spans().is_empty(), "{:?}", f.spans());
    assert_eq!(f.total("cyber.tool.calls", &[("outcome", "ok")]), 1.0);
    assert_eq!(f.total("cyber.token.usage", &[("type", "input")]), 10.0);
    assert_eq!(f.total("cyber.turn.count", &[]), 0.0);
}

#[test]
fn shutdown_ends_open_spans_as_interrupted() {
    let mut f = fixture(Content::default());
    f.feed([
        created("ses_a", None, None, None),
        promoted("ses_a", "msg_1"),
        step("ses_a"),
        called("ses_a", "call_1", "bash", json!({})),
        durable(
            "run_1",
            "workflow.run.started.1",
            json!({"session_id": "ses_a", "name": "pair"}),
        ),
        durable(
            "run_1",
            "workflow.agent.started.1",
            json!({"id": "agt_1", "run_id": "run_1"}),
        ),
    ]);
    assert!(f.spans().is_empty());
    f.mapper.shutdown();
    let spans = f.spans();
    let names: Vec<_> = spans.iter().map(|s| s.name.to_string()).collect();
    for name in [
        "cyber.tool",
        "cyber.turn",
        "cyber.session",
        "cyber.workflow.agent",
        "cyber.workflow.run",
    ] {
        assert!(names.contains(&name.to_string()), "{names:?}");
    }
    for span in &spans {
        assert_eq!(
            attr_text(span, "cyber.interrupted").as_deref(),
            Some("true")
        );
        assert_eq!(span.status, Status::Unset, "{}", span.name);
    }
    assert_eq!(
        f.total("cyber.turn.count", &[("outcome", "interrupted")]),
        1.0
    );
}

/// A call that settles as interrupted (the shutdown's Drain cancellation) ends like an
/// open span at shutdown, and is counted once.
#[test]
fn an_interrupted_call_ends_its_span_as_interrupted() {
    let mut f = fixture(Content::default());
    f.feed([
        created("ses_a", None, None, None),
        promoted("ses_a", "msg_1"),
        step("ses_a"),
        called("ses_a", "call_1", "read", json!({})),
        settled("ses_a", "call_1", "interrupted"),
    ]);
    let tool = f.span("cyber.tool");
    assert_eq!(
        attr_text(&tool, "cyber.interrupted").as_deref(),
        Some("true")
    );
    assert_eq!(tool.status, Status::Unset);
    f.mapper.shutdown();
    assert_eq!(f.named("cyber.tool").len(), 1, "the span ends once");
    assert_eq!(
        f.total("cyber.tool.calls", &[("outcome", "interrupted")]),
        1.0
    );
}

#[test]
fn hook_spans_use_receipt_times_under_their_tool_call() {
    let mut f = fixture(Content::new(true, []));
    f.feed([
        created("ses_a", None, None, None),
        promoted("ses_a", "msg_1"),
        step("ses_a"),
        called("ses_a", "call_1", "bash", json!({})),
        LiveEvent::HookReceipt {
            session_id: "ses_a".into(),
            receipt: receipt(Some("call_1"), None),
        },
        LiveEvent::HookReceipt {
            session_id: "ses_a".into(),
            receipt: receipt(Some("call_1"), Some(HookOutcome::Blocked)),
        },
        settled("ses_a", "call_1", "error"),
        idle("ses_a"),
    ]);
    let hook = f.span("cyber.hook");
    assert!(is_child(&hook, &f.span("cyber.tool")));
    assert_eq!(
        attr_text(&hook, "cyber.hook.outcome").as_deref(),
        Some("blocked")
    );
    assert_eq!(
        attr_text(&hook, "cyber.hook.decision").as_deref(),
        Some("deny")
    );
    assert_eq!(
        attr_text(&hook, "cyber.hook.scope").as_deref(),
        Some("global")
    );
    let started = std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_700_000_000_000);
    assert_eq!(hook.start_time, started);
    assert_eq!(
        hook.end_time,
        started + std::time::Duration::from_millis(250)
    );
}

#[test]
fn log_prompts_exports_redacted_content() {
    let header = "hdr-secret-value";
    let mut f = fixture(Content::new(true, [header.to_string()]));
    f.feed([
        created("ses_a", None, None, None),
        admitted("ses_a", "msg_1", "use ghp_abcdefghijklmnop please"),
        promoted("ses_a", "msg_1"),
        step("ses_a"),
        called(
            "ses_a",
            "call_1",
            "bash",
            json!({"command": format!("curl -H 'x-key: {header}' sk-abcdefghijklmnop"),
                "authorization": "Bearer abc"}),
        ),
        settled("ses_a", "call_1", "ok"),
        durable(
            "ses_a",
            "session.text.ended.1",
            json!({"message_id": "msg_a", "text": "done with password=hunter2"}),
        ),
        idle("ses_a"),
    ]);
    let turn = f.span("cyber.turn");
    assert_eq!(
        attr_text(&turn, "cyber.prompt").as_deref(),
        Some("use *** please")
    );
    assert_eq!(
        attr_text(&turn, "cyber.response").as_deref(),
        Some("done with ***")
    );
    let input: Value =
        serde_json::from_str(&attr_text(&f.span("cyber.tool"), "cyber.tool.input").unwrap())
            .unwrap();
    assert_eq!(input["command"], "curl -H 'x-key: ***' ***");
    assert_eq!(input["authorization"], "***");
    let session = f.span("cyber.session");
    assert_eq!(
        attr_text(&session, "cyber.session.title").as_deref(),
        Some("Fix the build")
    );
    let everything = format!("{:?}", f.spans());
    for secret in [
        header,
        "ghp_abcdefghijklmnop",
        "sk-abcdefghijklmnop",
        "hunter2",
    ] {
        assert!(!everything.contains(secret), "{secret}");
    }
}

/// Regression: titles, compactions and auto-mode reviews bill model calls outside a
/// step, and the usage ledger counts them; so do the token and cost metrics, with the
/// event's model (else the Session's) split as the ledger splits it.
#[test]
fn calls_billed_outside_steps_count_tokens_and_cost() {
    let usage = |input: u64, output: u64| json!({"input": input, "output": output, "reasoning": 0, "cache_read": 0, "cache_write": 0});
    let mut f = fixture(Content::default());
    f.feed([
        created("ses_a", None, None, None),
        durable(
            "ses_a",
            "session.title.generated.1",
            json!({"title": "Fix", "usage": usage(11, 2), "cost": 0.01, "model": "test/title"}),
        ),
        // A title without a model call (the Session's first words) bills nothing.
        durable(
            "ses_a",
            "session.title.generated.1",
            json!({"title": "Fix it"}),
        ),
        durable(
            "ses_a",
            "session.compaction.completed.1",
            json!({"message_id": "msg_s", "summary": "s", "tail_start_id": "", "tokens_before": 9,
                "tokens_after": 1, "trigger": "manual", "usage": usage(7, 3), "cost": 0.03}),
        ),
        durable(
            "ses_a",
            "permission.auto_decided.1",
            json!({"call_id": "call_1", "action": "bash", "decision": "allow", "reason": "ok",
                "evaluator": "test/judge", "usage": usage(5, 1), "cost": 0.002}),
        ),
    ]);
    let tokens = |model: &str, class: &str| {
        f.total(
            "cyber.token.usage",
            &[
                ("cyber.model", model),
                ("cyber.provider", "test"),
                ("type", class),
            ],
        )
    };
    assert_eq!(
        (tokens("title", "input"), tokens("title", "output")),
        (11.0, 2.0)
    );
    // No model on the compaction: the Session's `test/main`.
    assert_eq!(
        (tokens("main", "input"), tokens("main", "output")),
        (7.0, 3.0)
    );
    assert_eq!(
        (tokens("judge", "input"), tokens("judge", "output")),
        (5.0, 1.0)
    );
    let cost = |model: &str| f.total("cyber.cost.usage", &[("cyber.model", model)]);
    assert_eq!(cost("title"), 0.01);
    assert_eq!(cost("main"), 0.03);
    assert_eq!(cost("judge"), 0.002);
    assert_eq!(f.total("cyber.token.usage", &[]), 29.0);
    // The review is still counted as a decision.
    assert_eq!(
        f.total(
            "cyber.permission.decisions",
            &[("decision", "allow"), ("source", "auto")]
        ),
        1.0
    );
}

/// Regression: a reply the runtime records when it withdraws a request (a stop, an
/// abandoned call) is nobody's decision: no `deny/user` count and no span event.
#[test]
fn a_withdrawn_request_is_not_a_user_denial() {
    let mut f = fixture(Content::default());
    f.feed([
        created("ses_a", None, None, None),
        promoted("ses_a", "msg_1"),
        step("ses_a"),
        called("ses_a", "call_1", "bash", json!({})),
        durable(
            "ses_a",
            "permission.asked.1",
            json!({"id": "per_1", "call_id": "call_1", "action": "bash"}),
        ),
        durable(
            "ses_a",
            "permission.replied.1",
            json!({"request_id": "per_1", "call_id": "call_1", "cancelled": true,
                "reply": {"reply": "reject", "message": null}}),
        ),
        settled("ses_a", "call_1", "interrupted"),
        idle("ses_a"),
    ]);
    assert_eq!(f.total("cyber.permission.decisions", &[]), 0.0);
    assert!(f.span("cyber.tool").events.is_empty());
}

fn hook_receipt(id: &str, event: &str, decision: Option<HookAction>, settled: bool) -> LiveEvent {
    LiveEvent::HookReceipt {
        session_id: "ses_a".into(),
        receipt: HookReceipt {
            id: id.into(),
            event: event.into(),
            call_id: None,
            tool_name: None,
            duration_ms: settled.then_some(5),
            outcome: settled.then_some(HookOutcome::Ok),
            decision: decision.filter(|_| settled),
            ..receipt(None, None)
        },
    }
}

/// Regression: `UserPromptSubmit` (at admission) and `SessionStart` (before the first
/// promotion) run while the Session has no activity period: they open its session span,
/// from the first receipt's start, and the Turn that follows joins it.
#[test]
fn pre_turn_hooks_are_under_the_session_span_the_turn_joins() {
    let mut f = fixture(Content::default());
    f.feed([
        created("ses_a", None, None, None),
        hook_receipt("hke_1", "UserPromptSubmit", None, false),
        hook_receipt("hke_1", "UserPromptSubmit", Some(HookAction::Allow), true),
        admitted("ses_a", "msg_1", "hi"),
        hook_receipt("hke_2", "SessionStart", None, false),
        hook_receipt("hke_2", "SessionStart", None, true),
        promoted("ses_a", "msg_1"),
        step("ses_a"),
        step_ended("ses_a", None),
        idle("ses_a"),
    ]);
    let sessions = f.named("cyber.session");
    assert_eq!(sessions.len(), 1, "one activity period");
    let session = &sessions[0];
    assert_eq!(session.parent_span_id, SpanId::INVALID);
    let hooks = f.named("cyber.hook");
    assert_eq!(hooks.len(), 2);
    for hook in &hooks {
        assert!(
            is_child(hook, session),
            "{:?}",
            attr_text(hook, "cyber.hook.event")
        );
    }
    assert!(is_child(&f.span("cyber.turn"), session));
    let started = std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_700_000_000_000);
    assert_eq!(session.start_time, started, "from the first hook's start");
}

/// A hook that opened the session span and leads to no Turn (a denied prompt, a
/// configuration hook) ends it with itself, so no span stays open.
#[test]
fn a_hook_without_a_following_turn_ends_the_span_it_opened() {
    let mut f = fixture(Content::default());
    f.feed([
        created("ses_a", None, None, None),
        hook_receipt("hke_1", "UserPromptSubmit", None, false),
        hook_receipt("hke_1", "UserPromptSubmit", Some(HookAction::Deny), true),
        hook_receipt("hke_2", "ConfigChange", None, false),
        hook_receipt("hke_2", "ConfigChange", None, true),
    ]);
    let sessions = f.named("cyber.session");
    assert_eq!(sessions.len(), 2, "both periods ended");
    let hooks = f.named("cyber.hook");
    assert!(is_child(&hooks[0], &sessions[0]));
    assert!(is_child(&hooks[1], &sessions[1]));
    f.mapper.shutdown();
    assert_eq!(f.named("cyber.session").len(), 2, "nothing was left open");
}

#[test]
fn opaque_reasoning_state_never_reaches_a_signal() {
    let mut f = fixture(Content::new(true, []));
    let opaque = json!({"provider": "openai", "model": "gpt-6-sol",
        "kind": "encrypted_reasoning", "data": "gAAAA-opaque-blob"});
    f.feed([
        created("ses_a", None, None, None),
        admitted("ses_a", "msg_1", "go"),
        promoted("ses_a", "msg_1"),
        step("ses_a"),
        durable(
            "ses_a",
            "session.reasoning.ended.1",
            json!({"message_id": "msg_a", "text": "plan", "index": 0, "opaque": opaque}),
        ),
        durable(
            "ses_a",
            "session.tool.called.1",
            json!({"message_id": "msg_a", "call_id": "call_1", "name": "read",
                "arguments": "{}", "input": {}, "opaque": opaque, "retry_safety": "read_only"}),
        ),
        settled("ses_a", "call_1", "ok"),
        step_ended("ses_a", None),
        idle("ses_a"),
    ]);
    assert!(f.spans().iter().any(|span| span.name == "cyber.tool"));
    assert!(!format!("{:?}", f.spans()).contains("gAAAA-opaque-blob"));
}
