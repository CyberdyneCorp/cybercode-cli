//! The model fallback chain through the real tool host and runtime (`provider-catalog` →
//! Model fallback chain): a Turn that fails before any output is re-sent on the next
//! model of `fallback_models`, which the Session keeps until `fallback.sticky_minutes`
//! pass or the user switches model.

mod support;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use cyber_core::config::{self, LoadRequest};
use cyber_core::paths::Paths;
use cyber_core::trust::TrustStore;
use cyber_llm::ErrorKind;
use cyber_llm::adapters::ScriptStep;
use cyber_server::runtime::{NoSnapshots, WaitClock};
use futures::future::BoxFuture;
use serde_json::Value;
use support::flow::{Flow, Scripts, text};

/// Resolve every model through the real tool host's configuration loader.
fn attach_config(flow: &Flow) {
    let root = flow.f.dir.path().to_path_buf();
    let env = HashMap::from([(
        "CYBER_HOME".into(),
        root.join("cyber").display().to_string(),
    )]);
    let paths = Paths::resolve(&env, &root);
    paths.ensure().unwrap();
    let trust = TrustStore::new(paths.trust_file());
    flow.f
        .host
        .attach_hook_config(
            Arc::new(move |location| {
                config::load(&LoadRequest {
                    location,
                    paths: &paths,
                    env: &env,
                    home: &root,
                    profile: None,
                    overrides: &[],
                    flags: serde_json::json!({}),
                })
                .map_err(|error| error.to_string())
            }),
            trust,
        )
        .unwrap();
}

/// The host clock moved forward by `offset_ms`.
#[derive(Default)]
struct Clock {
    offset_ms: AtomicI64,
}

impl WaitClock for Clock {
    fn now_ms(&self) -> i64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        i64::try_from(now).unwrap() + self.offset_ms.load(Ordering::SeqCst)
    }

    fn sleep(&self, _duration: Duration) -> BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(Duration::from_millis(1)))
    }
}

fn error(kind: ErrorKind, message: &str) -> Vec<ScriptStep> {
    vec![ScriptStep::Error {
        error: kind,
        message: message.into(),
    }]
}

