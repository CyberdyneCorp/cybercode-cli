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
