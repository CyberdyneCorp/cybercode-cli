//! Production controller with finite offline inference and disposable local Git remotes.
use super::*;
use cyber_core::paths::DatabaseLocation;
use cyber_llm::adapters::{ScriptStep, ScriptedAdapter};
use cyber_llm::{FinishReason, LlmEvent, LlmRequest, ToolCall, ToolSpec, Usage};
use cyber_server::runtime::{
    Invocation, ResolvedModel, RetrySafety, RuntimeOptions, ToolDef, ToolHost, ToolOutcome,
    TurnContext,
};
use cyber_store::{Store, StoreOptions};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

struct Models(Arc<ScriptedAdapter>);
impl ModelResolver for Models {
    fn role(&self, _: cyber_llm::catalog::ModelRole) -> Option<String> {
        None
    }
    fn resolve(&self, _: &str) -> Result<ResolvedModel, String> {
        Ok(ResolvedModel {
            adapter: self.0.clone(),
            template: LlmRequest {
                model: "fixture".into(),
                max_output_tokens: Some(100),
                ..Default::default()
            },
            provider: "fixture".into(),
            model: "fixture".into(),
            context_limit: 4096,
            cost: None,
            prefers_apply_patch: false,
            family: cyber_core::prompts::ModelFamily::Default,
            vision: false,
            pdf: false,
        })
    }
}
struct FileTool;
impl ToolHost for FileTool {
    fn definitions(&self, _: &TurnContext) -> Vec<ToolDef> {
        vec![ToolDef {
            scope: cyber_server::runtime::ToolScope::Builtin,
            deferred: false,
            registration: None,
            spec: ToolSpec {
                name: "write".into(),
                description: "fixture write".into(),
                input_schema: json!({"type":"object","properties":{}}),
            },
            retry_safety: RetrySafety::Idempotent,
            concurrency_safe: false,
        }]
    }
    fn execute(&self, call: Invocation, _: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        Box::pin(async move {
            std::fs::write(Path::new(&call.directory).join("code.txt"), "fixed\n").unwrap();
            ToolOutcome::Ok("fixture edit".into())
        })
    }
}
fn script() -> Arc<ScriptedAdapter> {
    Arc::new(ScriptedAdapter::new(vec![
        vec![
            ScriptStep::Event(LlmEvent::ToolCallDone(ToolCall {
                id: "fixture_write".into(),
                name: "write".into(),
                arguments: "{}".into(),
                input: Some(json!({})),
            })),
            ScriptStep::Event(LlmEvent::Usage(Usage {
                input: 10,
                output: 2,
                ..Default::default()
            })),
            ScriptStep::Event(LlmEvent::Finish {
                reason: FinishReason::ToolCalls,
            }),
        ],
        vec![
            ScriptStep::Event(LlmEvent::TextDelta {
                text: "fixed".into(),
            }),
            ScriptStep::Event(LlmEvent::Usage(Usage {
                input: 12,
                output: 3,
                ..Default::default()
            })),
            ScriptStep::Event(LlmEvent::Finish {
                reason: FinishReason::Stop,
            }),
        ],
    ]))
}
struct Fixture {
    _temp: tempfile::TempDir,
    controller: Arc<Controller>,
    model: Arc<ScriptedAdapter>,
    remote: PathBuf,
    head: String,
}
fn git(path: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args([
            "-c",
            "init.templateDir=",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgSign=false",
        ])
        .args(args)
        .current_dir(path)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@localhost")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@localhost")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
