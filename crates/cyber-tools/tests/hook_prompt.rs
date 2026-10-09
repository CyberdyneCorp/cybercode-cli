//! Evaluator decisions, no-tools boundaries and metering through durable hook owners.
mod support;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cyber_core::config::{self, LoadRequest, Resolved};
use cyber_core::hooks::{HookAction, HookEvent, HookLocation, HookOutcome};
use cyber_core::paths::Paths;
use cyber_core::trust::TrustStore;
use cyber_llm::adapters::{ScriptStep, ScriptedAdapter};
use cyber_llm::catalog::{Cost, ModelRole};
use cyber_llm::{Adapter, FinishReason, LlmEvent, LlmRequest, Usage};
use cyber_server::runtime::{HookExecutionStatus, ModelResolver, ResolvedModel};
use cyber_tools::hook_prompt::HookPromptRunner;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

const POINTER: &str = "/hooks/PreToolUse/0/hooks/0";
struct Models {
    adapter: Arc<dyn Adapter>,
    evaluator: bool,
    resolved: Mutex<Vec<String>>,
}
impl ModelResolver for Models {
    fn role(&self, role: ModelRole) -> Option<String> {
        match role {
            ModelRole::Evaluator if self.evaluator => Some("test/evaluator".into()),
            ModelRole::Small => Some("test/small".into()),
            _ => None,
        }
    }
    fn resolve(&self, reference: &str) -> Result<ResolvedModel, String> {
        self.resolved.lock().unwrap().push(reference.into());
        Ok(ResolvedModel {
            adapter: self.adapter.clone(),
            provider: "test".into(),
            model: reference.into(),
            template: LlmRequest {
                model: reference.into(),
                body: json!({"tools":[{"name":"evil"}],"tool_choice":"required","max_tokens":99999,"messages":["spoofed"],"system":"spoofed"}),
                ..Default::default()
            },
            context_limit: 200000,
            prefers_apply_patch: false,
            cost: Some(Cost {
                input: 1.0,
                output: 2.0,
                ..Default::default()
            }),
        })
    }
}

struct Fixture {
    f: support::Fixture,
    resolved: Resolved,
    trust: TrustStore,
    event: HookEvent,
}
impl Fixture {
    fn new(handler: Value) -> Self {
        let f = support::Fixture::new();
        let env = HashMap::from([(
            "CYBER_HOME".into(),
            f.dir.path().join("config").display().to_string(),
        )]);
        let paths = Paths::resolve(&env, f.dir.path());
        paths.ensure().unwrap();
        std::fs::write(
            paths.config.join("cyber.jsonc"),
            json!({"telemetry":{"log_hook_io":false},"hooks":{"PreToolUse":[{"hooks":[handler]}]}})
                .to_string(),
        )
        .unwrap();
        let resolved = config::load(&LoadRequest {
            location: &f.repo,
            paths: &paths,
            env: &env,
            home: f.dir.path(),
            profile: None,
            overrides: &[],
            flags: json!({}),
        })
        .unwrap();
        let trust = TrustStore::new(paths.trust_file());
        let event = HookEvent::synthetic(
            "PreToolUse",
            HookLocation {
                directory: f.repo.clone(),
                workspace: None,
            },
            "global".into(),
            "build".into(),
            "default".into(),
            1,
            json!({"tool_name":"bash","tool_input":{"command":"rm /outside"}}),
        )
        .unwrap();
        Self {
            f,
            resolved,
            trust,
            event,
        }
    }
    fn runner(&self) -> HookPromptRunner<'_> {
        HookPromptRunner {
            resolved: &self.resolved,
            trust: &self.trust,
            invocation_trust: None,
            home: self.f.dir.path(),
        }
    }
    fn receipt(&self) -> Value {
        self.f
            .store
            .read(|db| {
                let raw: String =
                    db.query_row("SELECT data FROM hook_test_execution", [], |r| r.get(0))?;
                Ok(serde_json::from_str(&raw).unwrap())
            })
            .unwrap()
    }
}
fn handler() -> Value {
    json!({"type":"prompt","prompt":"Deny deletion outside the repository","id":"guard"})
}
fn script(text: &str) -> Vec<ScriptStep> {
    vec![
        ScriptStep::Event(LlmEvent::TextDelta { text: text.into() }),
        ScriptStep::Event(LlmEvent::Usage(Usage {
            input: 100,
            output: 10,
            ..Default::default()
        })),
        ScriptStep::Event(LlmEvent::Finish {
            reason: FinishReason::Stop,
        }),
    ]
}

