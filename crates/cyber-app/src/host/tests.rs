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
    let mut denied = invocation(&turn);
    denied.registration = host.remote.definitions()[0].registration.clone();
    let outcome = host.execute(denied, CancellationToken::new()).await;
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

struct ClientFixture {
    _root: tempfile::TempDir,
    app: App,
    host: AppHost,
    turn: TurnContext,
    def: ToolDef,
    frames: mpsc::Receiver<serde_json::Value>,
    #[cfg(unix)]
    out: mpsc::Sender<serde_json::Value>,
    #[cfg(unix)]
    owner: u64,
}

impl ClientFixture {
    async fn new(mode: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let app = application(&root.path().canonicalize().unwrap()).await;
        let config = json!({"sandbox":{"policy":"full-access"},"providers":{"test":{"api":{"type":"openai-compatible","url":"http://127.0.0.1:9/v1","settings":{"auth":"none"}},"models":{"main":{}}}}});
        std::fs::write(app.paths.config.join("cyber.json"), config.to_string()).unwrap();
        let info = app
            .runtime
            .create_session(cyber_server::runtime::CreateSession {
                directory: root.path().display().to_string(),
                model: "test/main".into(),
                mode: Some(mode.into()),
                ..Default::default()
            })
            .await
            .unwrap();
        let host = AppHost {
            builtin: app.host.clone(),
            remote: app.state.remote_tools.clone(),
        };
        let (out, frames) = mpsc::channel(4);
        let owner = host.remote.owner();
        host.remote.register(owner,out.clone(),&json!({"name":"client_write","input":{"type":"object","properties":{"value":{"type":"string"}}}}),&[]).unwrap();
        let def = host.remote.definitions()[0].clone();
        let turn = TurnContext {
            session_id: info.id,
            directory: info.directory,
            agent: info.agent,
            mode: info.mode,
            prefers_apply_patch: false,
            rules: info.rules,
        };
        Self {
            _root: root,
            app,
            host,
            turn,
            def,
            frames,
            #[cfg(unix)]
            out,
            #[cfg(unix)]
            owner,
        }
    }
    fn call(&self) -> Invocation {
        let mut call = invocation(&self.turn);
        call.registration = self.def.registration.clone();
        call
    }
    fn configure(&self, update: impl FnOnce(&mut serde_json::Value)) {
        let path = self.app.paths.config.join("cyber.json");
        let mut config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        update(&mut config);
        std::fs::write(path, config.to_string()).unwrap();
    }
    async fn reply(
        &mut self,
        call: Invocation,
        reply: serde_json::Value,
    ) -> (ToolOutcome, serde_json::Value) {
        self.exchange(call, json!({"result":reply})).await
    }

    async fn exchange(
        &mut self,
        call: Invocation,
        mut reply: serde_json::Value,
    ) -> (ToolOutcome, serde_json::Value) {
        let mut running = self.host.execute(call, CancellationToken::new());
        let frame = tokio::select! {
            result=&mut running => panic!("client RPC refused unexpectedly: {result:?}"),
            frame=tokio::time::timeout(std::time::Duration::from_secs(3),self.frames.recv()) => frame.unwrap().unwrap(),
        };
        reply["id"] = frame["id"].clone();
        self.host.remote.resolve(&reply);
        (running.await, frame)
    }
    fn no_rpc(&mut self) {
        assert_eq!(
            self.frames.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        );
    }
}