impl Fixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let work = root.join("project");
        std::fs::create_dir(&work).unwrap();
        git(&work, &["init", "--initial-branch=fix"]);
        std::fs::create_dir(work.join("tests")).unwrap();
        std::fs::write(work.join("tests/check.txt"), "pinned\n").unwrap();
        std::fs::write(work.join("code.txt"), "old\n").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "base"]);
        let head = git(&work, &["rev-parse", "HEAD"]);
        let remote = root.join("remote.git");
        git(
            &root,
            &[
                "clone",
                "--bare",
                work.to_str().unwrap(),
                remote.to_str().unwrap(),
            ],
        );
        let store = Arc::new(
            Store::open(StoreOptions::new(
                DatabaseLocation::File(root.join("db")),
                Runtime::registry(),
            ))
            .unwrap(),
        );
        let model = script();
        let resolver: Arc<dyn ModelResolver> = Arc::new(Models(model.clone()));
        let runtime = Runtime::new(RuntimeOptions {
            store: store.clone(),
            resolver: resolver.clone(),
            tools: Arc::new(FileTool),
            global_config_dir: root.join("config"),
            shell: "sh".into(),
            claude_compat: false,
            compaction: Default::default(),
            retry: Default::default(),
            max_steps: Some(10),
            today: None,
            interactive: false,
            snapshots: Arc::new(cyber_server::runtime::NoSnapshots),
        });
        runtime
            .create_session(CreateSession {
                id: Some("ses_owner".into()),
                directory: work.to_string_lossy().into_owned(),
                model: "fixture/model".into(),
                title: Some("Owner".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        let mut controller = Controller::new(
            runtime,
            store,
            root.join("attempts"),
            root.join("home"),
            Arc::new(|_| Ok((json!({}), Default::default()))),
            resolver,
            vec![],
        );
        Arc::get_mut(&mut controller)
            .unwrap()
            .executor
            .source_override = Some(remote.clone());
        Self {
            _temp: temp,
            controller,
            model,
            remote,
            head,
        }
    }
    fn subscription(&self) -> Subscription {
        Subscription::new(
            "afx_controller".into(),
            self._temp
                .path()
                .join("project")
                .to_string_lossy()
                .into_owned(),
            Config {
                repository: "team/repo".into(),
                pr: 1,
                session_id: "ses_owner".into(),
                model: "fixture/model".into(),
                checks: vec![vec!["true".into()]],
                required_ci: vec!["check:9:test".into()],
                verification_paths: vec!["tests".into()],
                max_attempts: 3,
                max_tokens: 10000,
                max_seconds: 60,
            },
            42,
            "fix".into(),
            now(),
        )
        .unwrap()
    }
    fn claimed(&self) -> Subscription {
        let mut s = self.subscription();
        s.enqueue(
            vec![cyber_core::autofix::Trigger {
                key: "check:1".into(),
                head: self.head.clone(),
                kind: cyber_core::autofix::TriggerKind::CheckFailure,
                text: "fix code.txt".into(),
            }],
            &self.head,
        )
        .unwrap();
        s.claim("afxa_one".into(), &self.head, now()).unwrap();
        self.controller.records.create(s).unwrap()
    }
    async fn github(
        &self,
        head: Arc<std::sync::Mutex<String>>,
        write: bool,
    ) -> tokio::task::JoinHandle<()> {
        use axum::{Json, Router, extract::State, routing::get};
        #[derive(Clone)]
        struct Api {
            head: Arc<std::sync::Mutex<String>>,
            write: bool,
        }
        async fn repo(State(s): State<Api>) -> Json<Value> {
            Json(json!({"id":42,"full_name":"team/repo","permissions":{"push":s.write}}))
        }
        async fn pull(State(s): State<Api>) -> Json<Value> {
            Json(
                json!({"state":"open","head":{"sha":s.head.lock().unwrap().clone(),"ref":"fix","repo":{"id":42}},"base":{"repo":{"id":42}}}),
            )
        }
        let app = Router::new()
            .route("/repos/team/repo", get(repo))
            .route("/repos/team/repo/pulls/1", get(pull))
            .route("/user", get(|| async { Json(json!({"login":"owner"})) }))
            .route(
                "/repos/team/repo/commits/{sha}/check-runs",
                get(|| async { Json(json!({"total_count":0,"check_runs":[]})) }),
            )
            .route(
                "/repos/team/repo/commits/{sha}/statuses",
                get(|| async { Json(json!([])) }),
            )
            .route(
                "/repos/team/repo/pulls/1/comments",
                get(|| async { Json(json!([])) }),
            )
            .route(
                "/repos/team/repo/pulls/1/reviews",
                get(|| async { Json(json!([])) }),
            )
            .with_state(Api { head, write });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let github = GitHub::fixture(base).unwrap();
        assert!(self.controller.github.set(github).is_ok());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() })
    }
}
#[tokio::test]
async fn recovery_marks_interrupted_work_unknown_without_model_dispatch() {
    let f = Fixture::new().await;
    let s = f.claimed();
    f.controller
        .records
        .update(&s.id, s.revision, now(), |s| {
            s.running("afxa_one", "ses_fix".into())
        })
        .unwrap();
    f.controller.recover();
    let value = f.controller.records.get(&s.id).unwrap();
    assert_eq!(value.status, Status::Paused);
    assert_eq!(value.active().unwrap().stage, Stage::Unknown);
    assert!(f.model.requests().is_empty());
    f.controller.runtime.shutdown().await;
}
#[tokio::test]
async fn recovery_keeps_push_intent_for_read_only_reconciliation() {
    let f = Fixture::new().await;
    let s = f.claimed();
    let s = f
        .controller
        .records
        .update(&s.id, s.revision, now(), |s| {
            let a = s.attempts.last_mut().unwrap();
            a.stage = Stage::PushIntent;
            a.commit = Some("2".repeat(40));
            Ok(())
        })
        .unwrap();
    f.controller.recover();
    assert_eq!(
        f.controller
            .records
            .get(&s.id)
            .unwrap()
            .active()
            .unwrap()
            .stage,
        Stage::PushIntent
    );
    assert!(f.model.requests().is_empty());
    f.controller.runtime.shutdown().await;
}
#[tokio::test]
async fn revoked_repository_permission_pauses_before_dispatch() {
    let f = Fixture::new().await;
    let s = f.claimed();
    let server = f
        .github(Arc::new(std::sync::Mutex::new(f.head.clone())), false)
        .await;
    let value = f.controller.poll_one(s).await.unwrap();
    assert_eq!(value.status, Status::Paused);
    assert!(f.model.requests().is_empty());
    server.abort();
    f.controller.runtime.shutdown().await;
}
#[tokio::test]
async fn lost_push_acknowledgement_reconciles_exact_remote_commit_without_repeat() {
    let f = Fixture::new().await;
    let s = f.claimed();
    let commit = "2".repeat(40);
    let c = commit.clone();
    let s = f
        .controller
        .records
        .update(&s.id, s.revision, now(), move |s| {
            let a = s.attempts.last_mut().unwrap();
            a.stage = Stage::PushIntent;
            a.commit = Some(c);
            s.unknown("afxa_one", "lost acknowledgement")
        })
        .unwrap();
    let server = f
        .github(Arc::new(std::sync::Mutex::new(commit)), true)
        .await;
    let value = f.controller.poll_one(s).await.unwrap();
    assert_eq!(value.active().unwrap().stage, Stage::AwaitingCi);
    assert_eq!(value.status, Status::Paused);
    assert!(f.model.requests().is_empty());
    server.abort();
    f.controller.runtime.shutdown().await;
}
#[cfg(unix)]
#[tokio::test]
#[ignore = "controlled native sandbox acceptance; run explicitly"]
async fn native_controller_fixes_verifies_and_pushes_only_once() {
    let f = Fixture::new().await;
    let head = Arc::new(std::sync::Mutex::new(f.head.clone()));
    let server = f.github(head, true).await;
    let s = f.claimed();
    f.controller.attempt(s.clone()).await.unwrap();
    let value = f.controller.records.get(&s.id).unwrap();
    assert_eq!(value.active().unwrap().stage, Stage::AwaitingCi);
    assert_eq!(f.model.requests().len(), 2);
    let commit = value.active().unwrap().commit.as_ref().unwrap();
    assert_eq!(git(&f.remote, &["rev-parse", "refs/heads/fix"]), *commit);
    assert_eq!(git(&f.remote, &["show", "fix:code.txt"]), "fixed");
    assert_eq!(git(&f.remote, &["rev-parse", "fix^"]), f.head);
    assert!(f.controller.attempt(value).await.is_err());
    assert_eq!(f.model.requests().len(), 2);
    server.abort();
    f.controller.runtime.shutdown().await;
}

