//! Real BuiltinHost HTTP-hook dispatch from the durable observation port. Native
//! actor lifecycle/external editor acceptance is covered separately as it lands.
mod support;
use cyber_core::hooks::file_observations::{self as records, Delta, Stamp, Update};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Recorder {
    seen: Arc<Mutex<Vec<Value>>>,
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Recorder {
    async fn start() -> Self {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        let router=axum::Router::new().route("/hook",axum::routing::post({
            let seen=Arc::clone(&seen);
            move |axum::Json(event):axum::Json<Value>| {
                let seen=Arc::clone(&seen);
                async move {
                    seen.lock().unwrap().push(event);
                    axum::Json(json!({"decision":"deny","reason":"observation cannot veto a committed write"}))
                }
            }
        }));
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self { seen, url, task }
    }
}
impl Drop for Recorder {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn committed_observation_dispatch_is_scoped_deduplicated_and_observational() {
    let flow = support::flow::Flow::new(vec![], false);
    let first = flow.session("default").await;
    let second = flow.session("default").await;
    let recorder = Recorder::start().await;
    flow.f.global_hooks(
        json!({"FileChanged":[{"matcher":"nested/*","paths":["nested/**"],"hooks":[{
            "type":"http","id":"observer","url":recorder.url,"fail_closed":true
        }]}]}),
    );
    std::fs::create_dir(flow.f.repo.join("nested")).unwrap();
    std::fs::write(flow.f.repo.join("nested/data"), "observed").unwrap();
    let mut watch = flow
        .runtime
        .claim_file_watch(
            flow.f.repo.display().to_string(),
            "fixture-root".into(),
            BTreeMap::new(),
        )
        .await
        .unwrap();
    let source = watch
        .record(Delta {
            generation: watch.state().generation.clone(),
            updates: vec![Update {
                path: "nested/data".into(),
                stamp: Some(Stamp {
                    length: 8,
                    modified_ns: None,
                    identity: None,
                }),
            }],
            remaining: records::FEEDBACK_LIMIT - 1,
            dispatch: true,
            renewal: None,
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        watch.dispatch_to(&source, &first).await.unwrap();
        watch.dispatch_to(&source, &first).await.unwrap();
        watch.dispatch_to(&source, &second).await.unwrap();
    })
    .await
    .expect("scoped dispatch must settle without holding a Session or lifecycle lock");
    let events = recorder.seen.lock().unwrap().clone();
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[0]["session_id"], first);
    assert_eq!(events[1]["session_id"], second);
    for event in &events {
        assert_eq!(event["event"], "FileChanged");
        assert_eq!(event["file_path"], "nested/data");
        assert_eq!(event["observation_id"], source.id);
        assert_eq!(event["watch_generation"], source.data["generation"]);
        assert_eq!(
            event["location"]["directory"],
            flow.f.repo.display().to_string()
        );
        assert!(event.get("writer").is_none());
    }
    for id in [&first, &second] {
        let receipts = flow.runtime.hook_executions(id, 10).unwrap();
        assert_eq!(receipts.len(), 1, "{receipts:?}");
        assert_eq!(receipts[0].acknowledged, Some(true));
        assert!(
            serde_json::to_value(&receipts[0])
                .unwrap()
                .get("file_observation_binding")
                .is_none()
        );
    }
    assert_eq!(
        std::fs::read_to_string(flow.f.repo.join("nested/data")).unwrap(),
        "observed"
    );
    assert!(
        flow.requests("test/main").is_empty(),
        "observations must not infer through a model"
    );
}