#[tokio::test]
async fn client_calls_default_to_ask_and_obey_mode_and_current_explicit_denials() {
    let mut f = ClientFixture::new("default").await;
    assert!(matches!(
        f.host.execute(f.call(), CancellationToken::new()).await,
        ToolOutcome::Failed(_)
    ));
    f.no_rpc();
    f.turn.mode = "plan".into();
    assert!(
        f.app
            .state
            .services
            .tools(&f.turn)
            .iter()
            .all(|tool| tool.spec.name != "client_write")
    );
    assert!(
        f.host
            .definitions(&f.turn)
            .iter()
            .all(|tool| tool.spec.name != "client_write")
    );
    assert!(matches!(
        f.host.execute(f.call(), CancellationToken::new()).await,
        ToolOutcome::Failed(_)
    ));
    f.no_rpc();
    f.turn.mode = "bypass".into();
    f.configure(|config| config["permissions"] = json!({"client_write":"deny"}));
    assert!(
        f.app
            .state
            .services
            .tools(&f.turn)
            .iter()
            .all(|tool| tool.spec.name != "client_write")
    );
    assert!(
        f.host
            .definitions(&f.turn)
            .iter()
            .all(|tool| tool.spec.name != "client_write")
    );
    assert!(matches!(
        f.host.execute(f.call(), CancellationToken::new()).await,
        ToolOutcome::Failed(_)
    ));
    f.no_rpc();
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn client_schema_and_cancellation_checks_precede_rpc_effects() {
    let mut f = ClientFixture::new("bypass").await;
    let mut call = f.call();
    call.input = json!({"value":7});
    assert!(matches!(
        f.host.execute(call, CancellationToken::new()).await,
        ToolOutcome::Failed(_)
    ));
    f.no_rpc();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(f.host.execute(f.call(), cancel).await, ToolOutcome::Aborted);
    f.no_rpc();
    f.app.runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn client_pre_hook_rewrites_rpc_input_and_post_hook_receipt_tracks_the_result() {
    let mut f = ClientFixture::new("bypass").await;
    f.configure(|config|config["hooks"]=json!({
        "PreToolUse":[{"matcher":"client_write","hooks":[{"type":"command","command":"cat >/dev/null; printf '%s' '{\"updated_input\":{\"value\":\"rewritten\"}}'"}]}],
        "PostToolUse":[{"matcher":"client_write","hooks":[{"type":"command","command":"cat > client-post.json"}]}]
    }));
    let call = f.call();
    let (outcome, frame) = f.reply(call, json!("client result")).await;
    assert_eq!(outcome, ToolOutcome::Ok("client result".into()));
    assert_eq!(frame["params"]["input"], json!({"value":"rewritten"}));
    let event: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(std::path::Path::new(&f.turn.directory).join("client-post.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(event["event"], "PostToolUse");
    assert_eq!(
        f.app
            .runtime
            .hook_executions(&f.turn.session_id, 10)
            .unwrap()
            .len(),
        2
    );
    f.app.runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn client_permission_hook_answers_one_ask_without_bypassing_hard_denials() {
    let mut f = ClientFixture::new("default").await;
    f.configure(|config|config["hooks"]=json!({"PermissionRequest":[{"matcher":"client_write","hooks":[{"type":"command","command":"cat >/dev/null; printf '%s' '{\"decision\":\"allow\"}'"}]}]}));
    let call = f.call();
    assert_eq!(
        f.reply(call, json!("allowed once")).await.0,
        ToolOutcome::Ok("allowed once".into())
    );
    assert!(
        cyber_tools::permissions::saved::list(
            &f.app.store,
            Some(std::path::Path::new(&f.turn.directory))
        )
        .unwrap()
        .is_empty()
    );
    assert_eq!(
        f.app
            .runtime
            .hook_executions(&f.turn.session_id, 10)
            .unwrap()
            .len(),
        1
    );
    f.configure(|config| config["permissions"] = json!({"client_write":"deny"}));
    assert!(matches!(
        f.host.execute(f.call(), CancellationToken::new()).await,
        ToolOutcome::Failed(_)
    ));
    f.no_rpc();
    assert_eq!(
        f.app
            .runtime
            .hook_executions(&f.turn.session_id, 10)
            .unwrap()
            .len(),
        1
    );
    f.app.runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn client_replacement_during_permission_hook_cannot_receive_the_approved_old_call() {
    let mut f = ClientFixture::new("default").await;
    f.configure(|config|config["hooks"]=json!({"PermissionRequest":[{"matcher":"client_write","hooks":[{"type":"command","command":"cat >/dev/null; touch permission-start; while [ ! -f permission-release ]; do sleep 0.01; done; printf '%s' '{\"decision\":\"allow\"}'"}]}]}));
    let call = f.call();
    let running = f.host.execute(call, CancellationToken::new());
    tokio::pin!(running);
    let directory = std::path::Path::new(&f.turn.directory);
    tokio::select! {
        result=&mut running => panic!("expected pending permission hook, got {result:?}"),
        wait=tokio::time::timeout(std::time::Duration::from_secs(3),async {while !directory.join("permission-start").exists() {tokio::time::sleep(std::time::Duration::from_millis(10)).await;}}) => wait.unwrap()
    }
    f.host
        .remote
        .register(
            f.owner,
            f.out.clone(),
            &json!({"name":"client_write","input":{"type":"object"}}),
            &[],
        )
        .unwrap();
    std::fs::write(directory.join("permission-release"), "release").unwrap();
    assert_eq!(
        running.await,
        ToolOutcome::Failed("Stale tool call: client_write".into())
    );
    assert_eq!(f.frames.try_recv(), Err(mpsc::error::TryRecvError::Empty));
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn client_oversized_success_keeps_full_output_in_managed_artifacts() {
    let mut f = ClientFixture::new("bypass").await;
    f.configure(|config| config["tool_output"] = json!({"max_lines":2,"max_bytes":64}));
    let full = "client output line\n".repeat(100);
    let call = f.call();
    let (outcome, _) = f.reply(call, json!(full)).await;
    let ToolOutcome::Ok(output) = outcome else {
        panic!("expected bounded success")
    };
    assert!(output.contains("output truncated"));
    let artifact = std::fs::read_dir(f.app.paths.data.join("tool-output"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(std::fs::read_to_string(artifact).unwrap(), full);
    f.app.runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn client_failures_are_bounded_and_emit_failure_hooks_with_durable_receipts() {
    let mut f = ClientFixture::new("bypass").await;
    f.configure(|config| {
        config["tool_output"]=json!({"max_lines":2,"max_bytes":64});
        config["hooks"]=json!({"PostToolUseFailure":[{"matcher":"client_write","hooks":[{"type":"command","command":"cat > client-failure.json"}]}]});
    });
    let full = "client failure line\n".repeat(100);
    let call = f.call();
    let (outcome, _) = f.exchange(call, json!({"error":{"message":full}})).await;
    let ToolOutcome::Failed(output) = outcome else {
        panic!("expected bounded failure")
    };
    assert!(output.contains("output truncated"));
    let artifact = std::fs::read_dir(f.app.paths.data.join("tool-output"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(std::fs::read_to_string(artifact).unwrap(), full);
    let event: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(&f.turn.directory).join("client-failure.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(event["event"], "PostToolUseFailure");
    assert_eq!(
        f.app
            .runtime
            .hook_executions(&f.turn.session_id, 10)
            .unwrap()
            .len(),
        1
    );
    f.app.runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn hidden_client_override_does_not_reveal_a_lower_read_only_mcp_registration() {
    let mut f = ClientFixture::new("default").await;
    let server = r#"import json,sys
for line in sys.stdin:
 r=json.loads(line)
 if 'id' not in r: continue
 result=({'protocolVersion':'2025-11-25','capabilities':{'tools':{}},'serverInfo':{'name':'lower','version':'1'}} if r['method']=='initialize' else {'tools':[{'name':'read','inputSchema':{'type':'object'},'annotations':{'readOnlyHint':True}}]})
 print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':result}),flush=True)
"#;
    f.configure(|config| {
        config["mcp"] =
            json!({"lower":{"type":"local","command":"/usr/bin/python3","args":["-u","-c",server]}})
    });
    let info = f.app.runtime.state(&f.turn.session_id).await.unwrap().info;
    f.host.open_location(&info);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !f
            .host
            .definitions(&f.turn)
            .iter()
            .any(|def| def.spec.name == "mcp__lower__read")
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let native = f
        .host
        .definitions(&f.turn)
        .into_iter()
        .find(|def| def.spec.name == "mcp__lower__read")
        .unwrap();
    f.host
        .remote
        .register(
            f.owner,
            f.out.clone(),
            &json!({"name":"mcp__lower__read","input":{"type":"object"}}),
            &[],
        )
        .unwrap();
    let definitions = f.host.definitions(&f.turn);
    let effective: Vec<_> = definitions
        .iter()
        .filter(|def| def.spec.name == "mcp__lower__read")
        .collect();
    assert_eq!(effective.len(), 1);
    assert_ne!(effective[0].registration, native.registration);
    let mut old = f.call();
    old.name = native.spec.name;
    old.registration = native.registration;
    assert_eq!(
        f.host.execute(old, CancellationToken::new()).await,
        ToolOutcome::Failed("Stale tool call: mcp__lower__read".into())
    );
    f.no_rpc();
    f.turn.mode = "plan".into();
    assert!(
        f.host
            .definitions(&f.turn)
            .iter()
            .all(|def| def.spec.name != "mcp__lower__read")
    );
    f.app.runtime.shutdown().await;
}
