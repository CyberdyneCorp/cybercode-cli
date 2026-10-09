//! Configured MCP behind the actual host and runtime, with real stdio servers.
#![cfg(unix)]
mod support;
use cyber_core::config::{self, LoadRequest};
use cyber_core::paths::Paths;
use cyber_core::trust::TrustStore;
use cyber_server::runtime::{
    McpConnectionPhase, ToolDef, ToolHost, ToolOutcome, TurnContext, mcp_connections,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use support::flow::{Flow, call, text};
use tokio_util::sync::CancellationToken;

const SERVER: &str = r#"
import json,sys,pathlib,time,os
with open('spawned','a') as f: f.write(str(os.getpid())+'\n')
for line in sys.stdin:
    request=json.loads(line)
    if 'id' not in request: continue
    method=request['method']
    if method=='initialize':
        while not pathlib.Path('ready').exists(): time.sleep(.01)
        result={'protocolVersion':'2025-11-25','capabilities':{'tools':{}},'serverInfo':{'name':'real','version':'1'}}
    elif method=='tools/list':
        result={'tools':[{'name':name,'inputSchema':{'type':'object'},'annotations':{'readOnlyHint':name!='write'}} for name in ['read','write','structured','error']]}
    else:
        name=request['params']['name']
        with open('calls','a') as f: f.write(name+'\n')
        while pathlib.Path('block-call').exists(): time.sleep(.01)
        result={'content':[{'type':'text','text':'remote '+name}], 'isError':name=='error'}
        if name=='structured': result['structuredContent']={'answer':42}
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}),flush=True)
"#;

