//! A mode switch during inference must wait for the next Turn, including its tools.
mod support;

use cyber_llm::{Adapter, EventStream, LlmError, LlmRequest};
use cyber_server::runtime::*;
use futures::future::BoxFuture;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use support::*;
use tokio::sync::Notify;

struct GatedAdapter {
    base: Arc<dyn Adapter>,
    first: AtomicBool,
    started: Notify,
    release: Notify,
}

impl Adapter for GatedAdapter {
    fn stream(&self, request: LlmRequest) -> BoxFuture<'_, Result<EventStream, LlmError>> {
        Box::pin(async move {
            if self.first.swap(false, Ordering::SeqCst) {
                self.started.notify_one();
                self.release.notified().await;
            }
            self.base.stream(request).await
        })
    }
}

struct GatedModels {
    base: Arc<Models>,
    gate: Arc<GatedAdapter>,
}

impl ModelResolver for GatedModels {
    fn resolve(&self, model: &str) -> Result<ResolvedModel, String> {
        let mut resolved = self.base.resolve(model)?;
        if model == "test/main" {
            resolved.adapter = self.gate.clone();
        }
        Ok(resolved)
    }
    fn role(&self, role: cyber_llm::catalog::ModelRole) -> Option<String> {
        self.base.role(role)
    }
}

#[tokio::test]
async fn switch_during_inference_preserves_the_current_turn_tool_mode() {
    let mut h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("c0", "clock", "{}")]),
                tools(&[("c1", "read", "{}")]),
                text("done"),
            ],
        )],
        ..Setup::default()
    });
    let gate = Arc::new(GatedAdapter {
        base: h.models.adapter("test/main"),
        first: AtomicBool::new(true),
        started: Notify::new(),
        release: Notify::new(),
    });
    h.runtime = Runtime::new(RuntimeOptions {
        store: h.store.clone(),
        resolver: Arc::new(GatedModels {
            base: h.models.clone(),
            gate: gate.clone(),
        }),
        tools: h.tools.clone(),
        global_config_dir: h.dir.path().join("global"),
        shell: "bash".into(),
        claude_compat: false,
        compaction: CompactionConfig::default(),
        retry: cyber_llm::RetryPolicy::default(),
        max_steps: None,
        today: Some("2026-10-05".into()),
        interactive: true,
        snapshots: Arc::new(NoSnapshots),
    });
    h.tools.set("clock", Behavior::Gated);
    let id = h.session().await;
    h.runtime
        .admit(&id, Admission::text("inspect", Delivery::Steer))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), gate.started.notified())
        .await
        .unwrap();
    h.runtime.switch_mode(&id, "plan").await.unwrap();
    let state = h.state(&id).await;
    assert_eq!(state.effective_mode(true), "default");
    assert_eq!(state.pending_mode(true), Some("plan"));
    gate.release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), h.tools.started.notified())
        .await
        .unwrap();
    let rows = h.store.read_events(&id, -1, 100).unwrap().events;
    let replayed = SessionState::replay(&rows).unwrap();
    assert!(
        replayed.open_step.is_none(),
        "inference ended, but tools still belong to this Turn"
    );
    assert_eq!(replayed.effective_mode(true), "default");
    assert_eq!(replayed.pending_mode(true), Some("plan"));
    assert!(rows.iter().any(|row| row.kind == "session.step.started.1"));
    let mut legacy = rows.clone();
    for row in &mut legacy {
        if row.kind == "session.step.started.1" {
            row.data.as_object_mut().unwrap().remove("mode");
        }
    }
    assert_eq!(
        SessionState::replay(&legacy).unwrap().effective_mode(true),
        "default",
        "legacy start events use their historical selection"
    );
    h.tools.release.notify_one();
    h.settle(&id).await;
    let state = h.state(&id).await;
    assert_eq!(state.effective_mode(false), "plan");
    assert_eq!(state.pending_mode(false), None);
    let modes: Vec<_> = h
        .tools
        .executed
        .lock()
        .unwrap()
        .iter()
        .map(|i| (i.call_id.clone(), i.mode.clone()))
        .collect();
    assert_eq!(
        modes,
        vec![
            ("c0".into(), "default".into()),
            ("c1".into(), "plan".into())
        ]
    );
}

