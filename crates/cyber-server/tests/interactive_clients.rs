//! Real owner API and fake-clock browser attachment tests. No native credentials.
mod support;
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use cyber_server::{
    http::{
        self, AgentInfo, AppState, CommandInfo, EmbeddedClient, HttpOptions, ModelInfo, Services,
    },
    runtime::{
        CreateSession, PermissionAsk, PermissionReply, QuestionReply, Runtime, ToolDef,
        TurnContext,
        presence::{InteractiveClientError, Watching},
    },
};
use futures::future::BoxFuture;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};
use support::{Harness, Setup};
struct NoServices;
impl Services for NoServices {
    fn models(&self, _: &Path) -> BoxFuture<'_, Result<Vec<ModelInfo>, String>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn default_model(&self, _: &Path) -> Option<String> {
        Some("test/main".into())
    }
    fn agents(&self, _: &Path) -> Vec<AgentInfo> {
        vec![]
    }
    fn tools(&self, _: &TurnContext) -> Vec<ToolDef> {
        vec![]
    }
    fn commands(&self, _: &Path) -> Vec<CommandInfo> {
        vec![]
    }
    fn find_files(&self, _: &Path, _: &str, _: usize) -> Vec<String> {
        vec![]
    }
    fn expand_command(&self, _: &Path, _: &str, _: &str) -> Option<String> {
        None
    }
}
fn state(h: &Harness, runtime: Runtime) -> AppState {
    AppState {
        service: None,
        telemetry: None,
        runtime,
        remote_tools: Arc::default(),
        store: h.store.clone(),
        services: Arc::new(NoServices),
        options: Arc::new(HttpOptions {
            version: "fixture".into(),
            password: Some("owner-fixture-password".into()),
            cors_origins: vec![],
            default_directory: h.repo.clone(),
            features: vec!["session".into()],
        }),
    }
}
async fn call(
    client: &EmbeddedClient,
    method: Method,
    path: &str,
    key: Option<&str>,
    directory: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(format!("http://cyber.internal/api/v1{path}"));
    if let Some(key) = key {
        builder = builder.header("idempotency-key", key);
    }
    if let Some(dir) = directory {
        builder = builder.header("x-cyber-directory", dir);
    }
    let response = client.request(builder.body(Body::empty()).unwrap()).await;
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
fn permission() -> PermissionAsk {
    PermissionAsk {
        action: "bash".into(),
        resources: vec!["npm test".into()],
        always_patterns: vec![],
        metadata: Value::Null,
    }
}
async fn ask(runtime: &Runtime, id: &str) -> tokio::task::JoinHandle<PermissionReply> {
    let asker = runtime.operation_asker(id).await.unwrap();
    tokio::spawn(async move { asker.permission(permission()).await })
}
async fn pending(runtime: &Runtime, id: &str) -> cyber_server::runtime::PendingRequest {
    for _ in 0..1000 {
        if let Some(request) = runtime.pending_requests(Some(id)).into_iter().next() {
            return request;
        }
        tokio::task::yield_now().await;
    }
    panic!("request did not open");
}
// Paused-time tests explicitly advance time. Keep a ready task so unrelated SQLite
// receipt work does not make Tokio auto-advance to the next lease timer.
fn manual_time() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async {
        loop {
            tokio::task::yield_now().await;
        }
    })
}