fn configure(flow: &Flow, hooks: Value) -> PathBuf {
    let home = flow.f.dir.path().to_path_buf();
    let env = HashMap::from([(
        "CYBER_HOME".into(),
        home.join("cyber").display().to_string(),
    )]);
    let paths = Paths::resolve(&env, &home);
    paths.ensure().unwrap();
    let path = paths.config.join("cyber.jsonc");
    std::fs::write(&path, json!({"sandbox":{"policy":"full-access"}, "hooks":hooks, "mcp":{"shared":{"type":"local","command":"/usr/bin/python3","args":["-u","-c",SERVER]}}}).to_string()).unwrap();
    let resolve = Arc::new(move |directory: &std::path::Path| {
        config::load(&LoadRequest {
            location: directory,
            paths: &paths,
            env: &env,
            home: &home,
            profile: None,
            overrides: &[],
            flags: json!({}),
        })
        .map_err(|error| error.to_string())
    });
    flow.f.set_config(resolve(&flow.f.repo).unwrap().value);
    flow.f
        .host
        .attach_hook_config(
            resolve,
            TrustStore::new(flow.f.dir.path().join("trust.json")),
        )
        .unwrap();
    path
}
fn turn(flow: &Flow, id: &str) -> TurnContext {
    TurnContext {
        session_id: id.into(),
        directory: flow.f.repo.display().to_string(),
        agent: "build".into(),
        mode: "default".into(),
        prefers_apply_patch: false,
        rules: Value::Null,
    }
}
async fn ready(flow: &Flow, id: &str) -> Vec<ToolDef> {
    flow.f.write("ready", "ready");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let defs: Vec<_> = flow
                .f
                .host
                .definitions(&turn(flow, id))
                .into_iter()
                .filter(|def| def.spec.name.starts_with("mcp__"))
                .collect();
            if !defs.is_empty() {
                return defs;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
async fn invoke(flow: &Flow, id: &str, def: &ToolDef, mode: &str) -> ToolOutcome {
    let mut inv = flow.f.invocation(mode, &def.spec.name, json!({}));
    inv.session_id = id.into();
    inv.registration = def.registration.clone();
    flow.f.host.execute(inv, CancellationToken::new()).await
}

#[tokio::test]
async fn slow_startup_does_not_block_turns_and_sessions_share_one_connection() {
    let flow = Flow::new(
        vec![
            text("first"),
            call("c1", "mcp__shared__read", json!({})),
            text("done"),
        ],
        false,
    );
    configure(&flow, json!({}));
    let id = tokio::time::timeout(Duration::from_secs(3), flow.session("default"))
        .await
        .unwrap();
    let second = flow.session("default").await;
    assert!(
        flow.f
            .host
            .definitions(&turn(&flow, &id))
            .iter()
            .all(|def| !def.spec.name.starts_with("mcp__"))
    );
    flow.prompt(&id, "before readiness").await;
    flow.settle(&id).await;
    assert!(
        flow.main.requests()[0]
            .tools
            .iter()
            .all(|tool| !tool.name.starts_with("mcp__"))
    );
    let defs = ready(&flow, &id).await;
    let other = flow.f.host.definitions(&turn(&flow, &second));
    assert_eq!(
        defs[0].registration,
        other
            .iter()
            .find(|tool| tool.spec.name == defs[0].spec.name)
            .unwrap()
            .registration
    );
    assert_eq!(
        mcp_connections(&flow.f.store, &flow.f.repo).unwrap().len(),
        1
    );
    assert_eq!(flow.f.read("spawned").lines().count(), 1);
    flow.prompt(&id, "after readiness").await;
    flow.settle(&id).await;
    assert_eq!(flow.output(&id, "c1").await, "remote read");
    assert_eq!(flow.f.read("calls"), "read\n");
    flow.runtime.shutdown().await;
    assert_eq!(
        mcp_connections(&flow.f.store, &flow.f.repo).unwrap()[0].phase,
        McpConnectionPhase::Settled
    );
}

#[tokio::test]
async fn permission_defaults_modes_structured_results_and_errors_use_host_boundaries() {
    let flow = Flow::new(vec![], false);
    configure(&flow, json!({}));
    let id = flow.session("default").await;
    let defs = ready(&flow, &id).await;
    let find = |name: &str| {
        defs.iter()
            .find(|def| def.spec.name == format!("mcp__shared__{name}"))
            .unwrap()
    };
    let mut plan = turn(&flow, &id);
    plan.mode = "plan".into();
    assert!(
        flow.f
            .host
            .definitions(&plan)
            .iter()
            .all(|def| def.spec.name != "mcp__shared__write")
    );
    assert_eq!(
        invoke(&flow, &id, find("read"), "default").await,
        ToolOutcome::Ok("remote read".into())
    );
    assert!(matches!(
        invoke(&flow, &id, find("write"), "default").await,
        ToolOutcome::Failed(_)
    ));
    assert!(matches!(
        invoke(&flow, &id, find("write"), "plan").await,
        ToolOutcome::Failed(_)
    ));
    assert_eq!(
        invoke(&flow, &id, find("structured"), "default").await,
        ToolOutcome::Structured {
            output: "remote structured".into(),
            value: json!({"answer":42})
        }
    );
    assert_eq!(
        invoke(&flow, &id, find("error"), "default").await,
        ToolOutcome::Failed("remote error".into())
    );
    assert_eq!(flow.f.read("calls"), "read\nstructured\nerror\n");
    let mut config = flow.f.config.lock().unwrap().clone();
    config["permissions"] = json!({"mcp__shared__read":"deny"});
    flow.f.set_config(config);
    assert!(
        flow.f
            .host
            .definitions(&turn(&flow, &id))
            .iter()
            .all(|def| def.spec.name != "mcp__shared__read")
    );
    assert!(matches!(
        invoke(&flow, &id, find("read"), "bypass").await,
        ToolOutcome::Failed(_)
    ));
    assert_eq!(flow.f.read("calls"), "read\nstructured\nerror\n");
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn fresh_definition_changes_and_forged_bindings_refuse_rpc_effects() {
    let flow = Flow::new(vec![], false);
    let path = configure(&flow, json!({}));
    let id = flow.session("default").await;
    let defs = ready(&flow, &id).await;
    let def = defs
        .iter()
        .find(|def| def.spec.name == "mcp__shared__read")
        .unwrap();
    let mut forged = def.clone();
    forged.registration = Some("mcs_foreign:read".into());
    assert_eq!(
        invoke(&flow, &id, &forged, "bypass").await,
        ToolOutcome::Failed("Stale tool call: mcp__shared__read".into())
    );
    let mut value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    value["mcp"]["shared"]["env"] = json!({"CHANGED":"yes"});
    std::fs::write(&path, value.to_string()).unwrap();
    assert!(
        flow.f
            .host
            .definitions(&turn(&flow, &id))
            .iter()
            .all(|def| !def.spec.name.starts_with("mcp__"))
    );
    assert!(matches!(
        invoke(&flow, &id, def, "bypass").await,
        ToolOutcome::Failed(_)
    ));
    assert!(!flow.f.repo.join("calls").exists());
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn pre_tool_hook_denies_mcp_before_remote_effects_and_records_its_receipt() {
    let flow = Flow::new(
        vec![call("c1", "mcp__shared__read", json!({})), text("done")],
        false,
    );
    configure(
        &flow,
        json!({"PreToolUse":[{"matcher":"mcp__shared__read","hooks":[{"type":"command","command":"echo '{\"decision\":\"deny\",\"reason\":\"MCP hook denial\"}'"}]}]}),
    );
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    flow.prompt(&id, "try MCP").await;
    flow.settle(&id).await;
    assert!(flow.output(&id, "c1").await.contains("MCP hook denial"));
    assert!(!flow.f.repo.join("calls").exists());
    assert!(!flow.runtime.hook_executions(&id, 10).unwrap().is_empty());
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn shutdown_during_initialization_joins_startup_and_keeps_unknown_fence() {
    let flow = Flow::new(vec![], false);
    configure(&flow, json!({}));
    let id = flow.session("default").await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while !flow.f.repo.join("spawned").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    flow.runtime.shutdown().await;
    assert!(
        flow.f
            .host
            .definitions(&turn(&flow, &id))
            .iter()
            .all(|def| !def.spec.name.starts_with("mcp__"))
    );
    let records = mcp_connections(&flow.f.store, &flow.f.repo).unwrap();
    assert_eq!(records[0].phase, McpConnectionPhase::Unknown);
    assert_eq!(records[0].acknowledged, Some(false));
    flow.runtime.shutdown().await;
    assert_eq!(
        mcp_connections(&flow.f.store, &flow.f.repo).unwrap(),
        records
    );
}

#[tokio::test]
async fn interrupted_call_closes_native_owner_and_removes_registration() {
    let flow = Flow::new(vec![], false);
    configure(&flow, json!({}));
    let id = flow.session("default").await;
    let defs = ready(&flow, &id).await;
    let def = defs
        .iter()
        .find(|def| def.spec.name == "mcp__shared__read")
        .unwrap();
    let mut inv = flow.f.invocation("default", &def.spec.name, json!({}));
    inv.session_id = id.clone();
    inv.registration = def.registration.clone();
    flow.f.write("block-call", "block");
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let host = flow.f.host.clone();
    let task = tokio::spawn(async move { host.execute(inv, token).await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !flow.f.repo.join("calls").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap(),
        ToolOutcome::Aborted
    );
    assert!(
        flow.f
            .host
            .definitions(&turn(&flow, &id))
            .iter()
            .all(|def| !def.spec.name.starts_with("mcp__"))
    );
    let record = &mcp_connections(&flow.f.store, &flow.f.repo).unwrap()[0];
    assert_eq!(record.phase, McpConnectionPhase::Settled);
    assert_eq!(record.acknowledged, Some(true));
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn runtime_shutdown_interrupts_active_call_without_session_cancellation() {
    let flow = Flow::new(vec![], false);
    configure(&flow, json!({}));
    let id = flow.session("default").await;
    let defs = ready(&flow, &id).await;
    let def = defs
        .iter()
        .find(|def| def.spec.name == "mcp__shared__read")
        .unwrap();
    let mut inv = flow.f.invocation("default", &def.spec.name, json!({}));
    inv.session_id = id.clone();
    inv.registration = def.registration.clone();
    flow.f.write("block-call", "block");
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let host = flow.f.host.clone();
    let task = tokio::spawn(async move { host.execute(inv, token).await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !flow.f.repo.join("calls").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), flow.f.host.shutdown())
        .await
        .unwrap();
    assert!(!cancel.is_cancelled());
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap(),
        ToolOutcome::Aborted
    );
    assert!(
        flow.f
            .host
            .definitions(&turn(&flow, &id))
            .iter()
            .all(|def| !def.spec.name.starts_with("mcp__"))
    );
    let record = &mcp_connections(&flow.f.store, &flow.f.repo).unwrap()[0];
    assert_eq!(record.phase, McpConnectionPhase::Settled);
    assert_eq!(record.acknowledged, Some(true));
    flow.runtime.shutdown().await;
}

struct Git;
impl cyber_core::worktrees::GitExecution for Git {
    fn run<'a>(
        &'a self,
        directory: &'a std::path::Path,
        args: &'a [std::ffi::OsString],
    ) -> cyber_core::worktrees::GitFuture<'a> {
        Box::pin(async move {
            std::process::Command::new("git")
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .current_dir(directory)
                .args(args)
                .output()
        })
    }
}

#[tokio::test]
async fn shared_runtime_pins_actual_managed_checkout_until_native_shutdown() {
    use cyber_core::worktrees::{CheckoutActivity, Name, RemovalActivity, Repository, Settings};
    use cyber_server::runtime::CreateSession;
    let flow = Flow::new(vec![], false);
    configure(&flow, json!({}));
    flow.f.write("ready", "ready");
    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.name", "MCP test"],
        vec!["config", "user.email", "test@example.invalid"],
        vec!["add", "ready"],
        vec!["commit", "--quiet", "-m", "initial"],
    ] {
        use cyber_core::worktrees::GitExecution;
        assert!(
            Git.run(
                &flow.f.repo,
                &args
                    .into_iter()
                    .map(std::ffi::OsString::from)
                    .collect::<Vec<_>>()
            )
            .await
            .unwrap()
            .status
            .success()
        );
    }
    let repository = Repository::discover(&Git, &flow.f.repo).await.unwrap();
    let managed = repository
        .create(
            &Git,
            &Settings::default(),
            flow.f.dir.path(),
            "prj_test",
            &Name::parse("mcp-runtime").unwrap(),
        )
        .await
        .unwrap();
    let info = flow
        .runtime
        .create_session(CreateSession {
            directory: managed.path.display().to_string(),
            model: "test/main".into(),
            mode: Some("default".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let turn = TurnContext {
        session_id: info.id.clone(),
        directory: info.directory.clone(),
        agent: info.agent.clone(),
        mode: info.mode.clone(),
        prefers_apply_patch: false,
        rules: info.rules.clone(),
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while !flow
            .f
            .host
            .definitions(&turn)
            .iter()
            .any(|tool| tool.spec.name.starts_with("mcp__"))
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let record = &mcp_connections(&flow.f.store, &managed.path).unwrap()[0];
    assert_eq!(record.worktree_ids, vec![managed.id.clone()]);
    assert!(CheckoutActivity.reserve(&managed).is_err());
    flow.f.host.close_mcp_location(&managed.path).await.unwrap();
    assert_eq!(
        mcp_connections(&flow.f.store, &managed.path).unwrap()[0].phase,
        McpConnectionPhase::Settled
    );
    flow.runtime.shutdown().await;
    assert_eq!(
        mcp_connections(&flow.f.store, &managed.path).unwrap()[0].phase,
        McpConnectionPhase::Settled
    );
    assert!(CheckoutActivity.reserve(&managed).is_ok());
    assert_eq!(
        std::fs::read_to_string(managed.path.join("ready")).unwrap(),
        "ready"
    );
}

#[tokio::test]
async fn location_close_reopens_with_new_identity_and_refuses_old_calls() {
    let flow = Flow::new(vec![], false);
    configure(&flow, json!({}));
    let id = flow.session("default").await;
    let defs = ready(&flow, &id).await;
    let old = defs
        .iter()
        .find(|def| def.spec.name == "mcp__shared__read")
        .unwrap();
    let mut inv = flow.f.invocation("default", &old.spec.name, json!({}));
    inv.session_id = id.clone();
    inv.registration = old.registration.clone();
    flow.f.host.close_mcp_location(&flow.f.repo).await.unwrap();
    flow.f.host.close_mcp_location(&flow.f.repo).await.unwrap();
    assert!(
        flow.f
            .host
            .definitions(&turn(&flow, &id))
            .iter()
            .all(|def| !def.spec.name.starts_with("mcp__"))
    );
    let before = mcp_connections(&flow.f.store, &flow.f.repo).unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].phase, McpConnectionPhase::Settled);
    assert_eq!(before[0].acknowledged, Some(true));
    let other = flow.session("default").await;
    let fresh = ready(&flow, &other).await;
    let new = fresh
        .iter()
        .find(|def| def.spec.name == old.spec.name)
        .unwrap();
    assert_ne!(old.registration, new.registration);
    assert!(
        matches!(flow.f.host.execute(inv, CancellationToken::new()).await, ToolOutcome::Failed(error) if error.contains("Stale tool call"))
    );
    assert!(!flow.f.repo.join("calls").exists());
    assert_eq!(
        std::fs::read_to_string(flow.f.repo.join("spawned"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn location_close_during_startup_preserves_unknown_and_refuses_reopening() {
    let flow = Flow::new(vec![], false);
    configure(&flow, json!({}));
    let id = flow.session("default").await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while !flow.f.repo.join("spawned").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let error = flow
        .f
        .host
        .close_mcp_location(&flow.f.repo)
        .await
        .unwrap_err();
    assert!(error.contains("unresolved native ownership"), "{error}");
    flow.f.write("ready", "ready");
    flow.session("default").await;
    assert!(
        flow.f
            .host
            .definitions(&turn(&flow, &id))
            .iter()
            .all(|def| !def.spec.name.starts_with("mcp__"))
    );
    let records = mcp_connections(&flow.f.store, &flow.f.repo).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].phase, McpConnectionPhase::Unknown);
    assert_eq!(records[0].acknowledged, Some(false));
    assert_eq!(
        std::fs::read_to_string(flow.f.repo.join("spawned"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert!(flow.f.host.close_mcp_location(&flow.f.repo).await.is_err());
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn location_close_interrupts_active_call_without_session_cancellation() {
    let flow = Flow::new(vec![], false);
    configure(&flow, json!({}));
    let id = flow.session("default").await;
    let defs = ready(&flow, &id).await;
    let def = defs
        .iter()
        .find(|def| def.spec.name == "mcp__shared__read")
        .unwrap();
    let mut inv = flow.f.invocation("default", &def.spec.name, json!({}));
    inv.session_id = id.clone();
    inv.registration = def.registration.clone();
    flow.f.write("block-call", "block");
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let host = flow.f.host.clone();
    let task = tokio::spawn(async move { host.execute(inv, token).await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !flow.f.repo.join("calls").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(
        Duration::from_secs(3),
        flow.f.host.close_mcp_location(&flow.f.repo),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!cancel.is_cancelled());
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap(),
        ToolOutcome::Aborted
    );
    assert!(
        flow.f
            .host
            .definitions(&turn(&flow, &id))
            .iter()
            .all(|def| !def.spec.name.starts_with("mcp__"))
    );
    let record = &mcp_connections(&flow.f.store, &flow.f.repo).unwrap()[0];
    assert_eq!(record.phase, McpConnectionPhase::Settled);
    assert_eq!(record.acknowledged, Some(true));
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn location_close_keeps_sibling_connection_running() {
    let flow = Flow::new(vec![], false);
    configure(&flow, json!({}));
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    let sibling = flow.f.repo.parent().unwrap().join("sibling");
    std::fs::create_dir(&sibling).unwrap();
    std::fs::write(sibling.join("ready"), "ready").unwrap();
    let info = flow
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: sibling.display().to_string(),
            mode: Some("default".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut context = turn(&flow, &info.id);
    context.directory = info.directory.clone();
    let defs = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let defs = flow.f.host.definitions(&context);
            if defs.iter().any(|def| def.spec.name == "mcp__shared__read") {
                break defs;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    flow.f.host.close_mcp_location(&flow.f.repo).await.unwrap();
    assert_eq!(
        mcp_connections(&flow.f.store, &sibling).unwrap()[0].phase,
        McpConnectionPhase::Running
    );
    let def = defs
        .iter()
        .find(|def| def.spec.name == "mcp__shared__read")
        .unwrap();
    let mut inv = flow.f.invocation("default", &def.spec.name, json!({}));
    inv.session_id = info.id;
    inv.directory = info.directory;
    inv.registration = def.registration.clone();
    assert_eq!(
        flow.f.host.execute(inv, CancellationToken::new()).await,
        ToolOutcome::Ok("remote read".into())
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn configured_snapshot_tracks_startup_and_fresh_definition_changes() {
    use cyber_server::runtime::McpConnectionStatus;
    let flow = Flow::new(vec![], false);
    let path = configure(&flow, json!({}));
    let id = flow.session("default").await;
    let pending = flow.f.host.mcp_status(&flow.f.repo).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].status, McpConnectionStatus::Connecting);
    assert!(pending[0].tools.is_empty());
    ready(&flow, &id).await;
    let connected = flow.f.host.mcp_status(&flow.f.repo).unwrap();
    assert_eq!(connected[0].status, McpConnectionStatus::Connected);
    assert_eq!(connected[0].tools.len(), 4);
    assert_eq!(
        connected[0].connection.as_ref().unwrap().phase,
        McpConnectionPhase::Running
    );
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["mcp"]["shared"]["enabled"] = json!(false);
    std::fs::write(&path, config.to_string()).unwrap();
    let disabled = flow.f.host.mcp_status(&flow.f.repo).unwrap();
    assert_eq!(disabled[0].status, McpConnectionStatus::Disabled);
    assert!(disabled[0].tools.is_empty());
    assert_eq!(disabled[0].connection, connected[0].connection);
    config["mcp"]["shared"]["enabled"] = json!(true);
    config["mcp"]["shared"]["args"]
        .as_array_mut()
        .unwrap()
        .push(json!("changed"));
    std::fs::write(&path, config.to_string()).unwrap();
    let changed = flow.f.host.mcp_status(&flow.f.repo).unwrap();
    assert_eq!(changed[0].status, McpConnectionStatus::Failed);
    assert!(
        changed[0]
            .error
            .as_ref()
            .unwrap()
            .contains("definition changed")
    );
    assert!(changed[0].tools.is_empty());
    flow.f.host.close_mcp_location(&flow.f.repo).await.unwrap();
    let closed = flow.f.host.mcp_status(&flow.f.repo).unwrap();
    assert_eq!(closed[0].status, McpConnectionStatus::Failed);
    assert!(closed[0].connection.is_none());
    assert_eq!(
        std::fs::read_to_string(flow.f.repo.join("spawned"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    flow.runtime.shutdown().await;
}

async fn wait_for_idle_loss(flow: &Flow, id: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let records = mcp_connections(&flow.f.store, &flow.f.repo).unwrap();
            let snapshot = flow.f.host.mcp_status(&flow.f.repo).unwrap();
            if records[0].phase == McpConnectionPhase::Settled
                && snapshot[0]
                    .connection
                    .as_ref()
                    .is_some_and(|record| record.phase == McpConnectionPhase::Settled)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let records = mcp_connections(&flow.f.store, &flow.f.repo).unwrap();
    assert_eq!(records[0].acknowledged, Some(true));
    let snapshot = flow.f.host.mcp_status(&flow.f.repo).unwrap();
    assert_eq!(
        snapshot[0].status,
        cyber_server::runtime::McpConnectionStatus::Failed
    );
    assert!(snapshot[0].tools.is_empty());
    assert!(snapshot[0].error.as_ref().unwrap().contains("reconnect"));
    assert!(
        flow.f
            .host
            .definitions(&turn(flow, id))
            .iter()
            .all(|def| !def.spec.name.starts_with("mcp__"))
    );
    assert!(!flow.f.repo.join("calls").exists());
}

#[tokio::test]
async fn idle_leader_death_removes_discovery_and_settles_without_a_tool_call() {
    let flow = Flow::new(vec![], false);
    configure(&flow, json!({}));
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    let pid = flow.f.read("spawned").trim().parse::<u32>().unwrap();
    assert!(
        std::process::Command::new("/bin/kill")
            .args(["-9", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
    wait_for_idle_loss(&flow, &id).await;
    flow.f.host.close_mcp_location(&flow.f.repo).await.unwrap();
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn idle_stdout_eof_stops_a_still_running_leader_without_an_rpc() {
    let flow = Flow::new(vec![], false);
    let path = configure(&flow, json!({}));
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["mcp"]["shared"]["args"][2] = json!(format!(
        "{SERVER}\n    if method=='tools/list':\n        os.close(1)\n        while True: time.sleep(1)\n"
    ));
    std::fs::write(&path, config.to_string()).unwrap();
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    wait_for_idle_loss(&flow, &id).await;
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn idle_monitor_does_not_keep_a_dropped_host_alive() {
    let flow = Flow::new(vec![], false);
    configure(&flow, json!({}));
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    let host = Arc::downgrade(&flow.f.host);
    drop(flow);
    assert!(host.upgrade().is_none());
}

#[tokio::test]
async fn list_change_during_call_refreshes_metadata_and_rejects_old_registration() {
    let flow = Flow::new(vec![], false);
    let path = configure(&flow, json!({}));
    let script = SERVER.replace("result={'tools':", "with open('lists','a') as f: f.write('list\\n')\n        result={'tools':")
        .replace("['read','write','structured','error']", "(['read'] if pathlib.Path('changed').exists() else ['read','write','structured','error'])")
        .replace("'inputSchema':{'type':'object'}", "'inputSchema':{'type':'object','description':('changed' if pathlib.Path('changed').exists() else 'initial')}")
        .replace("with open('calls','a')", "pathlib.Path('changed').touch()\n        print(json.dumps({'jsonrpc':'2.0','method':'notifications/tools/list_changed'}),flush=True)\n        with open('calls','a')");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["mcp"]["shared"]["args"][2] = json!(script);
    std::fs::write(&path, config.to_string()).unwrap();
    let id = flow.session("default").await;
    let original = ready(&flow, &id).await;
    let old = original
        .iter()
        .find(|def| def.spec.name == "mcp__shared__read")
        .unwrap();
    assert_eq!(
        invoke(&flow, &id, old, "default").await,
        ToolOutcome::Ok("remote read".into())
    );
    let refreshed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let defs = flow.f.host.definitions(&turn(&flow, &id));
            if let Some(def) = defs
                .into_iter()
                .find(|def| def.spec.name == old.spec.name && def.registration != old.registration)
            {
                break def;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(flow.f.read("lists"), "list\nlist\n");
    assert!(
        flow.f
            .host
            .definitions(&turn(&flow, &id))
            .iter()
            .all(|def| def.spec.name != "mcp__shared__write")
    );
    assert!(matches!(
        invoke(&flow, &id, old, "default").await,
        ToolOutcome::Failed(_)
    ));
    assert_eq!(flow.f.read("calls"), "read\n");
    assert_eq!(
        invoke(&flow, &id, &refreshed, "default").await,
        ToolOutcome::Ok("remote read".into())
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn idle_notification_before_stdout_eof_still_settles_native_owner() {
    let flow = Flow::new(vec![], false);
    let path = configure(&flow, json!({}));
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["mcp"]["shared"]["args"][2] = json!(format!(
        "{SERVER}\n    if method=='tools/list':\n        print(json.dumps({{'jsonrpc':'2.0','method':'notifications/tools/list_changed'}}),flush=True)\n        os.close(1)\n        while True: time.sleep(1)\n"
    ));
    std::fs::write(&path, config.to_string()).unwrap();
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    wait_for_idle_loss(&flow, &id).await;
    flow.runtime.shutdown().await;
}

fn configure_idle_notifications(flow: &Flow) -> PathBuf {
    let path = configure(flow, json!({}));
    let script = format!("{}{}", r#"
import threading
watching=False
def notify():
    while not pathlib.Path('notify').exists(): time.sleep(.01)
    print(json.dumps({'jsonrpc':'2.0','method':'notifications/tools/list_changed'}),flush=True)
    pathlib.Path('notified').touch()
"#, SERVER.replace("result={'tools':", "with open('lists','a') as f: f.write('list\\n')\n        while pathlib.Path('block-list').exists(): time.sleep(.01)\n        result={'tools':")
        .replace("['read','write','structured','error']", "(['read'] if pathlib.Path('notify').exists() else ['read','write','structured','error'])"));
    let script = format!(
        "{script}\n    if method=='tools/list' and not watching:\n        watching=True\n        threading.Thread(target=notify,daemon=True).start()\n"
    );
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["mcp"]["shared"]["args"][2] = json!(script);
    std::fs::write(&path, config.to_string()).unwrap();
    path
}

async fn wait_for_file(flow: &Flow, name: &str, expected: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while std::fs::read_to_string(flow.f.repo.join(name))
            .ok()
            .as_deref()
            != Some(expected)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn idle_list_change_refreshes_without_a_tool_call() {
    let flow = Flow::new(vec![], false);
    configure_idle_notifications(&flow);
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    flow.f.write("notify", "notify");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = flow.f.host.mcp_status(&flow.f.repo).unwrap();
            if snapshot[0].tools.len() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(flow.f.read("lists"), "list\nlist\n");
    assert!(!flow.f.repo.join("calls").exists());
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn idle_refresh_rechecks_configuration_before_peer_effects() {
    let flow = Flow::new(vec![], false);
    let path = configure_idle_notifications(&flow);
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["mcp"]["shared"]["enabled"] = json!(false);
    std::fs::write(&path, config.to_string()).unwrap();
    flow.f.write("notify", "notify");
    wait_for_file(&flow, "notified", "").await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if mcp_connections(&flow.f.store, &flow.f.repo).unwrap()[0].phase
                == McpConnectionPhase::Settled
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(flow.f.read("lists"), "list\n");
    assert!(!flow.f.repo.join("calls").exists());
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn close_interrupts_blocked_idle_refresh_and_joins_native_owner() {
    let flow = Flow::new(vec![], false);
    configure_idle_notifications(&flow);
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    flow.f.write("block-list", "blocked");
    flow.f.write("notify", "notify");
    wait_for_file(&flow, "lists", "list\nlist\n").await;
    assert!(
        flow.f
            .host
            .definitions(&turn(&flow, &id))
            .iter()
            .all(|def| !def.spec.name.starts_with("mcp__"))
    );
    tokio::time::timeout(
        Duration::from_secs(5),
        flow.f.host.close_mcp_location(&flow.f.repo),
    )
    .await
    .unwrap()
    .unwrap();
    let record = &mcp_connections(&flow.f.store, &flow.f.repo).unwrap()[0];
    assert_eq!(record.phase, McpConnectionPhase::Settled);
    assert_eq!(record.acknowledged, Some(true));
    assert!(!flow.f.repo.join("calls").exists());
    flow.runtime.shutdown().await;
}

async fn wait_for_reconnected_tool(flow: &Flow, id: &str, old: &ToolDef) -> ToolDef {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let Some(def) = flow
                .f
                .host
                .definitions(&turn(flow, id))
                .into_iter()
                .find(|def| def.spec.name == old.spec.name && def.registration != old.registration)
            {
                break def;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
fn kill_first_mcp_leader(flow: &Flow) {
    let pid = flow.f.read("spawned").lines().next().unwrap().to_string();
    assert!(
        std::process::Command::new("/bin/kill")
            .args(["-9", &pid])
            .status()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn idle_loss_reconnects_with_new_identity_without_replaying_old_calls() {
    let flow = Flow::new(vec![], false);
    configure(&flow, json!({}));
    let id = flow.session("default").await;
    let defs = ready(&flow, &id).await;
    let old = defs
        .iter()
        .find(|def| def.spec.name == "mcp__shared__read")
        .unwrap();
    kill_first_mcp_leader(&flow);
    wait_for_idle_loss(&flow, &id).await;
    let new = wait_for_reconnected_tool(&flow, &id, old).await;
    let records = mcp_connections(&flow.f.store, &flow.f.repo).unwrap();
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .any(|record| record.phase == McpConnectionPhase::Settled
                && record.acknowledged == Some(true))
    );
    assert!(
        records
            .iter()
            .any(|record| record.phase == McpConnectionPhase::Running)
    );
    assert_eq!(flow.f.read("spawned").lines().count(), 2);
    assert!(!flow.f.repo.join("calls").exists());
    assert!(matches!(
        invoke(&flow, &id, old, "default").await,
        ToolOutcome::Failed(_)
    ));
    assert!(!flow.f.repo.join("calls").exists());
    assert_eq!(
        invoke(&flow, &id, &new, "default").await,
        ToolOutcome::Ok("remote read".into())
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn transient_reconnect_discovery_failure_retries_after_proven_settlement() {
    let flow = Flow::new(vec![], false);
    let path = configure(&flow, json!({}));
    let script = SERVER.replace("elif method=='tools/list':", "elif method=='tools/list':\n        if pathlib.Path('fail-list').exists():\n            print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'tools':'invalid'}}),flush=True)\n            continue");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["mcp"]["shared"]["args"][2] = json!(script);
    std::fs::write(&path, config.to_string()).unwrap();
    let id = flow.session("default").await;
    let defs = ready(&flow, &id).await;
    let old = defs
        .iter()
        .find(|def| def.spec.name == "mcp__shared__read")
        .unwrap();
    flow.f.write("fail-list", "fail");
    kill_first_mcp_leader(&flow);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let records = mcp_connections(&flow.f.store, &flow.f.repo).unwrap();
            if records.len() == 2
                && records
                    .iter()
                    .all(|record| record.phase == McpConnectionPhase::Settled)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    std::fs::remove_file(flow.f.repo.join("fail-list")).unwrap();
    wait_for_reconnected_tool(&flow, &id, old).await;
    assert_eq!(flow.f.read("spawned").lines().count(), 3);
    assert!(!flow.f.repo.join("calls").exists());
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn location_close_cancels_backoff_before_another_native_launch() {
    let flow = Flow::new(vec![], false);
    configure(&flow, json!({}));
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    kill_first_mcp_leader(&flow);
    wait_for_idle_loss(&flow, &id).await;
    flow.f.host.close_mcp_location(&flow.f.repo).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(flow.f.read("spawned").lines().count(), 1);
    assert_eq!(
        mcp_connections(&flow.f.store, &flow.f.repo).unwrap().len(),
        1
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn reconnect_rechecks_disabled_definition_before_new_admission() {
    let flow = Flow::new(vec![], false);
    let path = configure(&flow, json!({}));
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    kill_first_mcp_leader(&flow);
    wait_for_idle_loss(&flow, &id).await;
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["mcp"]["shared"]["enabled"] = json!(false);
    std::fs::write(&path, config.to_string()).unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(flow.f.read("spawned").lines().count(), 1);
    assert_eq!(
        mcp_connections(&flow.f.store, &flow.f.repo).unwrap().len(),
        1
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn failed_loss_settlement_retains_actor_and_never_reconnects_from_database_state() {
    use cyber_core::paths::DatabaseLocation;
    use cyber_server::runtime::{McpConnectionStatus, NoSnapshots, Runtime};
    use cyber_store::{Store, StoreOptions};
    use std::sync::atomic::{AtomicBool, Ordering};
    let reject = Arc::new(AtomicBool::new(true));
    let gate = reject.clone();
    let mut registry = Runtime::registry();
    registry.projector(move |_, event| {
        if gate.load(Ordering::SeqCst)
            && event.kind == "mcp.status.changed.1"
            && event.data["record"]["phase"] == "settled"
        {
            return Err("injected loss settlement failure".into());
        }
        Ok(())
    });
    let mut fixture = support::Fixture::new();
    fixture.store =
        Arc::new(Store::open(StoreOptions::new(DatabaseLocation::Memory, registry)).unwrap());
    fixture.renew_host(None);
    let flow = Flow::with(fixture, vec![], false, Arc::new(NoSnapshots));
    configure(&flow, json!({}));
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    kill_first_mcp_leader(&flow);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = flow.f.host.mcp_status(&flow.f.repo).unwrap();
            if snapshot[0].status == McpConnectionStatus::Failed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(flow.f.read("spawned").lines().count(), 1);
    assert_eq!(
        mcp_connections(&flow.f.store, &flow.f.repo).unwrap().len(),
        1
    );
    assert!(flow.f.host.close_mcp_location(&flow.f.repo).await.is_err());
    reject.store(false, Ordering::SeqCst);
    flow.f.host.close_mcp_location(&flow.f.repo).await.unwrap();
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn interrupted_tool_call_reconnects_transport_without_replaying_its_effect() {
    let flow = Flow::new(vec![], false);
    configure(&flow, json!({}));
    let id = flow.session("default").await;
    let defs = ready(&flow, &id).await;
    let old = defs
        .iter()
        .find(|def| def.spec.name == "mcp__shared__read")
        .unwrap();
    flow.f.write("block-call", "block");
    let mut inv = flow.f.invocation("default", &old.spec.name, json!({}));
    inv.session_id = id.clone();
    inv.registration = old.registration.clone();
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let host = flow.f.host.clone();
    let task = tokio::spawn(async move { host.execute(inv, token).await });
    wait_for_file(&flow, "calls", "read\n").await;
    cancel.cancel();
    assert_eq!(task.await.unwrap(), ToolOutcome::Aborted);
    wait_for_reconnected_tool(&flow, &id, old).await;
    assert_eq!(flow.f.read("calls"), "read\n");
    assert_eq!(flow.f.read("spawned").lines().count(), 2);
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn close_during_reconnect_initialization_retains_unknown_and_prevents_replacement() {
    let flow = Flow::new(vec![], false);
    configure(&flow, json!({}));
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    std::fs::remove_file(flow.f.repo.join("ready")).unwrap();
    kill_first_mcp_leader(&flow);
    tokio::time::timeout(Duration::from_secs(5), async {
        while std::fs::read_to_string(flow.f.repo.join("spawned"))
            .unwrap()
            .lines()
            .count()
            != 2
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_secs(3),
            flow.f.host.close_mcp_location(&flow.f.repo)
        )
        .await
        .unwrap()
        .is_err()
    );
    let records = mcp_connections(&flow.f.store, &flow.f.repo).unwrap();
    assert!(
        records
            .iter()
            .any(|record| record.phase == McpConnectionPhase::Unknown
                && record.acknowledged == Some(false))
    );
    flow.f.write("ready", "ready");
    let _ = flow.session("default").await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(flow.f.read("spawned").lines().count(), 2);
    assert!(flow.f.host.close_mcp_location(&flow.f.repo).await.is_err());
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn reconnect_backoff_does_not_keep_host_alive() {
    let flow = Flow::new(vec![], false);
    configure(&flow, json!({}));
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    kill_first_mcp_leader(&flow);
    wait_for_idle_loss(&flow, &id).await;
    let host = Arc::downgrade(&flow.f.host);
    drop(flow);
    assert!(host.upgrade().is_none());
}

#[tokio::test]
async fn wait_tool_observes_delayed_startup_without_invoking_remote_tools() {
    let flow = Flow::new(vec![], false);
    configure(&flow, json!({}));
    let id = flow.session("default").await;
    let mut inv = flow.f.invocation(
        "plan",
        "wait_for_mcp",
        json!({"servers":["shared"],"timeout":3}),
    );
    inv.session_id = id.clone();
    let host = flow.f.host.clone();
    let task = tokio::spawn(async move { host.execute(inv, CancellationToken::new()).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!task.is_finished());
    ready(&flow, &id).await;
    let result: Value = serde_json::from_str(&support::ok(task.await.unwrap())).unwrap();
    assert_eq!(result["servers"][0]["status"], "connected");
    assert_eq!(result["timed_out"], false);
    assert!(!flow.f.repo.join("calls").exists());
    assert_eq!(
        mcp_connections(&flow.f.store, &flow.f.repo).unwrap().len(),
        1
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn wait_tool_timeout_and_cancellation_leave_startup_owned() {
    let flow = Flow::new(vec![], false);
    configure(&flow, json!({}));
    let id = flow.session("default").await;
    let mut inv = flow.f.invocation(
        "default",
        "wait_for_mcp",
        json!({"servers":["shared"],"timeout":0}),
    );
    inv.session_id = id.clone();
    let result: Value = serde_json::from_str(&support::ok(
        flow.f
            .host
            .execute(inv.clone(), CancellationToken::new())
            .await,
    ))
    .unwrap();
    assert_eq!(result["timed_out"], true);
    assert_eq!(result["servers"][0]["status"], "connecting");
    inv.input["timeout"] = json!(1);
    let started = tokio::time::Instant::now();
    let output = tokio::time::timeout(
        Duration::from_secs(3),
        flow.f.host.execute(inv.clone(), CancellationToken::new()),
    )
    .await
    .unwrap();
    assert!(started.elapsed() >= Duration::from_secs(1));
    let result: Value = serde_json::from_str(&support::ok(output)).unwrap();
    assert_eq!(result["timed_out"], true);
    inv.input["timeout"] = json!(60);
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let host = flow.f.host.clone();
    let task = tokio::spawn(async move { host.execute(inv, token).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!task.is_finished());
    cancel.cancel();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap(),
        ToolOutcome::Aborted
    );
    ready(&flow, &id).await;
    assert!(!flow.f.repo.join("calls").exists());
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn runtime_wait_makes_connected_tools_available_on_following_model_step() {
    let flow = Flow::new(
        vec![
            call(
                "wait",
                "wait_for_mcp",
                json!({"servers":["shared"],"timeout":3}),
            ),
            call("read", "mcp__shared__read", json!({})),
            text("done"),
        ],
        false,
    );
    configure(&flow, json!({}));
    let id = flow.session("default").await;
    flow.prompt(&id, "wait then use the server").await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while flow.main.requests().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        flow.main.requests()[0]
            .tools
            .iter()
            .any(|tool| tool.name == "wait_for_mcp")
    );
    assert!(
        flow.main.requests()[0]
            .tools
            .iter()
            .all(|tool| !tool.name.starts_with("mcp__"))
    );
    ready(&flow, &id).await;
    flow.settle(&id).await;
    assert!(
        flow.main.requests()[1]
            .tools
            .iter()
            .any(|tool| tool.name == "mcp__shared__read")
    );
    assert_eq!(flow.output(&id, "read").await, "remote read");
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn wait_tool_pre_hook_can_refuse_before_waiting_even_in_bypass_mode() {
    let flow = Flow::new(vec![], false);
    configure(
        &flow,
        json!({"PreToolUse":[{"matcher":"wait_for_mcp","hooks":[{
            "type":"command","command":"echo '{\"decision\":\"deny\",\"reason\":\"wait denied by hook\"}'"
        }]}]}),
    );
    let id = flow.session("default").await;
    let mut inv = flow
        .f
        .invocation("bypass", "wait_for_mcp", json!({"servers":["shared"]}));
    inv.session_id = id.clone();
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        flow.f.host.execute(inv, CancellationToken::new()),
    )
    .await
    .unwrap();
    assert!(matches!(result, ToolOutcome::Failed(error) if error.contains("wait denied by hook")));
    assert!(!flow.f.repo.join("calls").exists());
    ready(&flow, &id).await;
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn deferred_search_loads_only_selected_schema_on_following_steps() {
    let flow = Flow::new(
        vec![
            call("unloaded", "mcp__shared__read", json!({})),
            call(
                "search",
                "tool_search",
                json!({"select":["mcp__shared__read"]}),
            ),
            call("loaded", "mcp__shared__read", json!({})),
            text("done"),
        ],
        false,
    );
    let path = configure(&flow, json!({}));
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["tool_output"] = json!({"deferred_threshold_tokens":0});
    std::fs::write(path, config.to_string()).unwrap();
    flow.f.set_config(config);
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    flow.prompt(&id, "discover then read").await;
    flow.settle(&id).await;
    assert_eq!(
        flow.output(&id, "unloaded").await,
        "Tool mcp__shared__read is deferred. Load it with tool_search first."
    );
    let result: Value = serde_json::from_str(&flow.output(&id, "search").await).unwrap();
    assert_eq!(result["tools"][0]["name"], "mcp__shared__read");
    assert_eq!(result["tools"][0]["input_schema"]["type"], "object");
    assert_eq!(flow.output(&id, "loaded").await, "remote read");
    assert_eq!(flow.f.read("calls"), "read\n");
    let requests = flow.main.requests();
    for request in &requests[..2] {
        assert!(
            request
                .tools
                .iter()
                .all(|tool| !tool.name.starts_with("mcp__"))
        );
        assert!(request.system.iter().any(|part| part.contains("<deferred_tools>") && part.contains("mcp__shared__read")));
    }
    assert!(
        requests[2]
            .tools
            .iter()
            .any(|tool| tool.name == "mcp__shared__read")
    );
    assert!(
        requests[2]
            .tools
            .iter()
            .all(|tool| tool.name != "mcp__shared__write")
    );
    let other = flow.session("default").await;
    let mut inv = flow.f.invocation(
        "default",
        "tool_search",
        json!({"select":["missing","mcp__shared__write"]}),
    );
    inv.session_id = other;
    assert!(
        matches!(flow.f.host.execute(inv, CancellationToken::new()).await,
        ToolOutcome::Failed(error) if error.contains("Unavailable tool: missing"))
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn search_query_loads_without_rpc_and_does_not_load_another_session() {
    let flow = Flow::new(
        vec![
            call("search", "tool_search", json!({"query":"SHARED read"})),
            text("done"),
            call("unloaded", "mcp__shared__read", json!({})),
            text("done"),
        ],
        false,
    );
    let path = configure(&flow, json!({}));
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["tool_output"] = json!({"deferred_threshold_tokens":0});
    std::fs::write(path, config.to_string()).unwrap();
    flow.f.set_config(config);
    let first = flow.session("default").await;
    ready(&flow, &first).await;
    flow.prompt(&first, "find the schema").await;
    flow.settle(&first).await;
    assert!(
        flow.runtime
            .state(&first)
            .await
            .unwrap()
            .loaded_tool_names()
            .contains("mcp__shared__read")
    );
    assert!(!flow.f.repo.join("calls").exists());
    let second = flow.session("default").await;
    flow.prompt(&second, "try without loading").await;
    flow.settle(&second).await;
    assert_eq!(
        flow.output(&second, "unloaded").await,
        "Tool mcp__shared__read is deferred. Load it with tool_search first."
    );
    assert!(
        flow.runtime
            .state(&second)
            .await
            .unwrap()
            .loaded_tool_names()
            .is_empty()
    );
    assert!(!flow.f.repo.join("calls").exists());
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn search_pre_hook_denial_preserves_empty_selection_even_in_bypass() {
    let flow = Flow::new(vec![], false);
    configure(
        &flow,
        json!({"PreToolUse":[{"matcher":"tool_search","hooks":[{
            "type":"command","command":"echo '{\"decision\":\"deny\",\"reason\":\"search denied by hook\"}'"
        }]}]}),
    );
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    let mut inv = flow.f.invocation(
        "bypass",
        "tool_search",
        json!({"select":["mcp__shared__read"]}),
    );
    inv.session_id = id.clone();
    assert!(
        matches!(flow.f.host.execute(inv, CancellationToken::new()).await,
        ToolOutcome::Failed(error) if error.contains("search denied by hook"))
    );
    assert!(
        flow.runtime
            .state(&id)
            .await
            .unwrap()
            .loaded_tool_names()
            .is_empty()
    );
    assert!(!flow.f.repo.join("calls").exists());
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn loaded_schema_does_not_override_a_fresh_permission_deny() {
    let flow = Flow::new(
        vec![call("denied", "mcp__shared__read", json!({})), text("done")],
        false,
    );
    let path = configure(&flow, json!({}));
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["tool_output"] = json!({"deferred_threshold_tokens":0});
    std::fs::write(&path, config.to_string()).unwrap();
    flow.f.set_config(config.clone());
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    let mut inv = flow.f.invocation(
        "default",
        "tool_search",
        json!({"select":["mcp__shared__read"]}),
    );
    inv.session_id = id.clone();
    assert!(matches!(
        flow.f.host.execute(inv, CancellationToken::new()).await,
        ToolOutcome::Ok(_)
    ));
    config["permissions"] = json!({"mcp__shared__read":"deny"});
    std::fs::write(path, config.to_string()).unwrap();
    flow.f.set_config(config);
    flow.prompt(&id, "try the loaded tool").await;
    flow.settle(&id).await;
    assert_eq!(
        flow.output(&id, "denied").await,
        "Unknown tool: mcp__shared__read"
    );
    assert!(
        flow.main.requests()[0]
            .tools
            .iter()
            .all(|tool| tool.name != "mcp__shared__read")
    );
    assert!(
        flow.main.requests()[0]
            .system
            .iter()
            .all(|part| !part.contains("mcp__shared__read"))
    );
    assert!(!flow.f.repo.join("calls").exists());
    flow.runtime.shutdown().await;
}