#[tokio::test]
async fn poll_does_not_reconcile_a_live_push_before_or_after_remote_update() {
    let f = Fixture::new().await;
    let s = f.claimed();
    let intended = "2".repeat(40);
    let c = intended.clone();
    let s = f
        .controller
        .records
        .update(&s.id, s.revision, now(), move |s| {
            let a = s.attempts.last_mut().unwrap();
            a.stage = Stage::PushIntent;
            a.commit = Some(c);
            Ok(())
        })
        .unwrap();
    let head = Arc::new(std::sync::Mutex::new(f.head.clone()));
    let server = f.github(head.clone(), true).await;
    let (release, wait) = tokio::sync::oneshot::channel::<()>();
    *f.controller.worker.lock().await = Some((
        s.id.clone(),
        "afxa_one".into(),
        tokio::spawn(async {
            let _ = wait.await;
        }),
    ));
    let before = f.controller.poll_one(s).await.unwrap();
    assert_eq!(before.status, Status::Enabled);
    assert_eq!(before.active().unwrap().stage, Stage::PushIntent);
    *head.lock().unwrap() = intended;
    let after = f.controller.poll_one(before).await.unwrap();
    assert_eq!(after.status, Status::Enabled);
    assert_eq!(after.active().unwrap().stage, Stage::PushIntent);
    let acknowledged = f
        .controller
        .records
        .update(&after.id, after.revision, now(), |s| s.pushed("afxa_one"))
        .unwrap();
    f.controller
        .records
        .update(&acknowledged.id, acknowledged.revision, now(), |s| {
            s.pushed("afxa_one")
        })
        .unwrap();
    release.send(()).unwrap();
    let (_, _, worker) = f.controller.worker.lock().await.take().unwrap();
    worker.await.unwrap();
    assert!(f.model.requests().is_empty());
    server.abort();
    f.controller.runtime.shutdown().await;
}

