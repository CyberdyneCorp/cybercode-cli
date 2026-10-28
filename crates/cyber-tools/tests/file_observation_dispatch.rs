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
        Self::with_gate(None).await
    }
    async fn with_gate(gate: Option<Arc<tokio::sync::Notify>>) -> Self {
        Self::configured(gate, None, false).await
    }
    async fn with_effect(effect: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::configured(None, Some(effect), false).await
    }
    async fn with_first_gate(gate: Arc<tokio::sync::Notify>) -> Self {
        Self::configured(Some(gate), None, true).await
    }
    async fn configured(
        gate: Option<Arc<tokio::sync::Notify>>,
        effect: Option<Arc<dyn Fn() + Send + Sync>>,
        gate_once: bool,
    ) -> Self {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let gated = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        let router=axum::Router::new().route("/hook",axum::routing::post({
            let seen=Arc::clone(&seen);
            move |axum::Json(event):axum::Json<Value>| {
                let seen=Arc::clone(&seen);
                let gate=gate.clone();
                let effect=effect.clone();
                let gated=Arc::clone(&gated);
                async move {
                    seen.lock().unwrap().push(event);
                    if let Some(effect) = effect { effect(); }
                    if let Some(gate) = gate
                        && (!gate_once || !gated.swap(true, std::sync::atomic::Ordering::AcqRel))
                    { gate.notified().await; }
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
fn journal_count(flow: &support::flow::Flow, kind: &'static str) -> i64 {
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
    std::fs::create_dir_all(flow.f.repo.join("nested/deep")).unwrap();
    std::fs::write(flow.f.repo.join("nested/deep/external"), "first").unwrap();
    await_condition("both real Session observations", || {
        has(&first, "nested/deep/external", "changed")
            && has(&second, "nested/deep/external", "changed")
    })
    .await;
    std::fs::rename(
        flow.f.repo.join("nested/deep/external"),
        flow.f.repo.join("nested/deep/renamed"),
    )
    .unwrap();
    await_condition("rename reconciliation", || {
        has(&first, "nested/deep/external", "removed")
            && has(&second, "nested/deep/renamed", "changed")
    })
    .await;
    std::fs::remove_file(flow.f.repo.join("nested/deep/renamed")).unwrap();
    await_condition("external removal", || {
        has(&first, "nested/deep/renamed", "removed")
            && has(&second, "nested/deep/renamed", "removed")
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
async fn idle_worktree_return_refreshes_observation_membership_without_another_turn() {
    use support::flow::{call, text};
    let fixture = full_access_fixture();
    initialize_project(
        &fixture.repo,
        "https://example.invalid/observation-idle-return",
    );
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&fixture.repo)
        .args([
            "-c",
            "user.name=Observation test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "baseline",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let flow = support::flow::Flow::with(
        fixture,
        vec![
            call(
                "enter",
                "enter_worktree",
                json!({"name":"observation-scope"}),
            ),
            text("idle in worktree"),
        ],
        false,
        Arc::new(cyber_server::runtime::NoSnapshots),
    );
    flow.f
        .set_config(json!({"permissions":{"worktree":"allow","edit":"allow"}}));
    let recorder = Recorder::start().await;
    flow.f.global_hooks(json!({
        "FileChanged":[{"hooks":[{"type":"http","id":"native-observer",
            "url":recorder.url,"fail_closed":true}]}],
        "CwdChanged":[{"hooks":[{"type":"http","id":"cwd-observer",
            "url":recorder.url,"fail_closed":true}]}]
    }));
    let active = flow.session("default").await;
    let moved = flow.session("default").await;
    flow.prompt(&moved, "enter a worktree").await;
    flow.settle_within(&moved, Duration::from_secs(60)).await;
    let current = flow.runtime.session_info(&moved).await.unwrap();
    let worktree = std::path::PathBuf::from(&current.directory);
    let entered = flow.output(&moved, "enter").await;
    assert_ne!(worktree, flow.f.repo, "{entered}");
    await_condition("both Location observers are live", || {
        journal_count(&flow, records::CHECKPOINT) == 2
    })
    .await;
    let has = |session: &str, path: &str| {
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["session_id"] == session && event["file_path"] == path)
    };
    std::fs::write(worktree.join("worktree-only"), "entered scope").unwrap();
    std::fs::write(flow.f.repo.join("base-before-return"), "base scope").unwrap();
    await_condition("effective scopes before idle return", || {
        has(&moved, "worktree-only") && has(&active, "base-before-return")
    })
    .await;
    assert!(!has(&moved, "base-before-return"));
    flow.runtime
        .exit_idle_worktree(&moved)
        .await
        .unwrap()
        .expect("entered idle Session returns");
    assert_eq!(
        flow.runtime.session_info(&moved).await.unwrap().directory,
        flow.f.repo.display().to_string()
    );
    let transitions: Vec<_> = recorder
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|event| event["event"] == "CwdChanged")
        .cloned()
        .collect();
    assert_eq!(transitions.len(), 2, "{transitions:?}");
    assert_eq!(transitions[0]["transition"], "session.worktree.entered.1");
    assert_eq!(transitions[1]["transition"], "session.worktree.exited.1");
    for event in &transitions {
        assert_eq!(event["session_id"], moved);
        assert_eq!(event["location"]["directory"], event["directory"]);
        assert_ne!(event["transition_id"], serde_json::Value::Null);
    }
    assert_eq!(
        transitions[1]["directory"],
        flow.f.repo.display().to_string()
    );
    assert_eq!(transitions[1]["previous_directory"], current.directory);
    std::fs::write(
        flow.f.repo.join("idle-return-barrier"),
        "settle any already captured membership",
    )
    .unwrap();
    await_condition("old active member reaches the return barrier", || {
        has(&active, "idle-return-barrier")
    })
    .await;
    std::fs::write(
        worktree.join("old-scope-after-return"),
        "must not reach returned Session",
    )
    .unwrap();
    std::fs::write(flow.f.repo.join("after-idle-return"), "new shared scope").unwrap();
    await_condition("returned membership observes without a new Turn", || {
        has(&active, "after-idle-return") && has(&moved, "after-idle-return")
    })
    .await;
    assert!(!has(&moved, "old-scope-after-return"));
    assert_eq!(
        flow.requests("test/main").len(),
        2,
        "observations cannot manufacture a Turn"
    );
    tokio::time::timeout(Duration::from_secs(10), flow.runtime.shutdown())
        .await
        .expect("relocated observers must join at shutdown");
}

#[tokio::test]
async fn archived_membership_resumes_future_observations_without_replaying_absent_effects() {
    use cyber_server::runtime::{Admission, Delivery};
    let flow = support::flow::Flow::new(vec![], false);
    let recorder = Recorder::start().await;
    automatic_hooks(&flow, &recorder);
    let active = flow.session("default").await;
    let resumed = flow.session("default").await;
    await_condition("shared baseline before archive", || {
        journal_count(&flow, records::CHECKPOINT) == 1
    })
    .await;
    let has = |session: &str, path: &str| {
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["session_id"] == session && event["file_path"] == path)
    };
    std::fs::write(flow.f.repo.join("before-archive"), "shared").unwrap();
    await_condition("both memberships initially deliver", || {
        has(&active, "before-archive") && has(&resumed, "before-archive")
    })
    .await;
    flow.runtime.archive(&resumed, true).await.unwrap();
    std::fs::write(flow.f.repo.join("while-archived"), "active Session only").unwrap();
    await_condition(
        "active membership still delivers while peer is archived",
        || has(&active, "while-archived"),
    )
    .await;
    assert!(!has(&resumed, "while-archived"));
    flow.runtime.archive(&resumed, false).await.unwrap();
    flow.runtime
        .admit(
            &resumed,
            Admission::text("resume file coverage", Delivery::Hold),
        )
        .await
        .unwrap();
    // The actor may already have captured its membership for an in-flight scan.
    // Observe a barrier, then create a separate file for the next reconciliation.
    std::fs::write(
        flow.f.repo.join("resume-barrier"),
        "settle the current scan",
    )
    .unwrap();
    await_condition("post-admission scan barrier", || {
        has(&active, "resume-barrier")
    })
    .await;
    std::fs::write(flow.f.repo.join("after-resume"), "future shared effect").unwrap();
    await_condition(
        "explicitly resumed membership receives future observations",
        || has(&active, "after-resume") && has(&resumed, "after-resume"),
    )
    .await;
    assert!(
        !has(&resumed, "while-archived"),
        "resume must not replay absent effects"
    );
    assert_eq!(
        journal_count(&flow, records::CHECKPOINT),
        1,
        "archiving one member must preserve the shared owner generation"
    );
    assert!(flow.requests("test/main").is_empty());
    tokio::time::timeout(Duration::from_secs(10), flow.runtime.shutdown())
        .await
        .expect("archive/resume must preserve joined owner shutdown");
}

#[tokio::test]
async fn native_burst_during_a_held_callback_reconciles_the_tail_under_feedback_bounds() {
    use cyber_server::runtime::LiveEvent;
    let flow = support::flow::Flow::new(vec![], false);
    let gate = Arc::new(tokio::sync::Notify::new());
    let recorder = Recorder::with_first_gate(Arc::clone(&gate)).await;
    automatic_hooks(&flow, &recorder);
    let session = flow.session("default").await;
    await_condition("native burst baseline", || {
        journal_count(&flow, records::CHECKPOINT) == 1
    })
    .await;
    let mut notices = flow.runtime.subscribe();
    std::fs::write(
        flow.f.repo.join("held-before-burst"),
        "hold the actor in its real HTTP effect",
    )
    .unwrap();
    await_condition("first callback holds the real native consumer", || {
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["file_path"] == "held-before-burst")
    })
    .await;
    // The OS watcher continues on its native thread while the actor awaits HTTP.
    // This exceeds the queue/path budgets without exceeding the file inventory.
    for index in 0..800 {
        std::fs::write(
            flow.f.repo.join(format!("burst-{index:04}")),
            "external burst",
        )
        .unwrap();
    }
    std::fs::write(
        flow.f.repo.join("zzz-overflow-tail"),
        "reconcile beyond the hint/credit tail",
    )
    .unwrap();
    // Allow batched native backends to flush while consumption remains blocked.
    tokio::time::sleep(Duration::from_millis(2100)).await;
    gate.notify_one();
    await_condition("durable tail survives native loss and exhausted hook credit", || {
        flow.f.store.read(|db| Ok(db.query_row(
            "SELECT EXISTS(SELECT 1 FROM event,json_each(event.data,'$.updates') AS item WHERE event.type=?1 AND json_extract(item.value,'$.path')='zzz-overflow-tail' AND json_extract(event.data,'$.dispatch')=0)",
            [records::CHANGED], |row| row.get::<_, bool>(0),
        )?)).unwrap()
    }).await;
    let mut partial = false;
    let mut paused = false;
    while let Ok(event) = notices.try_recv() {
        if let LiveEvent::HookNotice {
            session_id,
            hook_id,
            message,
        } = event
            && session_id == session
            && hook_id == "FileChanged"
        {
            partial |= message.contains("partial coverage");
            paused |= message.contains("feedback allowance exhausted");
        }
    }
    assert!(
        partial,
        "native loss/coalescing must surface reconciliation uncertainty"
    );
    assert!(
        paused,
        "exhausted feedback must pause visibly while recording continues"
    );
    assert_eq!(
        recorder.seen.lock().unwrap().len(),
        records::FEEDBACK_LIMIT as usize
    );
    assert!(
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .all(|event| event["file_path"] != "zzz-overflow-tail")
    );
    assert_eq!(
        flow.runtime.hook_executions(&session, 100).unwrap().len(),
        records::FEEDBACK_LIMIT as usize
    );
    assert_eq!(journal_count(&flow, records::CHECKPOINT), 1);
    assert!(flow.requests("test/main").is_empty());
    tokio::time::timeout(Duration::from_secs(10), flow.runtime.shutdown())
        .await
        .expect("burst recovery must preserve joined observer shutdown");
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
        flow.f.store.read(|db| Ok(db.query_row("SELECT COUNT(*) FROM event WHERE type=?1 AND json_extract(data,'$.dispatch')=0 AND json_array_length(json_extract(data,'$.updates'))>0", [records::CHANGED], |row| row.get::<_, i64>(0))?)).unwrap()
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
        [records::CHANGED], |row| row.get::<_, i64>(0))?)
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
    initialize_project(&flow.f.repo, "https://example.test/old-project.git");
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
    initialize_project(&flow.f.repo, "https://example.test/new-project.git");
    let replacement_project = cyber_core::project::identify(&flow.f.repo).id;
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
    let replacement_events: Vec<_> = events
        .iter()
        .filter(|event| event["file_path"] == "replacement")
        .collect();
    assert!(
        replacement_events
            .iter()
            .all(|event| event["project_id"] == replacement_project)
    );
    assert!(
        events
            .iter()
            .filter(|event| event["file_path"] == "before")
            .all(|event| event["project_id"] != replacement_project)
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn builtin_write_and_edit_produce_observations_without_language_services() {
    let flow = support::flow::Flow::new(
        vec![
            support::flow::call(
                "write-call",
                "write",
                json!({"path":"nested/tool.txt", "content":"first"}),
            ),
            support::flow::text("written"),
            support::flow::call(
                "edit-call",
                "edit",
                json!({"path":"nested/tool.txt", "old_string":"first", "new_string":"other"}),
            ),
            support::flow::text("edited"),
        ],
        false,
    );
    let recorder = Recorder::start().await;
    automatic_hooks(&flow, &recorder);
    let disabled: serde_json::Map<String, Value> = cyber_core::intelligence::builtin_formatters()
        .into_iter()
        .map(|formatter| (formatter.id, json!({"disabled":true})))
        .collect();
    flow.f.set_config(json!({"formatters":disabled}));
    let session = flow.session("bypass").await;
    await_condition("tool observation baseline", || {
        journal_count(&flow, records::CHECKPOINT) == 1
    })
    .await;
    flow.prompt(&session, "write the fixture file").await;
    flow.settle(&session).await;
    await_condition("built-in write observation", || {
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["file_path"] == "nested/tool.txt")
    })
    .await;
    let before = recorder.seen.lock().unwrap().len();
    flow.prompt(&session, "edit the fixture file").await;
    flow.settle(&session).await;
    await_condition("same-length built-in edit observation", || {
        recorder.seen.lock().unwrap().len() > before
    })
    .await;
    assert_eq!(flow.f.read("nested/tool.txt"), "other");
    assert!(
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .all(|event| event.get("writer").is_none())
    );
    flow.runtime.shutdown().await;
}

fn initialize_project(path: &std::path::Path, remote: &str) {
    for args in [
        vec!["init", "-q"],
        vec!["config", "remote.origin.url", remote],
    ] {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[tokio::test]
async fn hard_link_scope_allows_owned_aliases_and_suppresses_private_state_aliases() {
    let flow = support::flow::Flow::new(vec![], false);
    let recorder = Recorder::start().await;
    automatic_hooks(&flow, &recorder);
    flow.f.write("owned-original", "public");
    std::fs::hard_link(
        flow.f.repo.join("owned-original"),
        flow.f.repo.join("owned-alias"),
    )
    .unwrap();
    flow.f.write("ambiguous-alias", "public");
    let private = flow.f.dir.path().join("tmp/private-state");
    std::fs::create_dir_all(private.parent().unwrap()).unwrap();
    std::fs::write(&private, "private metadata must not reach hook observers").unwrap();
    let _session = flow.session("default").await;
    await_condition("hard-link baseline", || {
        journal_count(&flow, records::CHECKPOINT) == 1
    })
    .await;
    std::fs::write(flow.f.repo.join("owned-original"), "public modification").unwrap();
    await_condition("both in-scope aliases observed", || {
        let events = recorder.seen.lock().unwrap();
        ["owned-original", "owned-alias"]
            .iter()
            .all(|path| events.iter().any(|event| event["file_path"] == *path))
    })
    .await;
    let prepared = private.with_file_name("prepared-alias");
    std::fs::hard_link(&private, &prepared).unwrap();
    std::fs::rename(&prepared, flow.f.repo.join("ambiguous-alias")).unwrap();
    std::fs::write(&private, "changed private metadata outside the Location").unwrap();
    std::fs::write(flow.f.repo.join("after-alias-marker"), "public marker").unwrap();
    await_condition("reconciliation after private alias replacement", || {
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["file_path"] == "after-alias-marker")
    })
    .await;
    assert!(
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .all(|event| event["file_path"] != "ambiguous-alias")
    );
    let leaked = flow.f.store.read(|db| Ok(db.query_row(
        "SELECT COUNT(*) FROM event e, json_each(e.data,'$.updates') u WHERE e.type=?1 AND json_extract(u.value,'$.path')='ambiguous-alias'",
        [records::CHANGED], |row| row.get::<_, i64>(0))?)).unwrap();
    assert_eq!(
        leaked, 0,
        "uncertain alias metadata must not be journalled or reported as deletion"
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn shutdown_settles_a_held_observer_callback_and_restart_does_not_replay_it() {
    let mut flow = support::flow::Flow::new(vec![support::flow::text("Session reopened")], false);
    let gate = Arc::new(tokio::sync::Notify::new());
    let recorder = Recorder::with_gate(Some(Arc::clone(&gate))).await;
    automatic_hooks(&flow, &recorder);
    let session = flow.session("default").await;
    await_condition("held-callback baseline", || {
        journal_count(&flow, records::CHECKPOINT) == 1
    })
    .await;
    std::fs::write(flow.f.repo.join("held-trigger"), "observed").unwrap();
    await_condition("handler entered before shutdown", || {
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["file_path"] == "held-trigger")
    })
    .await;
    tokio::time::timeout(Duration::from_secs(15), flow.runtime.shutdown())
        .await
        .expect("shutdown must cancel and settle the launched callback");
    let receipts = flow.runtime.hook_executions(&session, 100).unwrap();
    assert_eq!(receipts.len(), 1);
    assert!(
        receipts
            .iter()
            .all(|record| record.status != cyber_server::runtime::HookExecutionStatus::Running)
    );
    let consumed = receipts[0].id.clone();
    gate.notify_waiters();
    flow.restart_default_runtime().await;
    automatic_hooks(&flow, &recorder);
    flow.runtime.resume(&session).await.unwrap();
    flow.runtime.wait_idle(&session).await;
    await_condition("restarted watcher generation", || {
        journal_count(&flow, records::CHECKPOINT) == 2
    })
    .await;
    std::fs::write(flow.f.repo.join("fresh-after-restart"), "new observation").unwrap();
    await_condition("restarted owner is live", || {
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["file_path"] == "fresh-after-restart")
    })
    .await;
    assert_eq!(
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event["file_path"] == "held-trigger")
            .count(),
        1,
        "restart must not replay an already consumed observation"
    );
    assert!(
        flow.runtime
            .hook_executions(&session, 100)
            .unwrap()
            .iter()
            .any(|record| record.id == consumed)
    );
    tokio::time::timeout(Duration::from_secs(15), flow.runtime.shutdown())
        .await
        .expect("restarted observer must also settle its callback");
    gate.notify_waiters();
}

#[tokio::test]
async fn hook_writes_stop_at_the_durable_feedback_limit_after_quiet_time() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let flow = support::flow::Flow::new(vec![], false);
    let effects = Arc::new(AtomicUsize::new(0));
    let recorder = Recorder::with_effect({
        let root = flow.f.repo.clone();
        let effects = Arc::clone(&effects);
        Arc::new(move || {
            let index = effects.fetch_add(1, Ordering::SeqCst);
            std::fs::write(
                root.join(format!("hook-followup-{index:03}")),
                "hook side effect",
            )
            .unwrap();
        })
    })
    .await;
    automatic_hooks(&flow, &recorder);
    let _session = flow.session("default").await;
    await_condition("self-write observer baseline", || {
        journal_count(&flow, records::CHECKPOINT) == 1
    })
    .await;
    std::fs::write(flow.f.repo.join("seed"), "external edit").unwrap();
    let suppressed = || {
        flow.f.store.read(|db| Ok(db.query_row(
        "SELECT COUNT(*) FROM event WHERE type=?1 AND json_extract(data,'$.dispatch')=0 AND json_array_length(json_extract(data,'$.updates'))>0",
        [records::CHANGED], |row| row.get::<_, i64>(0))?)).unwrap()
    };
    await_condition("self-write loop reaches its durable pause", || {
        suppressed() > 0
    })
    .await;
    assert_eq!(
        effects.load(Ordering::SeqCst),
        records::FEEDBACK_LIMIT as usize
    );
    assert_eq!(
        recorder.seen.lock().unwrap().len(),
        records::FEEDBACK_LIMIT as usize
    );
    let before = suppressed();
    tokio::time::sleep(Duration::from_millis(3300)).await;
    std::fs::write(
        flow.f.repo.join("late-background-effect"),
        "delayed follow-up",
    )
    .unwrap();
    await_condition("paused owner records delayed changes", || {
        suppressed() > before
    })
    .await;
    assert_eq!(
        effects.load(Ordering::SeqCst),
        records::FEEDBACK_LIMIT as usize,
        "quiet time must not restart the hook feedback loop"
    );
    assert!(flow.requests("test/main").is_empty());
    flow.runtime.shutdown().await;
}

fn full_access_fixture() -> support::Fixture {
    // These tests cover observation after native effects, with explicit full
    // access. Native confinement is verified by its separate enforcement suite.
    support::Fixture::with_policy(
        "bash",
        cyber_sandbox::find_helper(),
        Some("full-access".into()),
    )
}

#[cfg(unix)]
fn observation_formatter(_fixture: &support::Fixture) -> Vec<String> {
    [
        "/bin/sh",
        "-c",
        "printf formatted > \"$1\"",
        "formatter",
        "$FILE",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
#[cfg(windows)]
fn observation_formatter(fixture: &support::Fixture) -> Vec<String> {
    let script = fixture.dir.path().join("observer-formatter.ps1");
    std::fs::write(&script, "param([string]$Target)\n[System.IO.File]::WriteAllText($Target, 'formatted', [System.Text.UTF8Encoding]::new($false))\n").unwrap();
    let system = std::env::var_os("SystemRoot").expect("Windows requires SystemRoot");
    vec![
        std::path::PathBuf::from(system)
            .join("System32/WindowsPowerShell/v1.0/powershell.exe")
            .display()
            .to_string(),
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-ExecutionPolicy".into(),
        "Bypass".into(),
        "-File".into(),
        script.display().to_string(),
        "$FILE".into(),
    ]
}

#[tokio::test]
async fn native_formatter_settlement_produces_a_file_observation() {
    let fixture = full_access_fixture();
    let mut formatters: serde_json::Map<String, Value> =
        cyber_core::intelligence::builtin_formatters()
            .into_iter()
            .map(|formatter| (formatter.id, json!({"disabled":true})))
            .collect();
    formatters.insert(
        "observer-formatter".into(),
        json!({"command":observation_formatter(&fixture), "extensions":[".txt"]}),
    );
    fixture.set_config(json!({"formatters":formatters}));
    let flow = support::flow::Flow::with(
        fixture,
        vec![
            support::flow::call(
                "formatter-write",
                "write",
                json!({"path":"formatted.txt", "content":"draft"}),
            ),
            support::flow::text("formatted"),
        ],
        false,
        Arc::new(cyber_server::runtime::NoSnapshots),
    );
    let recorder = Recorder::start().await;
    automatic_hooks(&flow, &recorder);
    let session = flow.session("bypass").await;
    await_condition("formatter observer baseline", || {
        journal_count(&flow, records::CHECKPOINT) == 1
    })
    .await;
    flow.prompt(&session, "write and format the fixture").await;
    flow.settle_within(&session, Duration::from_secs(30)).await;
    assert_eq!(
        flow.f.read("formatted.txt"),
        "formatted",
        "a real formatter must have run"
    );
    assert!(
        flow.output(&session, "formatter-write")
            .await
            .contains("Formatting result:")
    );
    await_condition("native formatter observation", || {
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["file_path"] == "formatted.txt")
    })
    .await;
    flow.runtime.shutdown().await;
}

#[cfg(unix)]
fn observation_shell_commands(_fixture: &support::Fixture) -> (&'static str, String, String) {
    (
        "bash",
        "printf foreground > shell-observed".into(),
        "n=0; while [ ! -f release-background ] && [ \"$n\" -lt 1200 ]; do sleep 0.05; n=$((n+1)); done; test -f release-background && printf background > background-observed".into(),
    )
}
#[cfg(windows)]
fn observation_shell_commands(fixture: &support::Fixture) -> (&'static str, String, String) {
    let quoted = |name: &str| {
        format!(
            "'{}'",
            fixture
                .repo
                .join(name)
                .display()
                .to_string()
                .replace('\'', "''")
        )
    };
    (
        "powershell",
        format!("[System.IO.File]::WriteAllText({}, 'foreground')", quoted("shell-observed")),
        "$limit=[DateTime]::UtcNow.AddSeconds(60); while (-not (Test-Path -LiteralPath 'release-background') -and ([DateTime]::UtcNow -lt $limit)) { Start-Sleep -Milliseconds 50 }; if (Test-Path -LiteralPath 'release-background') { [System.IO.File]::WriteAllText('background-observed', 'background') } else { exit 1 }"
            .replace("'release-background'", &quoted("release-background"))
            .replace("'background-observed'", &quoted("background-observed")),
    )
}

#[cfg(windows)]
async fn approve_observation_commands(
    flow: &support::flow::Flow,
    session: &str,
    commands: [&str; 2],
) {
    use cyber_server::runtime::{PendingKind, PermissionReply};
    for command in commands {
        let pending = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Some(pending) = flow
                    .runtime
                    .pending_requests(Some(session))
                    .into_iter()
                    .next()
                {
                    break pending;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        let pending = match pending {
            Ok(pending) => pending,
            Err(_) => {
                let state = flow.runtime.state(session).await.unwrap();
                let outputs: Vec<_> = state
                    .calls
                    .iter()
                    .map(|(id, call)| (id, &call.output))
                    .collect();
                panic!(
                    "native fixture command did not request approval; tool outputs: {outputs:?}"
                );
            }
        };
        let PendingKind::Permission(ask) = &pending.kind else {
            panic!("expected native command confirmation")
        };
        assert_eq!(ask.action, "bash");
        assert_eq!(ask.metadata["requires_confirmation"], true);
        assert_eq!(ask.metadata["command"], command);
        flow.runtime
            .reply_permission(&pending.id, PermissionReply::Once)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn native_foreground_and_background_shell_writes_are_observed_after_the_turn() {
    let fixture = full_access_fixture();
    let (tool, foreground, background) = observation_shell_commands(&fixture);
    let flow = support::flow::Flow::with(
        fixture,
        vec![
            support::flow::call("shell-write", tool, json!({"command":foreground})),
            support::flow::call(
                "background-write",
                tool,
                json!({"command":background,"background":true,"notify":false}),
            ),
            support::flow::text("background started"),
        ],
        cfg!(windows),
        Arc::new(cyber_server::runtime::NoSnapshots),
    );
    let recorder = Recorder::start().await;
    automatic_hooks(&flow, &recorder);
    let session = flow
        .session(if cfg!(windows) { "default" } else { "bypass" })
        .await;
    await_condition("shell observer baseline", || {
        journal_count(&flow, records::CHECKPOINT) == 1
    })
    .await;
    flow.prompt(&session, "run foreground and background fixture commands")
        .await;
    // Default Mode permits individual confirmation for member calls/control
    // flow; bypass Mode refuses them. Approve only these exact fixture commands.
    #[cfg(windows)]
    approve_observation_commands(&flow, &session, [&foreground, &background]).await;
    flow.settle_within(&session, Duration::from_secs(30)).await;
    let foreground_output = flow.output(&session, "shell-write").await;
    let foreground_content = std::fs::read_to_string(flow.f.repo.join("shell-observed"))
        .unwrap_or_else(|error| {
            panic!("foreground file unavailable ({error}); tool output: {foreground_output}")
        });
    assert_eq!(
        foreground_content, "foreground",
        "foreground effect missing; tool output: {foreground_output}"
    );
    assert!(
        flow.output(&session, "background-write")
            .await
            .contains("job_id"),
        "background execution must have been admitted"
    );
    assert!(
        !flow.f.repo.join("background-observed").exists(),
        "background effect must wait until the Turn has ended"
    );
    std::fs::write(flow.f.repo.join("release-background"), "release").unwrap();
    await_condition("both native shell observations", || {
        let seen = recorder.seen.lock().unwrap();
        ["shell-observed", "background-observed"]
            .iter()
            .all(|path| seen.iter().any(|event| event["file_path"] == *path))
    })
    .await;
    assert_eq!(flow.f.read("background-observed"), "background");
    assert!(
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .all(|event| event.get("writer").is_none())
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn location_limit_is_visible_without_disabling_other_location_observations() {
    use cyber_server::runtime::{Admission, CreateSession, Delivery, LiveEvent};
    let flow = support::flow::Flow::new(vec![], false);
    let recorder = Recorder::start().await;
    automatic_hooks(&flow, &recorder);
    let mut events = flow.runtime.subscribe();
    let mut locations = Vec::new();
    // Public owner limit: these are real Sessions and native watchers rather
    // than a stand-in registry. The extra Session remains usable without a watch.
    for index in 0..33 {
        let directory = flow.f.dir.path().join(format!("location-{index:02}"));
        std::fs::create_dir(&directory).unwrap();
        let directory = directory.canonicalize().unwrap();
        let session = flow
            .runtime
            .create_session(CreateSession {
                directory: directory.display().to_string(),
                model: "test/main".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        locations.push((directory, session.id));
    }
    let (unwatched, unobserved_session) = &locations[32];
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let LiveEvent::HookNotice {
                session_id,
                hook_id,
                message,
            } = events.recv().await.unwrap()
                && session_id == *unobserved_session
                && hook_id == "FileChanged"
                && message.contains("active Location limit reached")
            {
                break;
            }
        }
    })
    .await
    .expect("coverage refusal must be visible to the affected Session");
    await_condition("all admitted Location baselines", || {
        journal_count(&flow, records::CHECKPOINT) == 32
    })
    .await;
    flow.runtime
        .admit(
            unobserved_session,
            Admission::text("continue without file coverage", Delivery::Hold),
        )
        .await
        .expect("watcher saturation must not prevent ordinary user input");
    std::fs::write(unwatched.join("uncovered"), "outside current coverage").unwrap();
    let (watched, observed_session) = &locations[0];
    std::fs::write(watched.join("public"), "still covered").unwrap();
    await_condition("Location refusal preserves other live coverage", || {
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["session_id"] == *observed_session && event["file_path"] == "public")
    })
    .await;
    assert_eq!(journal_count(&flow, records::CHECKPOINT), 32);
    assert!(recorder.seen.lock().unwrap().iter().all(|event| {
        event["session_id"] != *unobserved_session && event["file_path"] != "uncovered"
    }));
    assert!(flow.requests("test/main").is_empty());
    tokio::time::timeout(Duration::from_secs(10), flow.runtime.shutdown())
        .await
        .expect("all admitted observer owners must settle at shutdown");
}

fn recent_file_notices(
    events: &mut tokio::sync::broadcast::Receiver<cyber_server::runtime::LiveEvent>,
    sessions: &[&str],
) -> std::collections::VecDeque<(String, String)> {
    use cyber_server::runtime::LiveEvent;
    let mut notices = std::collections::VecDeque::new();
    loop {
        match events.try_recv() {
            Ok(LiveEvent::HookNotice {
                session_id,
                hook_id,
                message,
            }) if hook_id == "FileChanged" && sessions.contains(&session_id.as_str()) => {
                if notices.len() == 8 {
                    notices.pop_front();
                }
                notices.push_back((session_id, message));
            }
            Ok(_) | Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {}
            Err(_) => return notices,
        }
    }
}

#[tokio::test]
async fn session_limit_is_visible_and_released_membership_admits_a_waiting_session() {
    use cyber_server::runtime::{Admission, Delivery, LiveEvent, ToolHost};
    let flow = support::flow::Flow::new(vec![], false);
    let recorder = Recorder::start().await;
    automatic_hooks(&flow, &recorder);
    let mut events = flow.runtime.subscribe();
    let mut sessions = Vec::new();
    // Exercise the production membership bound through actual Session creation.
    // Saturation must leave the extra Session usable and the owner bounded.
    for _ in 0..257 {
        sessions.push(flow.session("default").await);
    }
    let waiting = sessions.last().unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let LiveEvent::HookNotice {
                session_id,
                hook_id,
                message,
            } = events.recv().await.unwrap()
                && session_id == *waiting
                && hook_id == "FileChanged"
                && message.contains("Session limit reached")
            {
                break;
            }
        }
    })
    .await
    .expect("Session coverage refusal must be visible");
    await_condition("shared Location baseline at membership limit", || {
        journal_count(&flow, records::CHECKPOINT) == 1
    })
    .await;
    flow.runtime
        .admit(
            waiting,
            Admission::text("usable while coverage is full", Delivery::Hold),
        )
        .await
        .expect("watcher membership saturation must not refuse user input");
    // Retain one positive control and release the remaining covered memberships.
    // This tests owner recovery without generating 256 redundant HTTP effects.
    for session in &sessions[1..256] {
        ToolHost::end_session(&*flow.f.host, session).await;
    }
    std::fs::write(flow.f.repo.join("before-readmission"), "covered control").unwrap();
    await_condition("existing Session remains covered", || {
        recorder.seen.lock().unwrap().iter().any(|event| {
            event["session_id"] == sessions[0] && event["file_path"] == "before-readmission"
        })
    })
    .await;
    assert!(
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .all(|event| { event["session_id"] != *waiting }),
        "refused membership must not receive effects"
    );
    flow.runtime
        .admit(
            waiting,
            Admission::text("join after capacity is released", Delivery::Hold),
        )
        .await
        .unwrap();
    let waiting_scope = waiting.clone();
    let prepared = || {
        let session = waiting_scope.clone();
        flow.f.store.read(move |db| Ok(db.query_row(
            "SELECT EXISTS(SELECT 1 FROM event WHERE type=?1 AND json_extract(data,'$.session_id')=?2)",
            rusqlite::params![records::SCOPE, session],
            |row| row.get::<_, bool>(0),
        )?)).unwrap()
    };
    let ready = tokio::time::timeout(Duration::from_secs(45), async {
        while !prepared() {
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await;
    if ready.is_err() {
        let notices = recent_file_notices(&mut events, &[waiting, &sessions[0]]);
        panic!(
            "timed out waiting for released capacity prepares durable Session scope; latest notices: {notices:?}; checkpoints={}, scopes={}",
            journal_count(&flow, records::CHECKPOINT),
            journal_count(&flow, records::SCOPE)
        );
    }
    std::fs::write(flow.f.repo.join("after-readmission"), "shared again").unwrap();
    await_condition(
        "existing and newly admitted memberships both observe",
        || {
            let seen = recorder.seen.lock().unwrap();
            [&sessions[0], waiting].into_iter().all(|session| {
                seen.iter().any(|event| {
                    event["session_id"] == *session && event["file_path"] == "after-readmission"
                })
            })
        },
    )
    .await;
    assert_eq!(journal_count(&flow, records::CHECKPOINT), 1);
    assert!(flow.requests("test/main").is_empty());
    tokio::time::timeout(Duration::from_secs(10), flow.runtime.shutdown())
        .await
        .expect("membership recovery must preserve joined owner shutdown");
}

#[derive(Clone, Copy)]
enum MemberEnd {
    Waiting,
    Held,
    Readmitted,
}
fn recorded_file_observations(recorder: &Recorder, session: &str, path: &str) -> Vec<String> {
    recorder
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|event| event["session_id"] == session && event["file_path"] == path)
        .map(|event| {
            event["observation_id"]
                .as_str()
                .expect("FileChanged has an observation ID")
                .to_owned()
        })
        .collect()
}
fn recorded_file_event(recorder: &Recorder, session: &str, path: &str) -> bool {
    !recorded_file_observations(recorder, session, path).is_empty()
}

async fn remove_member_during_delivery(
    flow: &support::flow::Flow,
    recorder: &Recorder,
    gate: &tokio::sync::Notify,
    sessions: &[String; 2],
    end: MemberEnd,
) -> (String, String) {
    use cyber_server::runtime::{Admission, Delivery, ToolHost};
    let (removed, retained) = if matches!(end, MemberEnd::Held) {
        (&sessions[0], &sessions[1])
    } else {
        (&sessions[1], &sessions[0])
    };
    ToolHost::end_session(&*flow.f.host, removed).await;
    if matches!(end, MemberEnd::Readmitted) {
        flow.runtime
            .admit(
                removed,
                Admission::text("rejoin while an older batch is held", Delivery::Hold),
            )
            .await
            .unwrap();
        // A fresh change to the same pathname is a valid future observation.
        std::fs::write(flow.f.repo.join("held-membership"), "fresh observation").unwrap();
    }
    if matches!(end, MemberEnd::Held) {
        // Keep the first HTTP response held: cancellation must settle its owned
        // delivery before the remaining member can run.
        await_condition("remaining member continues past cancelled callback", || {
            recorded_file_event(recorder, retained, "held-membership")
        })
        .await;
    } else {
        gate.notify_one();
    }
    (removed.clone(), retained.clone())
}

async fn verify_delivery_after_member_end(
    flow: &support::flow::Flow,
    recorder: &Recorder,
    removed: &str,
    retained: &str,
    held_observation: &str,
    end: MemberEnd,
) {
    std::fs::write(flow.f.repo.join("after-member-end"), "fixture").unwrap();
    await_condition("remaining member observes a later reconciliation", || {
        recorded_file_event(recorder, retained, "after-member-end")
    })
    .await;
    if matches!(end, MemberEnd::Readmitted) {
        await_condition("readmitted member observes a future batch", || {
            recorded_file_event(recorder, removed, "after-member-end")
        })
        .await;
        await_condition(
            "readmitted member observes a new change at the same pathname",
            || {
                recorded_file_observations(recorder, removed, "held-membership")
                    .iter()
                    .any(|id| id != held_observation)
            },
        )
        .await;
    } else {
        assert!(!recorded_file_event(recorder, removed, "after-member-end"));
    }
    if !matches!(end, MemberEnd::Held) {
        let delivered = recorded_file_observations(recorder, removed, "held-membership");
        assert!(
            delivered.iter().all(|id| id != held_observation),
            "a captured Session list cannot revive ended membership: held={held_observation}, delivered={delivered:?}"
        );
    }
}

async fn end_membership_during_held_delivery(end: MemberEnd) {
    let flow = support::flow::Flow::new(vec![], false);
    let gate = Arc::new(tokio::sync::Notify::new());
    let recorder = Recorder::with_first_gate(Arc::clone(&gate)).await;
    let mut sessions = [flow.session("default").await, flow.session("default").await];
    sessions.sort();
    // Enable observation only after both memberships exist, so the first
    // reconciliation captures both and has a deterministic delivery order.
    automatic_hooks(&flow, &recorder);
    await_condition("membership delivery baseline", || {
        journal_count(&flow, records::CHECKPOINT) == 1
    })
    .await;
    std::fs::write(flow.f.repo.join("held-membership"), "fixture").unwrap();
    await_condition("first member callback is held", || {
        recorded_file_event(&recorder, &sessions[0], "held-membership")
    })
    .await;
    let held_observation = recorded_file_observations(&recorder, &sessions[0], "held-membership")
        .into_iter()
        .next()
        .expect("the gated callback has a source observation");
    let (removed, retained) =
        remove_member_during_delivery(&flow, &recorder, &gate, &sessions, end).await;
    verify_delivery_after_member_end(
        &flow,
        &recorder,
        &removed,
        &retained,
        &held_observation,
        end,
    )
    .await;
    gate.notify_one();
    assert!(flow.requests("test/main").is_empty());
    tokio::time::timeout(Duration::from_secs(10), flow.runtime.shutdown())
        .await
        .expect("membership delivery shutdown must settle");
}

#[tokio::test]
async fn ended_member_is_skipped_after_another_session_callback_settles() {
    end_membership_during_held_delivery(MemberEnd::Waiting).await;
}

#[tokio::test]
async fn ending_a_held_member_settles_delivery_and_retains_other_members() {
    end_membership_during_held_delivery(MemberEnd::Held).await;
}

#[tokio::test]
async fn readmitting_membership_does_not_revive_a_held_batch() {
    end_membership_during_held_delivery(MemberEnd::Readmitted).await;
}

#[tokio::test]
async fn long_path_inventory_publishes_a_partial_baseline_and_keeps_existing_files_live() {
    use cyber_server::runtime::LiveEvent;
    let flow = support::flow::Flow::new(vec![], false);
    std::fs::write(flow.f.repo.join("!healthy"), "before").unwrap();
    let mut directory = flow.f.repo.clone();
    for index in 0..6 {
        directory = directory.join(format!("{index}{}", "d".repeat(109)));
        std::fs::create_dir(&directory).unwrap();
    }
    for index in 0..4800 {
        std::fs::write(
            directory.join(format!("{index:04}{}", "f".repeat(196))),
            "x",
        )
        .unwrap();
    }
    let recorder = Recorder::start().await;
    automatic_hooks(&flow, &recorder);
    let mut events = flow.runtime.subscribe();
    let session = flow.session("default").await;
    await_condition("byte-budgeted native baseline", || {
        journal_count(&flow, records::CHECKPOINT) == 1
    })
    .await;
    let checkpoint: records::Checkpoint = flow
        .f
        .store
        .read(|db| {
            let data: String = db.query_row(
                "SELECT data FROM event WHERE type=?1 ORDER BY seq DESC LIMIT 1",
                [records::CHECKPOINT],
                |row| row.get(0),
            )?;
            Ok(serde_json::from_str(&data).unwrap())
        })
        .unwrap();
    checkpoint.validate().unwrap();
    assert!(checkpoint.files.contains_key("!healthy"));
    assert!(
        checkpoint.files.len() < 4801,
        "byte-limited metadata must be visibly incomplete"
    );
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            if let LiveEvent::HookNotice {
                session_id,
                hook_id,
                message,
            } = events.recv().await.unwrap()
                && session_id == session
                && hook_id == "FileChanged"
                && message.contains("partial coverage")
            {
                break;
            }
        }
    })
    .await
    .expect("byte-limit coverage must be visible");
    std::fs::write(flow.f.repo.join("!healthy"), "after").unwrap();
    await_condition(
        "existing scoped file stays live under the metadata byte bound",
        || {
            recorder
                .seen
                .lock()
                .unwrap()
                .iter()
                .any(|event| event["session_id"] == session && event["file_path"] == "!healthy")
        },
    )
    .await;
    assert!(flow.requests("test/main").is_empty());
    tokio::time::timeout(Duration::from_secs(15), flow.runtime.shutdown())
        .await
        .expect("byte-limited observer must join at shutdown");
}

#[tokio::test]
async fn directory_registration_limit_is_visible_and_reconciliation_covers_its_tail() {
    use cyber_server::runtime::LiveEvent;
    let flow = support::flow::Flow::new(vec![], false);
    // Exceed the production 256-directory native registration bound without
    // exceeding the metadata inventory bound. Initial files consume no credit.
    for index in 0..260 {
        let directory = flow.f.repo.join(format!("tree-{index:03}"));
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("public"), "initial").unwrap();
    }
    let recorder = Recorder::start().await;
    automatic_hooks(&flow, &recorder);
    let mut events = flow.runtime.subscribe();
    let session = flow.session("default").await;
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            if let LiveEvent::HookNotice {
                session_id,
                hook_id,
                message,
            } = events.recv().await.unwrap()
                && session_id == session
                && hook_id == "FileChanged"
                && message.contains("partial coverage")
            {
                break;
            }
        }
    })
    .await
    .expect("incomplete native registration must be visible");
    std::fs::write(
        flow.f.repo.join("tree-259/public"),
        "changed beyond native directory budget",
    )
    .unwrap();
    await_condition(
        "reconciliation still observes the registration tail",
        || {
            recorder.seen.lock().unwrap().iter().any(|event| {
                event["session_id"] == session
                    && event["file_path"] == "tree-259/public"
                    && event["change"] == "changed"
            })
        },
    )
    .await;
    assert!(flow.requests("test/main").is_empty());
    tokio::time::timeout(Duration::from_secs(10), flow.runtime.shutdown())
        .await
        .expect("partial native coverage must still settle its owner");
}