struct Stalled(tokio::sync::Notify);
impl Adapter for Stalled {
    fn stream(
        &self,
        _request: LlmRequest,
    ) -> futures::future::BoxFuture<'_, Result<cyber_llm::adapters::EventStream, cyber_llm::LlmError>>
    {
        Box::pin(async move {
            self.0.notify_one();
            let items = futures::stream::iter([Ok(LlmEvent::Usage(Usage {
                input: 100,
                ..Default::default()
            }))]);
            Ok(
                Box::pin(futures::StreamExt::chain(items, futures::stream::pending()))
                    as cyber_llm::adapters::EventStream,
            )
        })
    }
}

fn runtime(f: &support::Fixture, models: Arc<Models>) -> cyber_server::runtime::Runtime {
    use cyber_server::runtime::*;
    Runtime::new(RuntimeOptions {
        store: f.store.clone(),
        resolver: models,
        tools: f.host.clone(),
        global_config_dir: f.dir.path().join("global"),
        shell: "bash".into(),
        claude_compat: false,
        compaction: CompactionConfig::default(),
        retry: Default::default(),
        max_steps: None,
        today: Some("2026-10-08".into()),
        interactive: false,
        snapshots: Arc::new(NoSnapshots),
    })
}

async fn session_event(
    runtime: &cyber_server::runtime::Runtime,
    f: &Fixture,
    budget: Option<cyber_core::budget::Budget>,
) -> HookEvent {
    let session = runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: f.f.repo.display().to_string(),
            model: "test/main".into(),
            mode: Some("bypass".into()),
            budget,
            ..Default::default()
        })
        .await
        .unwrap();
    HookEvent::new(
        "PreToolUse",
        cyber_core::hooks::HookIdentity {
            session_id: session.id,
            location: HookLocation {
                directory: f.f.repo.clone(),
                workspace: None,
            },
            project_id: "global".into(),
            agent: "build".into(),
            mode: "bypass".into(),
        },
        1,
        json!({"tool_name":"write","tool_input":{"path":"a"}})
            .as_object()
            .unwrap()
            .clone(),
    )
    .unwrap()
}