#[tokio::test]
async fn agent_pinning_preserves_tool_identity_until_the_next_turn() {
    let mut h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("c0", "clock", "{}")]),
                tools(&[("c1", "read", "{}")]),
                text("done"),
            ],
        )],
        ..Setup::default()
    });
    let gate = Arc::new(GatedAdapter {
        base: h.models.adapter("test/main"),
        first: AtomicBool::new(true),
        started: Notify::new(),
        release: Notify::new(),
    });
    h.runtime = Runtime::new(RuntimeOptions {
        store: h.store.clone(),
        resolver: Arc::new(GatedModels {
            base: h.models.clone(),
            gate: gate.clone(),
        }),
        tools: h.tools.clone(),
        global_config_dir: h.dir.path().join("global"),
        shell: "bash".into(),
        claude_compat: false,
        compaction: CompactionConfig::default(),
        retry: cyber_llm::RetryPolicy::default(),
        max_steps: None,
        today: None,
        interactive: true,
        snapshots: Arc::new(NoSnapshots),
    });
    h.tools.set("clock", Behavior::Gated);
    let id = h.session().await;
    h.runtime
        .admit(&id, Admission::text("inspect", Delivery::Steer))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), gate.started.notified())
        .await
        .unwrap();
    h.runtime.switch_agent(&id, "docs").await.unwrap();
    gate.release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), h.tools.started.notified())
        .await
        .unwrap();
    let first_agent = h.tools.executed.lock().unwrap()[0].agent.clone();
    let rows = h.store.read_events(&id, -1, 100).unwrap().events;
    let replayed = SessionState::replay(&rows).unwrap();
    assert!(
        replayed.open_step.is_none(),
        "identity survives inference ending before tool settlement"
    );
    assert_eq!(replayed.effective_agent(true), "build");
    assert_eq!(replayed.effective_agent(false), "docs");
    let mut legacy = rows.clone();
    for row in &mut legacy {
        if row.kind == "session.step.started.1" {
            row.data.as_object_mut().unwrap().remove("agent");
        }
    }
    assert_eq!(
        SessionState::replay(&legacy).unwrap().effective_agent(true),
        "build"
    );
    h.tools.release.notify_one();
    h.settle(&id).await;
    assert_eq!(
        first_agent, "build",
        "switching during inference cannot replace the tool's agent"
    );
    let agents: Vec<_> = h
        .tools
        .executed
        .lock()
        .unwrap()
        .iter()
        .map(|call| (call.call_id.clone(), call.agent.clone()))
        .collect();
    assert_eq!(
        agents,
        vec![("c0".into(), "build".into()), ("c1".into(), "docs".into())]
    );
}

struct ProfileHost(Arc<Tools>);

impl ToolHost for ProfileHost {
    fn request_overlay(
        &self,
        turn: &TurnContext,
    ) -> Result<cyber_llm::catalog::RequestOverlay, String> {
        Ok(cyber_llm::catalog::RequestOverlay::from_config(Some(
            &serde_json::json!({
                "body":{"profile":turn.agent,"mode":turn.mode}
            }),
        )))
    }
    fn definitions(&self, turn: &TurnContext) -> Vec<ToolDef> {
        self.0.definitions(turn)
    }
    fn execute(
        &self,
        call: Invocation,
        cancel: tokio_util::sync::CancellationToken,
    ) -> BoxFuture<'_, ToolOutcome> {
        self.0.execute(call, cancel)
    }
}

struct PreparationSnapshots {
    calls: std::sync::atomic::AtomicUsize,
    first: AtomicBool,
    started: Notify,
    release: Notify,
}