#[tokio::test]
async fn alias_joining_first_shares_the_canonical_owner_and_preserves_raw_session_envelopes() {
    let flow = support::flow::Flow::new(vec![], false);
    let recorder = Recorder::start().await;
    automatic_hooks(&flow, &recorder);
    let canonical = flow.f.repo.display().to_string();
    let normal = canonical.strip_prefix(r"\\?\").unwrap_or(&canonical);
    let normal = normal
        .strip_prefix(r"UNC\")
        .map_or_else(|| normal.to_owned(), |unc| format!(r"\\{unc}"));
    let spelling = format!("{normal}/.");
    let alias = flow
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: spelling.clone(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    await_condition("alias-first canonical journal", || {
        journal_count(&flow, records::CHECKPOINT) == 1
    })
    .await;
    let direct = flow.session("default").await;
    std::fs::write(
        flow.f.repo.join("shared-alias-file"),
        "observed once per Session",
    )
    .unwrap();
    await_condition(
        "both spellings receive their own real scoped callback",
        || {
            let seen = recorder.seen.lock().unwrap();
            [
                (&alias.id, spelling.as_str()),
                (&direct, canonical.as_str()),
            ]
            .into_iter()
            .all(|(session, directory)| {
                seen.iter().any(|event| {
                    event["session_id"] == *session
                        && event["file_path"] == "shared-alias-file"
                        && event["location"]["directory"] == directory
                })
            })
        },
    )
    .await;
    assert_eq!(journal_count(&flow, records::CHECKPOINT), 1);
    {
        let seen = recorder.seen.lock().unwrap();
        let generations: std::collections::BTreeSet<_> = seen
            .iter()
            .map(|event| event["watch_generation"].as_str().unwrap())
            .collect();
        assert_eq!(generations.len(), 1);
    }
    flow.runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_retarget_loses_old_delivery_and_user_input_admits_the_new_physical_scope() {
    let flow = support::flow::Flow::new(vec![], false);
    initialize_project(&flow.f.repo, "https://example.test/alias-original.git");
    let other = flow.f.dir.path().join("other-repo");
    std::fs::create_dir(&other).unwrap();
    initialize_project(&other, "https://example.test/alias-retarget.git");
    let new_project = cyber_core::project::identify(&other).id;
    let alias_path = flow.f.dir.path().join("location-alias");
    std::os::unix::fs::symlink(&flow.f.repo, &alias_path).unwrap();
    let recorder = Recorder::start().await;
    automatic_hooks(&flow, &recorder);
    let alias = flow
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: alias_path.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let direct = flow.session("default").await;
    await_condition("symlink alias baseline", || {
        journal_count(&flow, records::CHECKPOINT) == 1
    })
    .await;
    std::fs::write(flow.f.repo.join("before-retarget"), "original").unwrap();
    await_condition("original alias callback", || {
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["session_id"] == alias.id && event["file_path"] == "before-retarget")
    })
    .await;
    std::fs::remove_file(&alias_path).unwrap();
    std::os::unix::fs::symlink(&other, &alias_path).unwrap();
    std::fs::write(
        flow.f.repo.join("old-root-only"),
        "other Session still observes this",
    )
    .unwrap();
    await_condition("unmoved Session retains original coverage", || {
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["session_id"] == direct && event["file_path"] == "old-root-only")
    })
    .await;
    assert!(
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .all(|event| event["session_id"] != alias.id || event["file_path"] != "old-root-only")
    );
    flow.runtime
        .admit(
            &alias.id,
            cyber_server::runtime::Admission::text(
                "observe the current root",
                cyber_server::runtime::Delivery::Hold,
            ),
        )
        .await
        .unwrap();
    await_condition("new physical Location baseline and input renewal", || {
        let session = alias.id.clone();
        journal_count(&flow, records::CHECKPOINT) == 2 && flow.f.store.read(move |db| Ok(db.query_row(
            "SELECT EXISTS(SELECT 1 FROM event WHERE type=?1 AND json_extract(data,'$.renewal.session_id')=?2)",
            rusqlite::params![records::CHANGED,session], |row| row.get::<_,bool>(0))?)).unwrap()
    }).await;
    std::fs::write(other.join("new-root-only"), "new scope").unwrap();
    await_condition("new scope has fresh project identity", || {
        recorder.seen.lock().unwrap().iter().any(|event| {
            event["session_id"] == alias.id
                && event["file_path"] == "new-root-only"
                && event["project_id"] == new_project
        })
    })
    .await;
    assert!(
        recorder
            .seen
            .lock()
            .unwrap()
            .iter()
            .all(|event| event["session_id"] != direct || event["file_path"] != "new-root-only")
    );
    flow.runtime.shutdown().await;
}