#[tokio::test]
async fn real_tcp_routes_require_owner_auth_and_bind_session_location_and_capacity() {
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let other = h.session().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(http::serve_tcp(
        http::router(state(&h, h.runtime.clone())),
        listener,
        std::future::pending(),
    ));
    let client = reqwest::Client::new();
    let path = format!("{url}/api/v1/sessions/{id}/interactive-clients");
    assert_eq!(client.post(&path).send().await.unwrap().status(), 401);
    assert_eq!(
        client
            .post(&path)
            .header("origin", "https://attacker.example")
            .basic_auth("cyber", Some("owner-fixture-password"))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let create = client
        .post(&path)
        .basic_auth("cyber", Some("owner-fixture-password"))
        .header("idempotency-key", "real-browser-create")
        .send()
        .await
        .unwrap();
    assert_eq!(create.status(), 201);
    assert_eq!(create.headers()["cache-control"], "no-store");
    let receipt = create.json::<Value>().await.unwrap();
    let lease = receipt["data"]["client_id"].as_str().unwrap();
    assert!(lease.starts_with("icl_"));
    assert!(receipt["data"]["expires_at"].is_i64());
    let wrong_session = format!("{url}/api/v1/sessions/{other}/interactive-clients/{lease}");
    assert_eq!(
        client
            .put(wrong_session)
            .basic_auth("cyber", Some("owner-fixture-password"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        client
            .put(format!("{path}/{lease}"))
            .header("x-cyber-directory", h.dir.path().display().to_string())
            .basic_auth("cyber", Some("owner-fixture-password"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        client
            .put(format!("{path}/{lease}"))
            .basic_auth("cyber", Some("owner-fixture-password"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let mut additional = Vec::new();
    for _ in 0..15 {
        additional.push(
            h.runtime
                .add_interactive_client(&id, &h.repo.display().to_string())
                .await
                .unwrap(),
        );
    }
    assert!(matches!(
        h.runtime
            .add_interactive_client(&id, &h.repo.display().to_string())
            .await,
        Err(InteractiveClientError::Limit)
    ));
    for _ in 0..2 {
        assert_eq!(
            client
                .delete(format!("{path}/{lease}"))
                .basic_auth("cyber", Some("owner-fixture-password"))
                .send()
                .await
                .unwrap()
                .status(),
            204
        );
    }
    // Deleting this browser does not release any other attachment.
    assert!(
        h.runtime
            .renew_interactive_client(&id, &h.repo.display().to_string(), &additional[0].client_id)
            .is_ok()
    );
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn create_and_renew_receipts_never_recreate_expired_or_restarted_presence() {
    let ready = manual_time();
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let api = EmbeddedClient::new(http::router(state(&h, h.runtime.clone())));
    let path = format!("/sessions/{id}/interactive-clients");
    let (status, original) = call(
        &api,
        Method::POST,
        &path,
        Some("expired-original-create"),
        None,
    )
    .await;
    assert_eq!(status, 201);
    let client = original["data"]["client_id"].as_str().unwrap();
    let item = format!("{path}/{client}");
    let (status, renewed) = call(
        &api,
        Method::PUT,
        &item,
        Some("expired-original-renew"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    tokio::time::advance(Duration::from_secs(45)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        call(
            &api,
            Method::POST,
            &path,
            Some("expired-original-create"),
            None
        )
        .await
        .1,
        original
    );
    assert_eq!(
        call(
            &api,
            Method::PUT,
            &item,
            Some("expired-original-renew"),
            None
        )
        .await
        .1,
        renewed
    );
    assert_eq!(
        call(&api, Method::PUT, &item, Some("expired-new-renew"), None)
            .await
            .0,
        404
    );
    assert_eq!(
        ask(&h.runtime, &id).await.await.unwrap(),
        PermissionReply::Unattended
    );
    let restarted = h.restart();
    assert_eq!(
        ask(&restarted, &id).await.await.unwrap(),
        PermissionReply::Unattended
    );
    let after = EmbeddedClient::new(http::router(state(&h, restarted.clone())));
    assert_eq!(
        call(
            &after,
            Method::POST,
            &path,
            Some("expired-original-create"),
            None
        )
        .await
        .1,
        original
    );
    assert_eq!(
        call(&after, Method::PUT, &item, Some("restart-renew"), None)
            .await
            .0,
        404
    );
    ready.abort();
}

#[tokio::test(start_paused = true)]
async fn last_browser_expiry_settles_permission_and_question_without_touching_terminal_presence() {
    let ready = manual_time();
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let directory = h.repo.display().to_string();
    let first = h
        .runtime
        .add_interactive_client(&id, &directory)
        .await
        .unwrap();
    let second = h
        .runtime
        .add_interactive_client(&id, &directory)
        .await
        .unwrap();
    let permission = ask(&h.runtime, &id).await;
    pending(&h.runtime, &id).await;
    tokio::time::advance(Duration::from_secs(44)).await;
    h.runtime
        .renew_interactive_client(&id, &directory, &second.client_id)
        .unwrap();
    tokio::time::advance(Duration::from_secs(1)).await;
    h.runtime
        .release_interactive_client(&id, &directory, &first.client_id)
        .await
        .unwrap();
    assert!(!permission.is_finished());
    let terminal = h.runtime.attach_client(Watching::Session(id.clone()));
    tokio::time::advance(Duration::from_secs(44)).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(!permission.is_finished());
    drop(terminal);
    assert_eq!(permission.await.unwrap(), PermissionReply::Unattended);
    assert_eq!(
        ask(&h.runtime, &id).await.await.unwrap(),
        PermissionReply::Unattended
    );
    h.runtime
        .add_interactive_client(&id, &directory)
        .await
        .unwrap();
    let asker = h.runtime.operation_asker(&id).await.unwrap();
    let question = tokio::spawn(async move { asker.question(vec![]).await });
    pending(&h.runtime, &id).await;
    tokio::time::advance(Duration::from_secs(45)).await;
    assert_eq!(question.await.unwrap(), QuestionReply::Unattended);
    ready.abort();
}

#[tokio::test(start_paused = true)]
async fn a_parent_browser_routes_child_requests_but_never_another_session_or_location() {
    let ready = manual_time();
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let unrelated = h.session().await;
    let child = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            parent_id: Some(parent.clone()),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let directory = h.repo.display().to_string();
    let lease = h
        .runtime
        .add_interactive_client(&parent, &directory)
        .await
        .unwrap();
    let child_ask = ask(&h.runtime, &child).await;
    let request = pending(&h.runtime, &parent).await;
    assert_eq!(request.session_id, child);
    assert!(
        request
            .routed_to
            .iter()
            .any(|route| route.session_id == parent)
    );
    let api = EmbeddedClient::new(http::router(state(&h, h.runtime.clone())));
    let response = api
        .request(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "http://cyber.internal/api/v1/sessions/{parent}/permissions/{}/reply",
                    request.id
                ))
                .header("content-type", "application/json")
                .body(Body::from(json!({"reply":"once"}).to_string()))
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), 204);
    assert_eq!(child_ask.await.unwrap(), PermissionReply::Once);
    assert!(
        h.runtime
            .renew_interactive_client(&unrelated, &directory, &lease.client_id)
            .is_err()
    );
    h.runtime
        .release_interactive_client(&parent, &directory, &lease.client_id)
        .await
        .unwrap();
    assert_eq!(
        ask(&h.runtime, &child).await.await.unwrap(),
        PermissionReply::Unattended
    );
    // A legacy never-browser Session keeps the existing SDK contract.
    let other = ask(&h.runtime, &unrelated).await;
    let request = pending(&h.runtime, &unrelated).await;
    h.runtime
        .reply_permission(&request.id, PermissionReply::Once)
        .await
        .unwrap();
    assert_eq!(other.await.unwrap(), PermissionReply::Once);
    ready.abort();
}

#[tokio::test]
async fn competing_owner_reply_and_release_settle_once_without_a_rejection_cascade() {
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let directory = h.repo.display().to_string();
    let lease = h
        .runtime
        .add_interactive_client(&id, &directory)
        .await
        .unwrap();
    let task = ask(&h.runtime, &id).await;
    let request = pending(&h.runtime, &id).await;
    let (reply, released) = tokio::join!(
        h.runtime
            .reply_permission(&request.id, PermissionReply::Once),
        h.runtime
            .release_interactive_client(&id, &directory, &lease.client_id)
    );
    assert!(released.is_ok());
    let outcome = task.await.unwrap();
    assert!(matches!(
        outcome,
        PermissionReply::Once | PermissionReply::Unattended
    ));
    assert_eq!(reply.is_ok(), outcome == PermissionReply::Once);
    let replies = h
        .store
        .read_events(&id, -1, 500)
        .unwrap()
        .events
        .into_iter()
        .filter(|e| e.kind == "permission.replied.1")
        .count();
    assert_eq!(replies, 1);
    assert!(h.runtime.pending_requests(Some(&id)).is_empty());
}

#[tokio::test(start_paused = true)]
async fn web_data_streams_cannot_extend_a_lease_but_legacy_streams_still_can_attend() {
    let ready = manual_time();
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let directory = h.repo.display().to_string();
    let api = EmbeddedClient::new(http::router(state(&h, h.runtime.clone())));
    let web_instance = api
        .request(
            Request::builder()
                .uri("http://cyber.internal/api/v1/event?scope=all")
                .header("x-cyber-client", "web")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    let web_session = api
        .request(
            Request::builder()
                .uri(format!("http://cyber.internal/api/v1/sessions/{id}/events"))
                .header("x-cyber-client", "web")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    h.runtime
        .add_interactive_client(&id, &directory)
        .await
        .unwrap();
    let question = ask(&h.runtime, &id).await;
    pending(&h.runtime, &id).await;
    tokio::time::advance(Duration::from_secs(45)).await;
    assert_eq!(question.await.unwrap(), PermissionReply::Unattended);
    h.runtime
        .add_interactive_client(&id, &directory)
        .await
        .unwrap();
    let legacy = api
        .request(
            Request::builder()
                .uri(format!("http://cyber.internal/api/v1/sessions/{id}/events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    let question = ask(&h.runtime, &id).await;
    pending(&h.runtime, &id).await;
    tokio::time::advance(Duration::from_secs(45)).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(!question.is_finished());
    drop(legacy);
    assert_eq!(question.await.unwrap(), PermissionReply::Unattended);
    drop(web_instance);
    drop(web_session);
    ready.abort();
}

#[tokio::test(start_paused = true)]
async fn expiry_does_not_cancel_an_unrelated_executing_tool() {
    use cyber_server::runtime::{Admission, CallStatus, Delivery};
    use support::{Behavior, text, tools};
    let ready = manual_time();
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![tools(&[("running", "shell", "{}")]), text("finished")],
        )],
        ..Setup::default()
    });
    h.tools.set("shell", Behavior::Gated);
    let id = h.session().await;
    h.runtime
        .add_interactive_client(&id, &h.repo.display().to_string())
        .await
        .unwrap();
    h.runtime
        .admit(&id, Admission::text("run", Delivery::Steer))
        .await
        .unwrap();
    h.tools.started.notified().await;
    let request = ask(&h.runtime, &id).await;
    pending(&h.runtime, &id).await;
    tokio::time::advance(Duration::from_secs(45)).await;
    assert_eq!(request.await.unwrap(), PermissionReply::Unattended);
    assert!(h.runtime.is_running(&id));
    assert_eq!(
        h.state(&id).await.calls["running"].status,
        CallStatus::Dispatched
    );
    h.tools.release.notify_one();
    h.settle(&id).await;
    assert_eq!(h.state(&id).await.calls["running"].status, CallStatus::Ok);
    ready.abort();
}

#[tokio::test]
async fn failed_capacity_attachment_settles_its_newly_managed_pending_ask() {
    let h = Harness::new(Setup::default());
    let directory = h.repo.display().to_string();
    for _ in 0..8 {
        let id = h.session().await;
        for _ in 0..16 {
            h.runtime
                .add_interactive_client(&id, &directory)
                .await
                .unwrap();
        }
    }
    let id = h.session().await;
    let request = ask(&h.runtime, &id).await;
    pending(&h.runtime, &id).await;
    assert!(matches!(
        h.runtime.add_interactive_client(&id, &directory).await,
        Err(InteractiveClientError::Limit)
    ));
    assert_eq!(request.await.unwrap(), PermissionReply::Unattended);
}

#[tokio::test]
async fn browser_watcher_does_not_own_the_runtime_after_all_hosts_drop() {
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    h.runtime
        .add_interactive_client(&id, &h.repo.display().to_string())
        .await
        .unwrap();
    let weak = h.runtime.downgrade();
    drop(h);
    for _ in 0..1000 {
        if weak.upgrade().is_none() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("browser expiry watcher retained the runtime");
}

#[tokio::test]
async fn browser_attachment_never_reopens_an_api_workflows_rejected_headless_ask() {
    use cyber_server::runtime::{RunStarter, StartWorkflowRun};
    let h = Harness::new(Setup::default());
    let owner = h.session().await;
    let run = h
        .runtime
        .start_workflow_run(StartWorkflowRun {
            module: cyber_workflow::WorkflowModule::inline("export const meta = { name: 'headless', description: 'Fixture' }; export default async () => null;")
                .unwrap(),
            args: Value::Null,
            session_id: owner.clone(),
            limits: cyber_workflow::Limits::default(),
            tiers: Default::default(),
            budget: cyber_core::budget::RunBudget::resolve(&json!({}), &[]).unwrap(),
            share: Default::default(),
            starter: RunStarter::Api,
            tools: vec![],
        })
        .await
        .unwrap();
    assert!(!run.interactive);
    let child = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            parent_id: Some(owner.clone()),
            workflow_run_id: Some(run.id),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    assert_eq!(
        ask(&h.runtime, &child).await.await.unwrap(),
        PermissionReply::Unattended
    );
    h.runtime
        .add_interactive_client(&owner, &h.repo.display().to_string())
        .await
        .unwrap();
    h.runtime
        .add_interactive_client(&child, &h.repo.display().to_string())
        .await
        .unwrap();
    assert_eq!(
        ask(&h.runtime, &child).await.await.unwrap(),
        PermissionReply::Unattended
    );
    assert!(h.runtime.pending_requests(Some(&child)).is_empty());
}
