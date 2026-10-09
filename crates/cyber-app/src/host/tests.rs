use super::*;
use crate::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use cyber_server::runtime::Asker;
use serde_json::json;
use tokio::sync::mpsc;

async fn application(root: &std::path::Path) -> App {
    let paths = Paths {
        data: root.join("data"),
        config: root.join("config"),
        state: root.join("state"),
        cache: root.join("cache"),
        tmp: root.join("tmp"),
    };
    paths.ensure().unwrap();
    App::build(AppOptions {
        paths,
        home: root.join("home"),
        database: DatabaseLocation::Memory,
        default_directory: root.to_path_buf(),
        sandbox_policy: None,
        snapshots: false,
        interactive: false,
        password: None,
    })
    .await
    .unwrap()
}

fn invocation(turn: &TurnContext) -> Invocation {
    Invocation {
        registration: None,
        session_id: turn.session_id.clone(),
        directory: turn.directory.clone(),
        agent: turn.agent.clone(),
        mode: turn.mode.clone(),
        message_id: "msg_test".into(),
        call_id: "call_test".into(),
        name: "client_write".into(),
        input: json!({}),
        attempt: 1,
        operation_key: "op_test".into(),
        asker: Asker::detached(),
        rules: json!(null),
    }
}

#[tokio::test]
async fn agent_tool_restrictions_refuse_client_dispatch_without_sending_a_request() {
    let scratch = tempfile::tempdir().unwrap();
    let app = application(scratch.path()).await;
    let host = AppHost {
        builtin: app.host.clone(),
        remote: app.state.remote_tools.clone(),
    };
    let turn = TurnContext {
        session_id: "ses_test".into(),
        directory: scratch.path().display().to_string(),
        agent: "build".into(),
        mode: "bypass".into(),
        prefers_apply_patch: false,
        rules: json!(null),
    };
    let (out, mut frames) = mpsc::channel(4);
    host.remote
        .register(
            host.remote.owner(),
            out,
            &json!({"name":"client_write","input":{"type":"object"}}),
            &[],
        )
        .unwrap();
    assert!(
        host.definitions(&turn)
            .iter()
            .any(|def| def.spec.name == "client_write")
    );
    // A real client request/reply first proves the registered dispatch route works.
    let mut call = invocation(&turn);
    call.registration = host.remote.definitions()[0].registration.clone();
    let mut running = host.execute(call, CancellationToken::new());
    let frame = tokio::select! {
        outcome = &mut running => panic!("client dispatch ended before its reply: {outcome:?}"),
        frame = tokio::time::timeout(std::time::Duration::from_secs(2), frames.recv()) => frame.unwrap().unwrap(),
    };
    host.remote
        .resolve(&json!({"id":frame["id"],"result":"client result"}));
    assert_eq!(running.await, ToolOutcome::Ok("client result".into()));
    std::fs::write(
        app.paths.config.join("cyber.json"),
        r#"{"agents":{"build":{"tools":{"deny":["client_*"]}}}}"#,
    )
    .unwrap();
    assert!(
        !host
            .definitions(&turn)
            .iter()
            .any(|def| def.spec.name == "client_write")
    );
    assert!(
        !app.state
            .services
            .tools(&turn)
            .iter()
            .any(|def| def.spec.name == "client_write")
    );
    let outcome = host
        .execute(invocation(&turn), CancellationToken::new())
        .await;
    assert!(
        matches!(outcome, ToolOutcome::Failed(error) if error.contains("client_write") && error.contains("build"))
    );
    assert_eq!(frames.try_recv(), Err(mpsc::error::TryRecvError::Empty));
    assert!(host.remote.has("client_write"));
}

#[tokio::test]
async fn app_host_preserves_typed_agent_context_observations() {
    let scratch = tempfile::tempdir().unwrap();
    let app = application(scratch.path()).await;
    let host = AppHost {
        builtin: app.host.clone(),
        remote: app.state.remote_tools.clone(),
    };
    let turn = TurnContext {
        session_id: "ses_test".into(),
        directory: scratch.path().display().to_string(),
        agent: "build".into(),
        mode: "default".into(),
        prefers_apply_patch: false,
        rules: json!(null),
    };
    let path = app.paths.config.join("cyber.json");
    std::fs::write(
        &path,
        r#"{"agents":{"build":{"system":"App agent instructions."}}}"#,
    )
    .unwrap();
    assert_eq!(
        host.context_observations(&turn)["core/agent"],
        cyber_server::runtime::ContextObservation::Value("App agent instructions.".into())
    );
    std::fs::write(&path, "{}").unwrap();
    assert_eq!(
        host.context_observations(&turn)["core/agent"],
        cyber_server::runtime::ContextObservation::Absent
    );
    std::fs::write(&path, r#"{"agents":{"build":{"steps":0}}}"#).unwrap();
    assert!(matches!(
        host.context_observations(&turn)["core/agent"],
        cyber_server::runtime::ContextObservation::Unavailable(_)
    ));
}

#[tokio::test]
async fn app_host_delegates_validated_agent_request_options() {
    let scratch = tempfile::tempdir().unwrap();
    let app = application(scratch.path()).await;
    let host = AppHost {
        builtin: app.host.clone(),
        remote: app.state.remote_tools.clone(),
    };
    let turn = TurnContext {
        session_id: "ses_test".into(),
        directory: scratch.path().display().to_string(),
        agent: "build".into(),
        mode: "default".into(),
        prefers_apply_patch: false,
        rules: json!(null),
    };
    let path = app.paths.config.join("cyber.json");
    std::fs::write(&path, r#"{"agents":{"build":{"request":{"headers":{"X-Agent":"build"},"body":{"temperature":0.2}}}}}"#).unwrap();
    let overlay = host.request_overlay(&turn).unwrap();
    assert_eq!(overlay.body["temperature"], 0.2);
    assert_eq!(overlay.headers["X-Agent"], "build");
    std::fs::write(&path, r#"{"agents":{"build":{"request":{"body":false}}}}"#).unwrap();
    assert!(host.request_overlay(&turn).is_err());
    assert!(host.agent_inference(&turn).is_err());
    std::fs::write(
        &path,
        r#"{"agents":{"build":{"model":"test/other#high","variant":"low","permission_mode":"plan","steps":4}}}"#,
    )
    .unwrap();
    let options = host.agent_inference(&turn).unwrap();
    assert_eq!(options.model.as_deref(), Some("test/other#high"));
    assert_eq!(options.variant.as_deref(), Some("low"));
    assert_eq!(options.permission_mode.as_deref(), Some("plan"));
    assert_eq!(options.steps, Some(4));
}

#[tokio::test]
async fn app_host_subtask_delegation_preserves_user_deny_rules() {
    let scratch = tempfile::tempdir().unwrap();
    let app = application(scratch.path()).await;
    let parent = app
        .state
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: scratch.path().display().to_string(),
            model: "test/unavailable".into(),
            mode: Some("bypass".into()),
            rules: Some(json!({"agent":"deny"})),
            ..Default::default()
        })
        .await
        .unwrap();
    let error = app
        .state
        .runtime
        .subtask(&parent.id, "try another approach")
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("Permission denied"), "{error}");
    let named_error = app
        .state
        .runtime
        .subtask_with_agent(&parent.id, "inspect retry logic", Some("explore".into()))
        .await
        .unwrap_err()
        .to_string();
    assert!(named_error.contains("Permission denied"), "{named_error}");
    assert!(app.state.runtime.jobs(Some(&parent.id)).unwrap().is_empty());
}
