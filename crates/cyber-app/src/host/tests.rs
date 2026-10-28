use super::*;
use crate::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use cyber_server::runtime::Asker;
use serde_json::json;
use tokio::sync::mpsc;

/// How long a client test waits for an RPC frame or a hook's marker before failing as
/// hung. Both follow a hook command's process spawn, which on a CI runner saturated by
/// the suite's start can take seconds; the waits themselves are on the real event.
const HOOK_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

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
        mcp_credentials: None,
        remote_skill_fetcher: None,
        workflow_cores: None,
        // These hosts run `bypass` Sessions under `full-access`, as with the explicit flag.
        isolation: Some(cyber_tools::IsolationOptions {
            dangerously_bypass: true,
            container: false,
        }),
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
        peers: Arc::new(|_| Ok(None)),
        peer_names: Arc::new(|| Ok(Vec::new())),
    };
    let turn = TurnContext {
        session_id: "ses_test".into(),
        directory: scratch.path().display().to_string(),
        agent: "build".into(),
        mode: "bypass".into(),
        prefers_apply_patch: false,
        rules: json!(null),
        worktree: Default::default(),
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
        peers: Arc::new(|_| Ok(None)),
        peer_names: Arc::new(|| Ok(Vec::new())),
    };
    let turn = TurnContext {
        session_id: "ses_test".into(),
        directory: scratch.path().display().to_string(),
        agent: "build".into(),
        mode: "default".into(),
        prefers_apply_patch: false,
        rules: json!(null),
        worktree: Default::default(),
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
        peers: Arc::new(|_| Ok(None)),
        peer_names: Arc::new(|| Ok(Vec::new())),
    };
    let turn = TurnContext {
        session_id: "ses_test".into(),
        directory: scratch.path().display().to_string(),
        agent: "build".into(),
        mode: "default".into(),
        prefers_apply_patch: false,
        rules: json!(null),
        worktree: Default::default(),
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
            peers: Arc::new(|_| Ok(None)),
            peer_names: Arc::new(|| Ok(Vec::new())),
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
            worktree: Default::default(),
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
            frame=tokio::time::timeout(HOOK_WAIT,self.frames.recv()) => frame.unwrap().unwrap(),
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
        wait=tokio::time::timeout(HOOK_WAIT,async {while !directory.join("permission-start").exists() {tokio::time::sleep(std::time::Duration::from_millis(10)).await;}}) => wait.unwrap()
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
 result=({'protocolVersion':'2025-11-25','capabilities':{'tools':{}},'serverInfo':{'name':'lower','version':'1'},'instructions':'Lower server guidance'} if r['method']=='initialize' else {'tools':[{'name':'read','inputSchema':{'type':'object'},'annotations':{'readOnlyHint':True}}]})
 print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':result}),flush=True)
"#;
    f.configure(|config| {
        config["mcp"] =
            json!({"lower":{"type":"local","command":"/usr/bin/python3","args":["-u","-c",server],"required":true}})
    });
    let info = f.app.runtime.state(&f.turn.session_id).await.unwrap().info;
    f.host.open_location(&info);
    let ready = f
        .host
        .wait_for_required_mcp(&info, CancellationToken::new())
        .await;
    assert!(
        ready.is_ok(),
        "required MCP startup: {ready:?}; status: {:?}",
        f.app
            .host
            .mcp_status(std::path::Path::new(&f.turn.directory))
    );
    assert!(
        f.host
            .definitions(&f.turn)
            .iter()
            .any(|def| def.spec.name == "mcp__lower__read")
    );
    assert!(
        f.host.context_sources(&f.turn)["core/mcp-instructions"].contains("Lower server guidance")
    );
    // The pre-rename key is always withdrawn, so a resumed Session drops it deterministically.
    assert!(
        !f.host
            .context_sources(&f.turn)
            .contains_key("mcp/instructions")
    );
    assert_eq!(
        f.host.context_observations(&f.turn)["mcp/instructions"],
        cyber_server::runtime::ContextObservation::Absent
    );
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
    assert!(
        !f.host
            .context_sources(&f.turn)
            .contains_key("core/mcp-instructions")
    );
    assert_eq!(
        f.host.context_observations(&f.turn)["core/mcp-instructions"],
        cyber_server::runtime::ContextObservation::Absent
    );
    let definitions = f.host.definitions(&f.turn);
    let effective: Vec<_> = definitions
        .iter()
        .filter(|def| def.spec.name == "mcp__lower__read")
        .collect();
    assert_eq!(effective.len(), 1);
    assert_eq!(native.scope, cyber_server::runtime::ToolScope::Mcp);
    assert_eq!(
        effective[0].scope,
        cyber_server::runtime::ToolScope::Session
    );
    let partition = cyber_server::runtime::deferred_tools::materialize(
        definitions.clone(),
        cyber_core::config::DeferredToolSettings {
            threshold_tokens: 0,
        },
        &std::collections::BTreeSet::new(),
    )
    .unwrap();
    assert!(
        partition
            .callable
            .iter()
            .any(|tool| tool.spec.name == "mcp__lower__read")
    );
    assert!(
        partition
            .deferred
            .iter()
            .all(|tool| tool.spec.name != "mcp__lower__read")
    );
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

#[tokio::test]
async fn application_host_budgets_reminders_and_preserves_complete_overflow() {
    let f = ClientFixture::new("bypass").await;
    f.configure(|config| config["tool_output"] = json!({"max_lines":8,"max_bytes":120}));
    let text = "application file output\n".repeat(12);
    let reminder = "\n\n<system-reminder>\nUse migrations\n</system-reminder>";
    let output = f
        .host
        .finalize_skill_output(&f.turn.directory, text.clone(), reminder)
        .unwrap();
    let payload = output
        .lines()
        .filter(|line| !line.starts_with("[output truncated:"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(payload.len() <= 120, "{output}");
    assert!(payload.lines().count() <= 8, "{output}");
    assert!(output.ends_with(reminder));
    let artifacts = std::fs::read_dir(f.app.paths.data.join("tool-output"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(
        std::fs::read_to_string(artifacts[0].path()).unwrap(),
        format!("{text}{reminder}")
    );
    f.app.runtime.shutdown().await;
}

/// Method names declared in the body that follows `header` (up to its closing `}`).
fn methods(source: &str, header: &str) -> std::collections::BTreeSet<String> {
    let start = source.find(header).expect(header);
    // `lines()` strips `\r`, so this also works on CRLF (Windows) checkouts.
    source[start..]
        .lines()
        .take_while(|line| *line != "}")
        .filter_map(|line| line.trim_start().strip_prefix("fn "))
        .map(|rest| rest.split(['(', '<']).next().unwrap_or_default().to_owned())
        .collect()
}

/// A trait method with a default body that AppHost does not forward silently
/// disables the built-in host's behavior in the real app (lifecycle hooks did).
#[test]
fn app_host_forwards_every_tool_host_method() {
    let trait_methods = methods(
        include_str!("../../../cyber-server/src/runtime/host.rs"),
        "pub trait ToolHost",
    );
    let forwarded = methods(include_str!("../host.rs"), "impl ToolHost for AppHost");
    for method in [
        "claude_compat",
        "config_digest",
        "instruction_read_gate",
        "lifecycle_hooks",
        "messaging_settings",
        "peer_names",
        "peer_target",
        "permission_effect",
        "resolve_prompt_resources",
        "take_hook_effects",
        "user_input_committed",
        "team_editor",
        "team_settings",
        "create_team_session",
    ] {
        assert!(
            trait_methods.contains(method),
            "{method}: {trait_methods:?}"
        );
    }
    let missing: Vec<_> = trait_methods.difference(&forwarded).collect();
    assert!(missing.is_empty(), "AppHost does not forward {missing:?}");
}

#[test]
fn method_scan_handles_crlf_sources() {
    let source = "trait T {\r\n    fn a(&self);\r\n    fn b<X>(&self);\r\n}\r\nfn after() {}\r\n";
    let names: Vec<_> = methods(source, "trait T").into_iter().collect();
    assert_eq!(names, ["a", "b"]);
}

/// A local OpenAI-compatible provider that answers every request with `ok` and keeps the
/// request bodies.
async fn recording_provider() -> (String, Arc<std::sync::Mutex<Vec<serde_json::Value>>>) {
    type Bodies = Arc<std::sync::Mutex<Vec<serde_json::Value>>>;
    async fn complete(
        axum::extract::State(bodies): axum::extract::State<Bodies>,
        axum::Json(body): axum::Json<serde_json::Value>,
    ) -> axum::response::Response {
        bodies.lock().unwrap().push(body);
        let chunk = json!({"choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":10,"completion_tokens":1}});
        axum::response::Response::builder()
            .header("content-type", "text/event-stream")
            .body(axum::body::Body::from(format!(
                "data: {chunk}\n\ndata: [DONE]\n\n"
            )))
            .unwrap()
    }
    let bodies = Bodies::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let router = axum::Router::new()
        .route("/v1/chat/completions", axum::routing::post(complete))
        .with_state(bodies.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (url, bodies)
}

/// The system text of each Turn request (title requests left out), in order.
fn turn_systems(bodies: &std::sync::Mutex<Vec<serde_json::Value>>) -> Vec<String> {
    bodies
        .lock()
        .unwrap()
        .iter()
        .filter(|body| !body["messages"].to_string().contains("Write a short title"))
        .map(|body| body["messages"][0]["content"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn base_prompt_follows_the_model_family() {
    use cyber_core::prompts::{ModelFamily, base_prompt};
    use cyber_server::runtime::{Admission, Delivery};
    let (url, bodies) = recording_provider().await;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    std::fs::create_dir_all(directory.join("config")).unwrap();
    let config = json!({"permissions":"allow","providers":{"lab":{
        "api":{"type":"openai-compatible","url":url,"settings":{"auth":"none"}},
        "models":{"claude-sonnet":{},"qwen3":{}}}}});
    std::fs::write(directory.join("config/cyber.json"), config.to_string()).unwrap();
    let app = application(&directory).await;
    let create = |model: &str| cyber_server::runtime::CreateSession {
        directory: directory.display().to_string(),
        model: model.into(),
        ..Default::default()
    };
    let turn = async |id: &str, text: &str| {
        let admission = Admission::text(text, Delivery::Queue);
        app.runtime.admit(id, admission).await.unwrap();
        let idle = app.runtime.wait_idle(id);
        tokio::time::timeout(std::time::Duration::from_secs(20), idle)
            .await
            .expect("the session never settled");
    };
    let session = app
        .runtime
        .create_session(create("lab/claude-sonnet"))
        .await
        .unwrap();
    turn(&session.id, "one").await;
    app.runtime
        .switch_model(&session.id, "lab/qwen3")
        .await
        .unwrap();
    turn(&session.id, "two").await;
    let systems = turn_systems(&bodies);
    let anthropic = format!("{}\n\n", base_prompt(ModelFamily::Anthropic));
    let local = format!("{}\n\n", base_prompt(ModelFamily::Local));
    assert!(systems[0].starts_with(&anthropic), "{}", systems[0]);
    assert!(systems[1].starts_with(&local), "{}", systems[1]);
    let page = app.store.read_events(&session.id, -1, 500).unwrap();
    let epochs: Vec<_> = page
        .events
        .iter()
        .filter(|event| event.kind == "session.context.epoch_started.1")
        .map(|event| (event.data["epoch"].clone(), event.data["family"].clone()))
        .collect();
    assert_eq!(
        epochs,
        [(json!(1), json!("anthropic")), (json!(2), json!("local"))]
    );

    // An agent's system text replaces the base prompt, whatever the family.
    std::fs::write(
        directory.join("config/cyber.json"),
        json!({"permissions":"allow","providers":config["providers"],
            "agents":{"build":{"system":"Agent instructions."}}})
        .to_string(),
    )
    .unwrap();
    let agent = app
        .runtime
        .create_session(create("lab/claude-sonnet"))
        .await
        .unwrap();
    turn(&agent.id, "three").await;
    let systems = turn_systems(&bodies);
    let last = systems.last().unwrap();
    assert!(last.starts_with("Agent instructions.\n\n"), "{last}");
    assert!(!last.contains("You are Cyber Code"), "{last}");
    app.runtime.shutdown().await;
}

#[tokio::test]
async fn app_host_forwards_committed_user_input_to_file_observer() {
    use cyber_core::hooks::file_observations as records;
    let scratch = tempfile::tempdir().unwrap();
    let app = application(scratch.path()).await;
    // Owner state intentionally lives below this Location, exercising the
    // production storage exclusions as well as the facade notification.
    let project = scratch.path().canonicalize().unwrap();
    // No handler matches the marker: the test exercises the real runtime/host
    // bridge and journal without launching a command or making a model call.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    std::fs::write(app.paths.config.join("cyber.json"), json!({"hooks":{"FileChanged":[{
        "matcher":"never-observed", "hooks":[{"type":"http", "url":format!("http://{}/unused", listener.local_addr().unwrap())}]
    }]}}).to_string()).unwrap();
    let private_roots = [
        &app.paths.data,
        &app.paths.config,
        &app.paths.state,
        &app.paths.cache,
        &app.paths.tmp,
    ];
    for root in private_roots {
        std::fs::write(
            root.join("observer-private-state"),
            "initial private metadata",
        )
        .unwrap();
    }
    let session = app
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: project.display().to_string(),
            model: "test/unavailable".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    wait_for_observer_record(&app, records::CHECKPOINT, false).await;
    for root in private_roots {
        std::fs::write(
            root.join("observer-private-state"),
            "changed private metadata must remain excluded",
        )
        .unwrap();
    }
    // The private cache keeps its identity after a rename inside the Location.
    // A path-only exclusion would publish its file metadata under the new name.
    std::fs::rename(&app.paths.cache, project.join("relocated-private-cache")).unwrap();
    std::fs::write(project.join("observer-marker"), "changed").unwrap();
    wait_for_observer_record(&app, records::CHANGED, false).await;
    let mut input = cyber_server::runtime::Admission::text(
        "resume observers",
        cyber_server::runtime::Delivery::Hold,
    );
    input.resume = false;
    app.runtime.admit(&session.id, input).await.unwrap();
    wait_for_observer_record(&app, records::CHANGED, true).await;
    let private: i64 = app.store.read(|db| Ok(db.query_row(
        "SELECT COUNT(*) FROM event e, json_each(e.data,'$.updates') u WHERE e.type=?1 AND json_extract(u.value,'$.path') LIKE '%/observer-private-state'",
        [records::CHANGED], |row| row.get(0))?)).unwrap();
    assert_eq!(
        private, 0,
        "nested owner state must not enter observation deltas"
    );
    let baselines: i64 = app.store.read(|db| Ok(db.query_row(
        "SELECT COUNT(*) FROM event e, json_each(e.data,'$.files') f WHERE e.type=?1 AND f.key LIKE '%/observer-private-state'",
        [records::CHECKPOINT], |row| row.get(0))?)).unwrap();
    assert_eq!(
        baselines, 0,
        "nested owner state must not enter observer baselines"
    );
    app.runtime.shutdown().await;
}

async fn wait_for_observer_record(app: &App, kind: &'static str, renewal: bool) {
    tokio::time::timeout(HOOK_WAIT, async {
        loop {
            let count: i64 = app.store.read(move |db| {
                let query = if renewal {
                    "SELECT COUNT(*) FROM event WHERE type=?1 AND json_type(data,'$.renewal')='object'"
                } else { "SELECT COUNT(*) FROM event WHERE type=?1" };
                Ok(db.query_row(query, [kind], |row| row.get(0))?)
            }).unwrap();
            if count > 0 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }).await.expect("application observer notification did not reach its durable journal");
}