impl Snapshots for PreparationSnapshots {
    fn track(&self, _: &str) -> BoxFuture<'_, Result<Option<Snapshot>, String>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.first.swap(false, Ordering::SeqCst) {
                self.started.notify_one();
                self.release.notified().await;
            }
            Ok(None)
        })
    }
    fn changed(&self, _: &str, _: &str, _: &str) -> BoxFuture<'_, Result<Vec<String>, String>> {
        Box::pin(async { unreachable!("no snapshot") })
    }
    fn diff(&self, _: &str, _: &str, _: &str) -> BoxFuture<'_, Result<Vec<FileDiff>, String>> {
        Box::pin(async { unreachable!("no snapshot") })
    }
    fn restore(
        &self,
        _: &str,
        _: &str,
        _: &str,
    ) -> BoxFuture<'_, Result<Vec<String>, RestoreError>> {
        Box::pin(async { unreachable!("no rewind") })
    }
}

#[tokio::test]
async fn selection_change_during_snapshot_preparation_rebuilds_the_request() {
    for change in ["agent", "model", "mode", "round_trip"] {
        check_preparation_change(change).await;
    }
}

async fn check_preparation_change(change: &str) {
    let script = vec![tools(&[("c0", "clock", "{}")]), text("done")];
    let mut h = Harness::new(Setup {
        scripts: vec![("test/main", script.clone()), ("other/main", script)],
        ..Setup::default()
    });
    let snapshots = Arc::new(PreparationSnapshots {
        calls: std::sync::atomic::AtomicUsize::new(0),
        first: AtomicBool::new(true),
        started: Notify::new(),
        release: Notify::new(),
    });
    h.runtime = Runtime::new(RuntimeOptions {
        store: h.store.clone(),
        resolver: h.models.clone(),
        tools: Arc::new(ProfileHost(h.tools.clone())),
        global_config_dir: h.dir.path().join("global"),
        shell: "bash".into(),
        claude_compat: false,
        compaction: CompactionConfig::default(),
        retry: cyber_llm::RetryPolicy::default(),
        max_steps: None,
        today: None,
        interactive: true,
        snapshots: snapshots.clone(),
    });
    let id = h.session().await;
    h.runtime
        .admit(&id, Admission::text("inspect", Delivery::Queue))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), snapshots.started.notified())
        .await
        .unwrap();
    match change {
        "agent" => h.runtime.switch_agent(&id, "docs").await.unwrap(),
        "model" => h.runtime.switch_model(&id, "other/main").await.unwrap(),
        "mode" => h.runtime.switch_mode(&id, "plan").await.unwrap(),
        "round_trip" => {
            h.runtime.switch_agent(&id, "docs").await.unwrap();
            h.runtime.switch_agent(&id, "build").await.unwrap();
        }
        _ => unreachable!(),
    }
    snapshots.release.notify_one();
    h.settle(&id).await;
    assert_prepared_request(&h, &snapshots, change);
    assert_preparation_events(&h, &id, change);
}

fn assert_prepared_request(h: &Harness, snapshots: &PreparationSnapshots, change: &str) {
    let model = if change == "model" {
        "other/main"
    } else {
        "test/main"
    };
    let agent = if change == "agent" { "docs" } else { "build" };
    let mode = if change == "mode" { "plan" } else { "default" };
    let requests = h.models.requests(model);
    assert_eq!(requests.len(), 2, "{change}");
    assert_eq!(requests[0].body["profile"], agent, "{change}");
    assert_eq!(requests[0].body["mode"], mode, "{change}");
    let executed = h.tools.executed.lock().unwrap();
    assert_eq!(executed[0].agent, agent, "{change}");
    assert_eq!(executed[0].mode, mode, "{change}");
    assert_eq!(
        snapshots.calls.load(Ordering::SeqCst),
        5,
        "discarded preparation has no post-Turn snapshot: {change}"
    );
}

fn assert_preparation_events(h: &Harness, id: &str, change: &str) {
    let rows = h.store.read_events(id, -1, 500).unwrap().events;
    assert_eq!(
        rows.iter()
            .filter(|r| r.kind == "session.step.started.1")
            .count(),
        2,
        "{change}"
    );
    assert_eq!(
        rows.iter()
            .filter(|r| r.kind == "session.prompt.promoted.1")
            .count(),
        1,
        "{change}"
    );
}