#[tokio::test]
async fn timeout_retains_unknown_provider_completion_and_fences_the_action() {
    for fail_closed in [false, true] {
        let mut f = Fixture::new(
            json!({"type":"prompt","prompt":"judge","timeout":1,"fail_closed":fail_closed}),
        );
        let adapter = Arc::new(Stalled(Default::default()));
        f.f.renew_host_with_models(
            None,
            Some(Arc::new(Models {
                adapter,
                evaluator: true,
                resolved: Mutex::default(),
            })),
        );
        let report = f
            .runner()
            .run_test(&f.f.host, POINTER, &f.event, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(report.outcome, HookOutcome::Timeout);
        assert_eq!(report.decision.decision, Some(HookAction::Deny));
        assert!(!report.acknowledged && report.must_stop);
        assert_eq!(f.receipt()["status"], "unknown");
    }
}

#[tokio::test]
async fn cancelled_recorded_inference_bills_observed_usage_and_retains_unknown() {
    let f = Fixture::new(handler());
    let adapter = Arc::new(Stalled(Default::default()));
    let models = Arc::new(Models {
        adapter: adapter.clone(),
        evaluator: true,
        resolved: Mutex::default(),
    });
    let runtime = runtime(&f.f, models);
    let event = session_event(&runtime, &f, None).await;
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let runner = f.runner();
    let execution = runner.run_recorded(&runtime, POINTER, &event, cancel);
    let interruption = async {
        adapter.0.notified().await;
        tokio::task::yield_now().await;
        stop.cancel();
    };
    let (report, ()) = tokio::join!(execution, interruption);
    let report = report.unwrap();
    assert!(!report.acknowledged && report.must_stop);
    let usage = runtime.session_usage(&event.identity().session_id).unwrap();
    assert_eq!(usage.own.tokens.input, 100);
    assert!((usage.own.cost - 0.0001).abs() < 1e-12);
    assert_eq!(
        runtime
            .hook_executions(&event.identity().session_id, 20)
            .unwrap()[0]
            .status,
        HookExecutionStatus::Unknown
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn budget_ceiling_refuses_inference_before_any_provider_call() {
    let f = Fixture::new(handler());
    let adapter = Arc::new(ScriptedAdapter::new(vec![script(
        r#"{"decision":"allow","reason":"safe"}"#,
    )]));
    let runtime = runtime(
        &f.f,
        Arc::new(Models {
            adapter: adapter.clone(),
            evaluator: true,
            resolved: Mutex::default(),
        }),
    );
    let budget: cyber_core::budget::Budget =
        serde_json::from_value(json!({"max_tokens":1,"enforcement":"soft"})).unwrap();
    let event = session_event(&runtime, &f, Some(budget)).await;
    let asker = runtime
        .operation_asker(&event.identity().session_id)
        .await
        .unwrap();
    asker
        .record_model_usage(cyber_server::runtime::AuxiliaryUsage {
            provider: "test".into(),
            model: "test/main".into(),
            purpose: "fixture".into(),
            call_id: None,
            duration_ms: 0,
            usage: Usage {
                input: 1,
                ..Default::default()
            },
            cost: Some(0.0),
        })
        .await
        .unwrap();
    let report = f
        .runner()
        .run_recorded(&runtime, POINTER, &event, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(report.outcome, HookOutcome::Error);
    assert!(report.must_stop && adapter.requests().is_empty());
    runtime.shutdown().await;
}

#[tokio::test]
async fn synthetic_evaluator_policy_and_flat_event_have_no_tools_or_session_effects() {
    let mut f = Fixture::new(handler());
    let adapter = Arc::new(ScriptedAdapter::new(vec![script(
        r#"{"decision":"deny","reason":"outside repository"}"#,
    )]));
    let models = Arc::new(Models {
        adapter: adapter.clone(),
        evaluator: true,
        resolved: Mutex::default(),
    });
    f.f.renew_host_with_models(None, Some(models.clone()));
    let report = f
        .runner()
        .run_test(&f.f.host, POINTER, &f.event, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(report.outcome, HookOutcome::Blocked);
    assert_eq!(
        report.decision.reason.as_deref(),
        Some("outside repository")
    );
    assert_eq!(*models.resolved.lock().unwrap(), ["test/evaluator"]);
    let request = &adapter.requests()[0];
    assert!(request.tools.is_empty() && request.tools_disabled);
    assert_eq!(request.max_output_tokens, Some(512));
    assert!(request.body.get("messages").is_none() && request.body.get("max_tokens").is_none());
    assert!(request.system[1].contains("Deny deletion outside"));
    let encoded = serde_json::to_value(&request.messages[0])
        .unwrap()
        .to_string();
    assert!(encoded.contains("synthetic") && encoded.contains("rm /outside"));
    assert!(f.receipt().get("io").is_none());
    let count: i64 =
        f.f.store
            .read(|db| Ok(db.query_row("SELECT count(*) FROM session", [], |r| r.get(0))?))
            .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn small_role_fallback_and_strict_structured_decisions() {
    for reply in [
        r#"{"decision":"ask","reason":"review"}"#,
        r#"{"decision":"allow","reason":"safe"}"#,
        r#"{"decision":"allow","reason":""}"#,
        r#"{"decision":"allow","reason":"safe","updated_input":{}}"#,
        r#"{"decision":"block","reason":"bad enum"}"#,
        "[]",
    ] {
        let mut f = Fixture::new(handler());
        let models = Arc::new(Models {
            adapter: Arc::new(ScriptedAdapter::new(vec![script(reply)])),
            evaluator: false,
            resolved: Mutex::default(),
        });
        f.f.renew_host_with_models(None, Some(models.clone()));
        let report = f
            .runner()
            .run_test(&f.f.host, POINTER, &f.event, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(*models.resolved.lock().unwrap(), ["test/small"]);
        assert!(report.acknowledged);
        if reply.contains("review") {
            assert_eq!(report.decision.decision, Some(HookAction::Ask));
        } else if reply == r#"{"decision":"allow","reason":"safe"}"# {
            assert_eq!(report.decision.decision, Some(HookAction::Allow));
        } else {
            assert_eq!(report.outcome, HookOutcome::Error);
            assert!(report.decision.decision.is_none());
        }
    }
}

#[tokio::test]
async fn missing_evaluator_is_durable_fail_closed_and_once_skips_repeat() {
    let f = Fixture::new(json!({"type":"prompt","prompt":"judge","fail_closed":true,"once":true}));
    let first = f
        .runner()
        .run_test(&f.f.host, POINTER, &f.event, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(first.decision.decision, Some(HookAction::Deny));
    let second = f
        .runner()
        .run_test(&f.f.host, POINTER, &f.event, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(second.outcome, HookOutcome::Skipped);
    assert_eq!(f.receipt()["status"], "completed");
}

#[tokio::test]
async fn recorded_prompt_usage_is_hidden_billed_and_does_not_authorize_tools() {
    let f = Fixture::new(handler());
    let flow = support::flow::Flow::with_models(
        f.f,
        Vec::new(),
        false,
        Arc::new(cyber_server::runtime::NoSnapshots),
        vec![(
            "test/summary",
            vec![script(r#"{"decision":"deny","reason":"policy"}"#)],
        )],
    );
    let session = flow.session("bypass").await;
    let event = HookEvent::new(
        "PreToolUse",
        cyber_core::hooks::HookIdentity {
            session_id: session.clone(),
            project_id: "global".into(),
            location: HookLocation {
                directory: flow.f.repo.clone(),
                workspace: None,
            },
            agent: "build".into(),
            mode: "bypass".into(),
        },
        1,
        json!({"tool_name":"write","tool_input":{"path":"a"}})
            .as_object()
            .unwrap()
            .clone(),
    )
    .unwrap();
    let runner = HookPromptRunner {
        resolved: &f.resolved,
        trust: &f.trust,
        invocation_trust: None,
        home: flow.f.dir.path(),
    };
    let report = runner
        .run_recorded(&flow.runtime, POINTER, &event, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(report.outcome, HookOutcome::Blocked);
    let usage = flow.runtime.session_usage(&session).unwrap();
    assert_eq!(usage.own.tokens.input, 100);
    assert_eq!(usage.own.tokens.output, 10);
    assert_eq!(usage.own.unpriced_steps, 1);
    let state = flow.runtime.state(&session).await.unwrap();
    assert!(state.steps.is_empty());
    assert!(flow.requests("test/summary")[0].tools_disabled);
    let records = flow.runtime.hook_executions(&session, 20).unwrap();
    assert_eq!(records[0].status, HookExecutionStatus::Completed);
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn prompt_policy_denies_a_real_builtin_write_and_bills_the_session() {
    let f = Fixture::new(handler());
    let loaded = f.resolved.clone();
    f.f.host
        .attach_hook_config(Arc::new(move |_| Ok(loaded.clone())), f.trust)
        .unwrap();
    let flow = support::flow::Flow::with_models(
        f.f,
        vec![
            support::flow::call(
                "call_write",
                "write",
                json!({"path":"a.txt","content":"unsafe"}),
            ),
            support::flow::text("done"),
        ],
        false,
        Arc::new(cyber_server::runtime::NoSnapshots),
        vec![(
            "test/summary",
            vec![script(r#"{"decision":"deny","reason":"hook policy"}"#)],
        )],
    );
    let session = flow.session("bypass").await;
    flow.prompt(&session, "write").await;
    flow.runtime.wait_idle(&session).await;
    assert!(!flow.f.repo.join("a.txt").exists());
    let receipts = flow.runtime.hook_executions(&session, 20).unwrap();
    assert_eq!(receipts[0].outcome, Some(HookOutcome::Blocked));
    assert!(
        flow.runtime
            .session_usage(&session)
            .unwrap()
            .own
            .tokens
            .input
            >= 100
    );
    assert_eq!(flow.requests("test/summary").len(), 1);
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn unexpected_tool_calls_incomplete_finish_and_response_overflow_never_authorize() {
    let tool = cyber_llm::ToolCall {
        id: "call_evil".into(),
        name: "write".into(),
        arguments: "{}".into(),
        input: Some(json!({})),
    };
    let variants = [
        vec![
            ScriptStep::Event(LlmEvent::ToolCallDone(tool)),
            ScriptStep::Event(LlmEvent::Finish {
                reason: FinishReason::Stop,
            }),
        ],
        vec![ScriptStep::Event(LlmEvent::TextDelta {
            text: r#"{"decision":"allow","reason":"safe"}"#.into(),
        })],
        vec![ScriptStep::Event(LlmEvent::TextDelta {
            text: "x".repeat(1024 * 1024 + 1),
        })],
    ];
    for script in variants {
        let mut f = Fixture::new(json!({"type":"prompt","prompt":"judge","fail_closed":true}));
        f.f.renew_host_with_models(
            None,
            Some(Arc::new(Models {
                adapter: Arc::new(ScriptedAdapter::new(vec![script])),
                evaluator: true,
                resolved: Mutex::default(),
            })),
        );
        let report = f
            .runner()
            .run_test(&f.f.host, POINTER, &f.event, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(report.outcome, HookOutcome::Error);
        assert_eq!(report.decision.decision, Some(HookAction::Deny));
    }
}

#[tokio::test]
async fn completed_provider_without_usage_is_unpriced_rather_than_free() {
    let f = Fixture::new(handler());
    let adapter = Arc::new(ScriptedAdapter::new(vec![vec![
        ScriptStep::Event(LlmEvent::TextDelta {
            text: r#"{"decision":"allow","reason":"safe"}"#.into(),
        }),
        ScriptStep::Event(LlmEvent::Finish {
            reason: FinishReason::Stop,
        }),
    ]]));
    let runtime = runtime(
        &f.f,
        Arc::new(Models {
            adapter,
            evaluator: true,
            resolved: Mutex::default(),
        }),
    );
    let event = session_event(&runtime, &f, None).await;
    let report = f
        .runner()
        .run_recorded(&runtime, POINTER, &event, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(report.outcome, HookOutcome::Ok);
    let usage = runtime.session_usage(&event.identity().session_id).unwrap();
    assert_eq!(usage.own.cost, 0.0);
    assert_eq!(usage.own.unpriced_steps, 1);
    runtime.shutdown().await;
}

type Served = tokio::task::JoinHandle<(String, Value)>;
async fn provider(
    status: u16,
    body: Vec<u8>,
    stall: bool,
) -> (String, tokio::sync::oneshot::Receiver<()>, Served) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (send, received) = tokio::sync::oneshot::channel();
    let served = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let (head, request) = provider_request(&mut socket).await;
        let length = if stall {
            String::new()
        } else {
            format!("Content-Length: {}\r\n", body.len())
        };
        socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Type: text/event-stream\r\n{length}Connection: close\r\n\r\n").as_bytes()).await.unwrap();
        let _ = socket.write_all(&body).await;
        let _ = send.send(());
        if stall {
            let mut byte = [0];
            let result =
                tokio::time::timeout(std::time::Duration::from_secs(5), socket.read(&mut byte))
                    .await
                    .unwrap();
            assert!(
                matches!(result, Ok(0) | Err(_)),
                "provider connection remained open"
            );
        }
        (head, request)
    });
    (format!("http://{address}/v1"), received, served)
}

async fn provider_request(socket: &mut tokio::net::TcpStream) -> (String, Value) {
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let n = socket.read(&mut chunk).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&bytes[..end]).into_owned();
            let length: usize = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|length| length.trim().parse().unwrap())
                })
                .unwrap();
            if bytes.len() >= end + 4 + length {
                return (
                    head,
                    serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap(),
                );
            }
        }
        assert!(bytes.len() <= 2 * 1024 * 1024);
    }
}

fn native(kind: cyber_llm::adapters::ApiKind, url: &str) -> Arc<Models> {
    let mut endpoint =
        cyber_llm::adapters::Endpoint::new(url, Some("private provider credential".into()));
    endpoint
        .headers
        .push(("x-custom-header".into(), "configured header".into()));
    Arc::new(Models {
        adapter: Arc::from(cyber_llm::adapters::adapter(kind, endpoint)),
        evaluator: true,
        resolved: Mutex::default(),
    })
}
fn protocols() -> [cyber_llm::adapters::ApiKind; 3] {
    use cyber_llm::adapters::ApiKind::*;
    [OpenaiCompatible, OpenaiResponses, Anthropic]
}

#[tokio::test]
async fn native_protocol_timeouts_close_provider_sockets_and_apply_fail_closed_policy() {
    for kind in protocols() {
        for fail_closed in [false, true] {
            let (url, _, served) = provider(200, b": keepalive\n\n".to_vec(), true).await;
            let mut f = Fixture::new(
                json!({"type":"prompt","prompt":"judge","timeout":1,"fail_closed":fail_closed}),
            );
            f.f.renew_host_with_models(None, Some(native(kind, &url)));
            let report = f
                .runner()
                .run_test(&f.f.host, POINTER, &f.event, CancellationToken::new())
                .await
                .unwrap();
            assert_eq!(report.outcome, HookOutcome::Timeout);
            assert!(report.acknowledged && !report.must_stop);
            assert_eq!(report.decision.decision.is_some(), fail_closed);
            let (head, body) = served.await.unwrap();
            assert!(
                head.contains("private provider credential") && head.contains("configured header")
            );
            assert!(body.get("tools").is_none());
            assert_eq!(f.receipt()["status"], "completed");
            assert!(
                !f.receipt()
                    .to_string()
                    .contains("private provider credential")
            );
        }
    }
}

#[tokio::test]
async fn native_subtree_stop_acknowledges_local_provider_transport_before_receipt() {
    for kind in protocols() {
        let (url, received, served) = provider(200, b": keepalive\n\n".to_vec(), true).await;
        let f = Fixture::new(handler());
        let runtime = runtime(&f.f, native(kind, &url));
        let event = session_event(&runtime, &f, None).await;
        let runner = f.runner();
        let mut running =
            Box::pin(runner.run_recorded(&runtime, POINTER, &event, CancellationToken::new()));
        tokio::select! { result = &mut running => panic!("request ended early: {result:?}"), result = received => result.unwrap() }
        let (report, stopped) =
            tokio::join!(running, runtime.stop_subtree(&event.identity().session_id));
        let report = report.unwrap();
        assert!(report.acknowledged && report.must_stop);
        assert_eq!(
            stopped.unwrap().status,
            cyber_server::runtime::SubtreeStopStatus::Acknowledged
        );
        served.await.unwrap();
        assert_eq!(
            runtime
                .hook_executions(&event.identity().session_id, 10)
                .unwrap()[0]
                .status,
            HookExecutionStatus::Completed
        );
        assert_eq!(
            runtime
                .session_usage(&event.identity().session_id)
                .unwrap()
                .own
                .unpriced_steps,
            1
        );
        runtime.shutdown().await;
    }
}

#[tokio::test]
async fn native_disposal_closes_socket_but_retains_unknown_receipt_without_acknowledgement() {
    let (url, received, served) = provider(200, b": keepalive\n\n".to_vec(), true).await;
    let mut f = Fixture::new(handler());
    f.f.renew_host_with_models(None, Some(native(protocols()[0], &url)));
    let runner = f.runner();
    let mut running =
        Box::pin(runner.run_test(&f.f.host, POINTER, &f.event, CancellationToken::new()));
    tokio::select! { result = &mut running => panic!("request ended early: {result:?}"), result = received => result.unwrap() }
    drop(running);
    served.await.unwrap();
    assert_eq!(f.receipt()["status"], "unknown");
    assert_eq!(f.receipt()["acknowledged"], false);
}

#[tokio::test]
async fn native_raw_sse_and_error_bodies_are_bounded_before_decoder_buffering() {
    for kind in protocols() {
        for status in [200, 500] {
            let (url, _, served) = provider(status, vec![b'x'; 1024 * 1024 + 1], false).await;
            let mut f = Fixture::new(handler());
            f.f.renew_host_with_models(None, Some(native(kind, &url)));
            let report = f
                .runner()
                .run_test(&f.f.host, POINTER, &f.event, CancellationToken::new())
                .await
                .unwrap();
            assert_eq!(report.outcome, HookOutcome::Error);
            assert!(report.acknowledged && !report.must_stop);
            assert!(report.decision.decision.is_none());
            served.await.unwrap();
        }
    }
}
