//! Budget refusals racing model preparation and retries retain their runtime error kind.
mod support;
use cyber_core::budget::Budget;
use cyber_llm::catalog::ModelRole;
use cyber_llm::{Adapter, ErrorKind, EventStream, LlmError, LlmRequest, RetryPolicy};
use cyber_server::runtime::*;
use cyber_store::{Expected, NewEvent, Store};
use futures::future::BoxFuture;
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use support::{Harness, Setup, text};

fn charge(store: &Store, child: &str) {
    store.append(child,Expected::Any,vec![NewEvent::new("session.step.ended.1",json!({"message_id":"msg_parallel_charge","finish":"stop","usage":{"input":1,"output":0,"reasoning":0,"cache_read":0,"cache_write":0},"cost":0.6}))]).unwrap();
}
struct RetryCharge {
    store: Arc<Store>,
    child: String,
    calls: Arc<AtomicUsize>,
}
impl Adapter for RetryCharge {
    fn stream(&self, _: LlmRequest) -> BoxFuture<'_, Result<EventStream, LlmError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            charge(&self.store, &self.child);
            Err(LlmError::new(
                ErrorKind::RateLimit,
                "rate limited before output",
            ))
        })
    }
}
enum DuringResolve {
    Charge(String),
    CorruptBudget(String),
}
struct Models {
    original: Arc<support::Models>,
    store: Arc<Store>,
    charge_on_resolve: Mutex<Option<DuringResolve>>,
    retry: Option<Arc<RetryCharge>>,
}
impl ModelResolver for Models {
    fn resolve(&self, reference: &str) -> Result<ResolvedModel, String> {
        if reference == "test/summary"
            && let Some(child) = self.charge_on_resolve.lock().unwrap().take()
        {
            match child {
                DuringResolve::Charge(child) => charge(&self.store, &child),
                DuringResolve::CorruptBudget(scope) => {
                    self.store
                        .transaction(move |tx| {
                            tx.execute(
                                "UPDATE session_budget SET spec='invalid json' WHERE session_id=?1",
                                [scope],
                            )?;
                            Ok(())
                        })
                        .unwrap();
                }
            }
        }
        let mut resolved = self.original.resolve(reference)?;
        if reference == "test/main"
            && let Some(retry) = &self.retry
        {
            resolved.adapter = retry.clone();
        }
        Ok(resolved)
    }
    fn role(&self, role: ModelRole) -> Option<String> {
        self.original.role(role)
    }
}
fn runtime(h: &Harness, models: Models) -> Runtime {
    Runtime::new(RuntimeOptions {
        store: h.store.clone(),
        resolver: Arc::new(models),
        tools: h.tools.clone(),
        global_config_dir: h.dir.path().join("config"),
        shell: "bash".into(),
        claude_compat: false,
        compaction: CompactionConfig {
            auto: false,
            keep_tokens: 10,
            keep_turns: Some(1),
            ..Default::default()
        },
        retry: RetryPolicy {
            base_delay: std::time::Duration::from_millis(1),
            ..Default::default()
        },
        max_steps: None,
        today: None,
        interactive: true,
        snapshots: Arc::new(NoSnapshots),
    })
}
async fn fixture() -> (Harness, String, String) {
    let h = Harness::new(Setup {
        scripts: vec![
            ("test/main", vec![text("first"), text("second")]),
            ("test/summary", vec![text("summary")]),
        ],
        roles: vec![("compaction", "test/summary")],
        ..Default::default()
    });
    let root = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            title: Some("Budget race".into()),
            budget: Some(Budget {
                max_cost_usd: Some(0.5),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    for _ in 0..2 {
        h.runtime
            .admit(&root, Admission::text("continue", Delivery::Steer))
            .await
            .unwrap();
        h.settle(&root).await;
    }
    let child = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(root.clone()),
            title: Some("Parallel billing".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    (h, root, child)
}
#[tokio::test]
async fn compaction_budget_exhaustion_during_model_resolution_is_typed() {
    let (h, root, child) = fixture().await;
    let runtime = runtime(
        &h,
        Models {
            original: h.models.clone(),
            store: h.store.clone(),
            charge_on_resolve: Mutex::new(Some(DuringResolve::Charge(child))),
            retry: None,
        },
    );
    let error = runtime.compact(&root, None).await.unwrap_err();
    assert!(
        matches!(error,RuntimeError::BudgetExceeded {ref scope,ref limit} if scope==&root && limit=="max_cost_usd"),
        "{error:?}"
    );
    assert!(h.models.requests("test/summary").is_empty());
    let events = h.store.read_events(&root, -1, 100).unwrap().events;
    assert!(events.iter().any(|e| {
        e.kind == "session.compaction.failed.1"
            && e.data["error"]
                .as_str()
                .unwrap()
                .contains("BudgetExceededError")
    }));
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "budget.exceeded.1")
            .count(),
        1
    );
}
#[tokio::test]
async fn retry_budget_refusal_settles_the_step_and_publishes_the_budget_kind_once() {
    let (h, root, child) = fixture().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let runtime = runtime(
        &h,
        Models {
            original: h.models.clone(),
            store: h.store.clone(),
            charge_on_resolve: Mutex::new(None),
            retry: Some(Arc::new(RetryCharge {
                store: h.store.clone(),
                child,
                calls: calls.clone(),
            })),
        },
    );
    let mut live = runtime.subscribe();
    runtime
        .admit(&root, Admission::text("new turn", Delivery::Steer))
        .await
        .unwrap();
    runtime.wait_idle(&root).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the retry cannot dispatch after parallel spend"
    );
    let mut errors = Vec::new();
    while let Ok(event) = live.try_recv() {
        if let LiveEvent::Error { kind, .. } = event {
            errors.push(kind)
        }
    }
    assert_eq!(errors, vec!["budget_exceeded"]);
    assert!(runtime.state(&root).await.unwrap().open_step.is_none());
    let failed = h
        .store
        .read_events(&root, -1, 100)
        .unwrap()
        .events
        .into_iter()
        .find(|e| e.kind == "session.step.failed.1")
        .unwrap();
    assert_eq!(failed.data["kind"], "budget_exceeded");
}

#[tokio::test]
async fn compaction_preparation_storage_failure_keeps_its_runtime_classification() {
    let (h, root, _) = fixture().await;
    let runtime = runtime(
        &h,
        Models {
            original: h.models.clone(),
            store: h.store.clone(),
            charge_on_resolve: Mutex::new(Some(DuringResolve::CorruptBudget(root.clone()))),
            retry: None,
        },
    );
    let error = runtime.compact(&root, None).await.unwrap_err();
    assert!(
        matches!(
            error,
            RuntimeError::Store(cyber_store::StoreError::Projector { .. })
        ),
        "{error:?}"
    );
    assert!(h.models.requests("test/summary").is_empty());
    let failure = h
        .store
        .read_events(&root, -1, 100)
        .unwrap()
        .events
        .into_iter()
        .find(|e| e.kind == "session.compaction.failed.1")
        .unwrap();
    assert!(
        failure.data["error"]
            .as_str()
            .unwrap()
            .contains("projector failed")
    );
}
