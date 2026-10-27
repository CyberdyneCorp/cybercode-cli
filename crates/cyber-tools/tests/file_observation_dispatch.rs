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
    // Isolate the manual journal/dispatch port from the automatic native owner.
    // The real Sessions stay live; only host watcher membership is detached.
    for session in [&first, &second] {
        cyber_server::runtime::ToolHost::end_session(&*flow.f.host, session).await;
    }
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
    let cancelled = tokio_util::sync::CancellationToken::new();
    cancelled.cancel();
    watch
        .dispatch_to_cancellable(&source, &first, cancelled)
        .await
        .unwrap();
    assert!(flow.runtime.hook_executions(&first, 10).unwrap().is_empty());
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

async fn await_condition(label: &str, predicate: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(45), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {label}"));
}
fn journal_count(flow: &support::flow::Flow, kind: &'static str) -> usize {
    flow.f
        .store
        .read(move |db| {
            Ok(
                db.query_row("SELECT COUNT(*) FROM event WHERE type=?1", [kind], |row| {
                    row.get(0)
                })?,
            )
        })
        .unwrap()
}
fn automatic_hooks(flow: &support::flow::Flow, recorder: &Recorder) {
    flow.f.global_hooks(json!({"FileChanged":[{"hooks":[{
        "type":"http", "id":"native-observer", "url":recorder.url, "fail_closed":true
    }]}]}));
}

#[tokio::test]
async fn native_owner_delivers_external_edits_rename_removal_to_shared_sessions_and_joins() {
    let flow = support::flow::Flow::new(vec![], false);
    let recorder = Recorder::start().await;
    automatic_hooks(&flow, &recorder);
    let first = flow.session("default").await;
    let second = flow.session("default").await;
    await_condition("automatic watcher checkpoint", || {
        journal_count(&flow, records::CHECKPOINT) == 1
    })
    .await;
    let has = |session: &str, path: &str, change: &str| {
        recorder.seen.lock().unwrap().iter().any(|event| {
            event["session_id"] == session
                && event["file_path"] == path
                && event["change"] == change
        })
    };
    std::fs::write(flow.f.repo.join("external"), "first").unwrap();
    await_condition("both real Session observations", || {
        has(&first, "external", "changed") && has(&second, "external", "changed")
    })
    .await;
    std::fs::rename(flow.f.repo.join("external"), flow.f.repo.join("renamed")).unwrap();
    await_condition("rename reconciliation", || {
        has(&first, "external", "removed") && has(&second, "renamed", "changed")
    })
    .await;
    std::fs::remove_file(flow.f.repo.join("renamed")).unwrap();
    await_condition("external removal", || {
        has(&first, "renamed", "removed") && has(&second, "renamed", "removed")
    })
    .await;
    std::fs::write(flow.f.repo.join(".git/private"), "secret").unwrap();
    std::fs::write(flow.f.repo.join("marker"), "after protected write").unwrap();
    await_condition("post-exclusion marker", || {
        has(&first, "marker", "changed") && has(&second, "marker", "changed")
    })
    .await;
    let events = recorder.seen.lock().unwrap().clone();
    assert!(
        events
            .iter()
            .all(|event| !event["file_path"].as_str().unwrap().starts_with(".git/"))
    );
    assert_eq!(
        journal_count(&flow, records::CHECKPOINT),
        1,
        "Sessions must share one journal writer"
    );
    assert!(events.iter().any(|a| {
        a["session_id"] == first
            && events
                .iter()
                .any(|b| b["session_id"] == second && a["observation_id"] == b["observation_id"])
    }));
    tokio::time::timeout(Duration::from_secs(20), flow.runtime.shutdown())
        .await
        .expect("shutdown must join automatic observers");
    let settled = recorder.seen.lock().unwrap().len();
    std::fs::write(flow.f.repo.join("after-shutdown"), "not delivered").unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(recorder.seen.lock().unwrap().len(), settled);
    assert!(flow.requests("test/main").is_empty());
}