/// A runtime with `test/main` and the extra models, the Location configured by `config`.
fn flow(main: Scripts, extra: Vec<(&'static str, Scripts)>, config: &str) -> (Flow, Arc<Clock>) {
    let flow = Flow::with_models(
        support::Fixture::new(),
        main,
        false,
        Arc::new(NoSnapshots),
        extra,
    );
    attach_config(&flow);
    // Global configuration: a checkout's own `fallback_models` is held until trusted.
    let global = flow
        .f
        .dir
        .path()
        .canonicalize()
        .unwrap()
        .join("cyber/config");
    std::fs::create_dir_all(&global).unwrap();
    std::fs::write(global.join("cyber.jsonc"), config).unwrap();
    let clock = Arc::new(Clock::default());
    flow.runtime
        .configure_wait_clock(Arc::clone(&clock) as Arc<dyn WaitClock>);
    (flow, clock)
}

/// The Session's fallback events, oldest first.
fn fallbacks(flow: &Flow, id: &str) -> Vec<Value> {
    flow.f
        .store
        .read_events(id, -1, 500)
        .unwrap()
        .events
        .into_iter()
        .filter(|e| e.kind == "session.model.fallback.1")
        .map(|e| e.data)
        .collect()
}

async fn answered(flow: &Flow, id: &str, prompt: &str) {
    flow.prompt(id, prompt).await;
    flow.settle(id).await;
}

const CHAIN: &str = r#"{ "fallback_models": ["test/backup", "test/spare"] }"#;

#[tokio::test]
async fn an_overloaded_model_falls_back_and_the_session_stays_there() {
    // The primary fails once and through the retry policy's four retries.
    let overloaded: Scripts = (0..5)
        .map(|_| error(ErrorKind::ProviderInternal, "529 overloaded"))
        .collect();
    let (flow, _) = flow(
        overloaded,
        vec![(
            "test/backup",
            vec![text("from backup"), text("still backup")],
        )],
        CHAIN,
    );
    let id = flow.session("default").await;
    answered(&flow, &id, "one").await;

    let recorded = fallbacks(&flow, &id);
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(recorded[0]["from"], "test/main");
    assert_eq!(recorded[0]["to"], "test/backup");
    assert_eq!(recorded[0]["kind"], "provider_internal");
    let state = flow.runtime.state(&id).await.unwrap();
    assert_eq!(state.info.model, "test/backup");
    let active = flow
        .runtime
        .model_fallback(&state)
        .expect("active fallback");
    assert_eq!(
        active.until_ms - active.started_ms,
        10 * 60_000,
        "sticky 10 minutes"
    );
    assert_eq!(flow.requests("test/backup").len(), 1);
    let last = state.entries.last().unwrap();
    assert!(format!("{last:?}").contains("from backup"), "{last:?}");

    // The next Turn stays on the fallback, without trying the primary again.
    let before = flow.requests("test/main").len();
    answered(&flow, &id, "two").await;
    assert_eq!(flow.requests("test/main").len(), before);
    assert_eq!(flow.requests("test/backup").len(), 2);
}

#[tokio::test]
async fn the_chain_moves_on_and_a_last_failure_stands() {
    let (flow, _) = flow(
        vec![error(ErrorKind::Authentication, "invalid key")],
        vec![
            (
                "test/backup",
                vec![error(ErrorKind::QuotaExceeded, "quota")],
            ),
            (
                "test/spare",
                vec![error(ErrorKind::Authentication, "spare key")],
            ),
        ],
        CHAIN,
    );
    let id = flow.session("default").await;
    answered(&flow, &id, "one").await;
    let recorded = fallbacks(&flow, &id);
    let hops: Vec<(&str, &str)> = recorded
        .iter()
        .map(|f| (f["from"].as_str().unwrap(), f["to"].as_str().unwrap()))
        .collect();
    assert_eq!(
        hops,
        [("test/main", "test/backup"), ("test/backup", "test/spare")]
    );
    assert_eq!(
        recorded[1]["kind"], "quota_exceeded",
        "before any usage limit wait"
    );
    assert_eq!(flow.requests("test/spare").len(), 1);
    let failures: Vec<Value> = flow
        .f
        .store
        .read_events(&id, -1, 500)
        .unwrap()
        .events
        .into_iter()
        .filter(|e| e.kind == "session.step.failed.1")
        .map(|e| e.data)
        .collect();
    assert_eq!(failures.len(), 3, "each model's failure is recorded");
    assert_eq!(failures[2]["message"], "spare key");
}

#[tokio::test]
async fn errors_that_another_model_would_repeat_do_not_fall_back() {
    let (flow, _) = flow(
        vec![error(ErrorKind::ContextOverflow, "too long"), text("ok")],
        vec![("test/backup", vec![text("never")])],
        CHAIN,
    );
    let id = flow.session("default").await;
    answered(&flow, &id, "one").await;
    assert!(fallbacks(&flow, &id).is_empty());
    assert!(flow.requests("test/backup").is_empty());
}

#[tokio::test]
async fn the_fallback_ends_when_it_expires_or_the_user_switches() {
    let (flow, clock) = flow(
        vec![
            error(ErrorKind::Authentication, "expired key"),
            text("primary again"),
            error(ErrorKind::Authentication, "expired key"),
        ],
        vec![(
            "test/backup",
            vec![text("backup"), text("backup two"), text("unused")],
        )],
        r#"{ "fallback_models": ["test/backup"], "fallback": { "sticky_minutes": 5 } }"#,
    );
    let id = flow.session("default").await;
    answered(&flow, &id, "one").await;
    assert_eq!(
        flow.runtime.state(&id).await.unwrap().info.model,
        "test/backup"
    );

    // Past five minutes the next Turn switches back to the configured model.
    clock.offset_ms.store(5 * 60_000 + 1, Ordering::SeqCst);
    answered(&flow, &id, "two").await;
    let state = flow.runtime.state(&id).await.unwrap();
    assert_eq!(state.info.model, "test/main");
    assert!(flow.runtime.model_fallback(&state).is_none());
    assert_eq!(flow.requests("test/main").len(), 2);

    // A user switch ends an active fallback too.
    answered(&flow, &id, "three").await;
    assert_eq!(
        flow.runtime.state(&id).await.unwrap().info.model,
        "test/backup"
    );
    flow.runtime.switch_model(&id, "test/main").await.unwrap();
    let state = flow.runtime.state(&id).await.unwrap();
    assert!(flow.runtime.model_fallback(&state).is_none());
    assert_eq!(state.info.model, "test/main");
}

#[tokio::test]
async fn without_a_chain_the_failure_stands() {
    let (flow, _) = flow(
        vec![error(ErrorKind::Authentication, "invalid key")],
        vec![("test/backup", vec![text("never")])],
        "{}",
    );
    let id = flow.session("default").await;
    answered(&flow, &id, "one").await;
    assert!(fallbacks(&flow, &id).is_empty());
    assert!(flow.requests("test/backup").is_empty());
    assert_eq!(
        flow.runtime.state(&id).await.unwrap().info.model,
        "test/main"
    );
}