#[tokio::test]
async fn another_pr_worker_does_not_consume_a_queued_attempts_time_allowance() {
    let f = Fixture::new().await;
    let server = f
        .github(Arc::new(std::sync::Mutex::new(f.head.clone())), true)
        .await;
    let mut s = f.subscription();
    s.config.max_seconds = 1;
    s.next_poll_ms = now() + 3_600_000;
    s.enqueue(
        vec![cyber_core::autofix::Trigger {
            key: "waiting_failure".into(),
            head: f.head.clone(),
            kind: cyber_core::autofix::TriggerKind::CheckFailure,
            text: "queued feedback".into(),
        }],
        &f.head,
    )
    .unwrap();
    let s = f.controller.records.create(s).unwrap();
    let (release, wait) = tokio::sync::oneshot::channel::<()>();
    *f.controller.worker.lock().await = Some((
        "another_pr".into(),
        "another_attempt".into(),
        tokio::spawn(async {
            let _ = wait.await;
        }),
    ));
    f.controller.tick().await.unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    f.controller.tick().await.unwrap();
    assert!(f.controller.records.get(&s.id).unwrap().attempts.is_empty());
    release.send(()).unwrap();
    let (_, _, worker) = f.controller.worker.lock().await.take().unwrap();
    worker.await.unwrap();
    let before = now();
    let selected = f.controller.claim_next().await.unwrap().unwrap();
    assert!(selected.active().unwrap().started_ms >= before);
    assert_eq!(selected.status, Status::Enabled);
    assert!(selected.active().unwrap().started_ms + 1000 > now());
    assert!(f.model.requests().is_empty());
    server.abort();
    f.controller.runtime.shutdown().await;
}

#[tokio::test]
async fn queued_subscription_respects_backoff_without_querying_credentials() {
    let f = Fixture::new().await;
    let mut s = f.subscription();
    s.backoff_until_ms = Some(now() + 60_000);
    s.enqueue(
        vec![cyber_core::autofix::Trigger {
            key: "waiting".into(),
            head: f.head.clone(),
            kind: cyber_core::autofix::TriggerKind::CheckFailure,
            text: "queued".into(),
        }],
        &f.head,
    )
    .unwrap();
    f.controller.records.create(s).unwrap();
    assert!(f.controller.claim_next().await.unwrap().is_none());
    assert!(f.controller.github.get().is_none());
    assert!(f.model.requests().is_empty());
    f.controller.runtime.shutdown().await;
}