#[tokio::test]
async fn native_owner_feedback_renews_only_for_new_committed_user_input() {
    let flow = support::flow::Flow::new(vec![], false);
    let recorder = Recorder::start().await;
    automatic_hooks(&flow, &recorder);
    let session = flow.session("default").await;
    await_condition("automatic baseline", || {
        journal_count(&flow, records::CHECKPOINT) == 1
    })
    .await;
    for index in 0..70 {
        std::fs::write(flow.f.repo.join(format!("file-{index:02}")), "external").unwrap();
    }
    let suppressed = || {
        flow.f.store.read(|db| Ok(db.query_row("SELECT COUNT(*) FROM event WHERE type=?1 AND json_extract(data,'$.dispatch')=0 AND json_array_length(json_extract(data,'$.updates'))>0", [records::CHANGED], |row| row.get::<_, usize>(0))?)).unwrap()
    };
    await_condition("feedback pause with continued recording", || {
        suppressed() > 0
    })
    .await;
    await_condition("all admitted handlers settled", || {
        recorder.seen.lock().unwrap().len() == records::FEEDBACK_LIMIT as usize
    })
    .await;
    let before = journal_count(&flow, records::CHANGED);
    tokio::time::sleep(Duration::from_millis(3300)).await;
    std::fs::write(flow.f.repo.join("after-quiet"), "still paused").unwrap();
    await_condition("paused watcher keeps recording", || {
        journal_count(&flow, records::CHANGED) > before
    })
    .await;
    assert_eq!(
        recorder.seen.lock().unwrap().len(),
        records::FEEDBACK_LIMIT as usize
    );

    let renewals = || {
        flow.f
            .store
            .read(|db| {
                Ok(db.query_row(
        "SELECT COUNT(*) FROM event WHERE type=?1 AND json_type(data,'$.renewal')='object'",
        [records::CHANGED], |row| row.get::<_, usize>(0))?)
            })
            .unwrap()
    };
    let mut hook = cyber_server::runtime::Admission::text(
        "hook input cannot resume observers",
        cyber_server::runtime::Delivery::Hold,
    );
    hook.source = "hook".into();
    hook.resume = false;
    flow.runtime.admit(&session, hook).await.unwrap();
    let before = journal_count(&flow, records::CHANGED);
    std::fs::write(flow.f.repo.join("after-hook-input"), "still paused").unwrap();
    await_condition("non-user input keeps observations paused", || {
        journal_count(&flow, records::CHANGED) > before
    })
    .await;
    assert_eq!(renewals(), 0);
    assert_eq!(
        recorder.seen.lock().unwrap().len(),
        records::FEEDBACK_LIMIT as usize
    );

    let mut user = cyber_server::runtime::Admission::text(
        "resume file observers",
        cyber_server::runtime::Delivery::Hold,
    );
    user.resume = false;
    user.message_id = Some(cyber_core::ids::new_id("msg"));
    flow.runtime.admit(&session, user.clone()).await.unwrap();
    await_condition("committed user input renews feedback", || renewals() == 1).await;
    std::fs::write(flow.f.repo.join("after-user-input"), "resumed").unwrap();
    await_condition("fresh observation after user action", || {
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["file_path"] == "after-user-input")
    })
    .await;
    flow.runtime.admit(&session, user).await.unwrap();
    std::fs::write(flow.f.repo.join("after-retry"), "no extra renewal").unwrap();
    await_condition("post-retry observation", || {
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["file_path"] == "after-retry")
    })
    .await;
    assert_eq!(
        renewals(),
        1,
        "an exact input retry must not enqueue another renewal"
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn native_owner_rebinds_after_root_replacement_and_ignores_the_old_directory() {
    let flow = support::flow::Flow::new(vec![], false);
    let recorder = Recorder::start().await;
    automatic_hooks(&flow, &recorder);
    let session = flow.session("default").await;
    await_condition("original root baseline", || {
        journal_count(&flow, records::CHECKPOINT) == 1
    })
    .await;
    std::fs::write(flow.f.repo.join("before"), "original").unwrap();
    await_condition("original observation", || {
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["file_path"] == "before")
    })
    .await;
    let old_generation = recorder.seen.lock().unwrap()[0]["watch_generation"].clone();
    let retired = flow.f.repo.with_file_name("retired-repo");
    std::fs::rename(&flow.f.repo, &retired).unwrap();
    std::fs::create_dir(&flow.f.repo).unwrap();
    std::fs::write(flow.f.repo.join("baseline"), "replacement baseline").unwrap();
    await_condition("replacement generation", || {
        journal_count(&flow, records::CHECKPOINT) == 2
    })
    .await;
    std::fs::write(retired.join("retired-only"), "must not dispatch").unwrap();
    std::fs::write(flow.f.repo.join("replacement"), "current root").unwrap();
    await_condition("replacement observation", || {
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["file_path"] == "replacement" && event["session_id"] == session)
    })
    .await;
    let events = recorder.seen.lock().unwrap().clone();
    assert!(
        events
            .iter()
            .all(|event| event["file_path"] != "retired-only")
    );
    assert!(
        events
            .iter()
            .filter(|event| event["file_path"] == "replacement")
            .all(|event| event["watch_generation"] != old_generation)
    );
    flow.runtime.shutdown().await;
}
