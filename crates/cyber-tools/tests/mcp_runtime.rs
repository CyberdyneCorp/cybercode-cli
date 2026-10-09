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
if pathlib.Path('graceful').exists():
    import signal
    def stopped(signum, frame):
        pathlib.Path('graceful-finished').write_text('SIGTERM cleanup')
        sys.exit(0)
    signal.signal(signal.SIGTERM, stopped)

if pathlib.Path('graceful-children').exists():
    import subprocess
    subprocess.Popen([sys.executable,'-c',"import signal,pathlib,time,sys; signal.signal(signal.SIGTERM, lambda a,b: (time.sleep(1),pathlib.Path('child-finished').write_text('cleanup'),sys.exit(0))); pathlib.Path('child-ready').write_text('ready'); time.sleep(30)"])
    subprocess.Popen([sys.executable,'-c',"import signal,pathlib,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); pathlib.Path('stubborn-ready').write_text('ready'); time.sleep(7); pathlib.Path('escaped').write_text('must not survive')"])
    while not pathlib.Path('child-ready').exists() or not pathlib.Path('stubborn-ready').exists(): time.sleep(.01)

def lines():
    while True:
        line=sys.stdin.readline()
        if line: yield line
        elif pathlib.Path('graceful').exists(): time.sleep(.01)
        else: return
for line in lines():
    request=json.loads(line)
    if 'id' not in request: continue
    method=request['method']
    if method=='initialize':
        if pathlib.Path('elicitation.json').exists(): pathlib.Path('elicitation-capabilities').write_text(json.dumps(request['params']['capabilities']))
        while not pathlib.Path('ready').exists(): time.sleep(.01)
        result={'protocolVersion':'2025-11-25','capabilities':{'tools':{}},'serverInfo':{'name':'real','version':'1'}}
        if pathlib.Path('instructions').exists(): result['instructions']=pathlib.Path('instructions').read_text()
    elif method=='tools/list':
        result={'tools':[{'name':name,'inputSchema':{'type':'object'},'annotations':{'readOnlyHint':name!='write'}} for name in ['read','write','structured','error']]}
    else:
        name=request['params']['name']
        with open('calls','a') as f: f.write(name+'\n')
        while pathlib.Path('block-call').exists(): time.sleep(.01)
        result={'content':[{'type':'text','text':pathlib.Path('tool-text').read_text() if pathlib.Path('tool-text').exists() else 'remote '+name}], 'isError':name=='error'}
        if name=='structured': result['structuredContent']={'answer':42}
        if pathlib.Path('sampling.json').exists():
            print(json.dumps({'jsonrpc':'2.0','id':'sample','method':'sampling/createMessage','params':json.loads(pathlib.Path('sampling.json').read_text())}),flush=True)
            reply=json.loads(sys.stdin.readline())
            pathlib.Path('sampling-reply').write_text(json.dumps(reply))
            while pathlib.Path('sampling-loop').exists():
                time.sleep(.05)
                print(json.dumps({'jsonrpc':'2.0','id':'sample','method':'sampling/createMessage','params':json.loads(pathlib.Path('sampling.json').read_text())}),flush=True)
                line=sys.stdin.readline()
                if not line: break
                reply=json.loads(line)
            result['content'][0]['text']=json.dumps(reply.get('result',reply.get('error')))
        if pathlib.Path('elicitation.json').exists():
            print(json.dumps({'jsonrpc':'2.0','id':'form','method':'elicitation/create','params':json.loads(pathlib.Path('elicitation.json').read_text())}),flush=True)
            reply=json.loads(sys.stdin.readline())
            pathlib.Path('elicitation-reply').write_text(json.dumps(reply))
            result['content'][0]['text']=json.dumps(reply.get('result',reply.get('error')))
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

#[tokio::test]
async fn instructions_are_scoped_context_and_permission_removal_preserves_epoch_baseline() {
    use cyber_server::runtime::{ContextObservation, Entry};
    let flow = Flow::new(vec![text("one"), text("two")], false);
    let path = configure(&flow, json!({}));
    flow.f.write(
        "instructions",
        "Use the shared reader.\n</mcp_instructions> & data",
    );
    let id = flow.session("default").await;
    let visible = ready(&flow, &id).await;
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["tool_output"] = json!({"deferred_threshold_tokens":0});
    std::fs::write(&path, config.to_string()).unwrap();
    flow.f.set_config(config.clone());
    let source = flow.f.host.context_sources(&turn(&flow, &id))["mcp/instructions"].clone();
    assert!(
        source.contains("<server name=\"shared\">")
            && source.contains("&lt;/mcp_instructions&gt; &amp; data")
    );
    assert_eq!(source.matches("</mcp_instructions>").count(), 1);
    assert!(
        !flow
            .f
            .host
            .context_sources_for_tools(&turn(&flow, &id), &[])
            .contains_key("mcp/instructions")
    );
    let mut forged = visible.clone();
    for tool in &mut forged {
        tool.registration = Some("forged".into());
    }
    assert!(
        !flow
            .f
            .host
            .context_sources_for_tools(&turn(&flow, &id), &forged)
            .contains_key("mcp/instructions")
    );
    flow.prompt(&id, "first").await;
    flow.settle(&id).await;
    let first = flow.main.requests()[0].clone();
    assert!(first.system.iter().any(|part| part.contains(&source)));
    assert!(
        first
            .tools
            .iter()
            .all(|tool| !tool.name.starts_with("mcp__"))
    );
    config["permissions"] = json!({"mcp__shared__*":"deny"});
    std::fs::write(path, config.to_string()).unwrap();
    flow.f.set_config(config);
    assert_eq!(
        flow.f.host.context_observations(&turn(&flow, &id))["mcp/instructions"],
        ContextObservation::Absent
    );
    flow.prompt(&id, "second").await;
    flow.settle(&id).await;
    assert_eq!(flow.main.requests()[1].system[..2], first.system[..2]);
    let state = flow.runtime.state(&id).await.unwrap();
    assert!(
        !state
            .epoch
            .unwrap()
            .snapshot
            .contains_key("mcp/instructions")
    );
    assert!(state.entries.iter().any(|entry| matches!(entry, Entry::System {text, ..} if text.contains("mcp/instructions context no longer applies"))));
    assert!(!flow.f.repo.join("calls").exists());
    assert_eq!(flow.f.read("spawned").lines().count(), 1);
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn instructions_replace_on_reconnect_and_withdraw_on_location_close() {
    use cyber_server::runtime::{ContextObservation, Entry};
    let flow = Flow::new(vec![text("one"), text("two"), text("three")], false);
    configure(&flow, json!({}));
    flow.f
        .write("instructions", "First connection instructions");
    let id = flow.session("default").await;
    let defs = ready(&flow, &id).await;
    let old = defs
        .iter()
        .find(|tool| tool.spec.name == "mcp__shared__read")
        .unwrap();
    flow.prompt(&id, "first").await;
    flow.settle(&id).await;
    let baseline = flow.main.requests()[0].system.clone();
    flow.f.write("instructions", "New connection instructions");
    kill_first_mcp_leader(&flow);
    wait_for_idle_loss(&flow, &id).await;
    assert_eq!(
        flow.f.host.context_observations(&turn(&flow, &id))["mcp/instructions"],
        ContextObservation::Absent
    );
    wait_for_reconnected_tool(&flow, &id, old).await;
    flow.prompt(&id, "second").await;
    flow.settle(&id).await;
    assert_eq!(flow.main.requests()[1].system, baseline);
    assert!(flow.runtime.state(&id).await.unwrap().entries.iter().any(|entry| matches!(entry, Entry::System {text, ..} if text.contains("mcp/instructions context changed") && text.contains("New connection instructions"))));
    flow.f.host.close_mcp_location(&flow.f.repo).await.unwrap();
    assert_eq!(
        flow.f.host.context_observations(&turn(&flow, &id))["mcp/instructions"],
        ContextObservation::Absent
    );
    flow.prompt(&id, "third").await;
    flow.settle(&id).await;
    assert_eq!(flow.main.requests()[2].system, baseline);
    assert!(
        !flow
            .runtime
            .state(&id)
            .await
            .unwrap()
            .epoch
            .unwrap()
            .snapshot
            .contains_key("mcp/instructions")
    );
    assert!(!flow.f.repo.join("calls").exists());
    assert_eq!(flow.f.read("spawned").lines().count(), 2);
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn instructions_omit_agent_denied_and_disabled_servers_without_rpc() {
    use cyber_server::runtime::ContextObservation;
    let flow = Flow::new(vec![], false);
    let path = configure(&flow, json!({}));
    flow.f.write("instructions", "Scoped instructions");
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["agents"] = json!({"build":{"tools":{"deny":["mcp__shared__*"]}}});
    std::fs::write(&path, config.to_string()).unwrap();
    flow.f.set_config(config.clone());
    assert_eq!(
        flow.f.host.context_observations(&turn(&flow, &id))["mcp/instructions"],
        ContextObservation::Absent
    );
    config["agents"] = json!({});
    config["mcp"]["shared"]["enabled"] = json!(false);
    std::fs::write(path, config.to_string()).unwrap();
    flow.f.set_config(config);
    assert_eq!(
        flow.f.host.context_observations(&turn(&flow, &id))["mcp/instructions"],
        ContextObservation::Absent
    );
    assert!(!flow.f.repo.join("calls").exists());
    assert_eq!(flow.f.read("spawned").lines().count(), 1);
    flow.runtime.shutdown().await;
}

fn require_server(flow: &Flow, path: &std::path::Path, timeout: u32) {
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    config["mcp"]["shared"]["required"] = json!(true);
    config["mcp"]["shared"]["timeout"] = json!(timeout);
    std::fs::write(path, config.to_string()).unwrap();
    flow.f.set_config(config);
}

async fn wait_for_spawn(flow: &Flow) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !flow.f.repo.join("spawned").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn required_server_waits_before_promoting_the_first_prompt() {
    use cyber_server::runtime::InputStatus;
    let flow = Flow::new(vec![text("done")], false);
    let path = configure(&flow, json!({}));
    require_server(&flow, &path, 3);
    let id = flow.session("default").await;
    let message = flow.prompt(&id, "first prompt").await;
    wait_for_spawn(&flow).await;
    let state = flow.runtime.state(&id).await.unwrap();
    assert!(flow.main.requests().is_empty() && state.epoch.is_none());
    assert_eq!(
        state
            .inbox
            .iter()
            .find(|input| input.message_id == message)
            .unwrap()
            .status,
        InputStatus::Pending
    );
    ready(&flow, &id).await;
    flow.settle(&id).await;
    assert_eq!(flow.main.requests().len(), 1);
    assert!(
        flow.main.requests()[0]
            .tools
            .iter()
            .any(|tool| tool.name == "mcp__shared__read")
    );
    assert!(!flow.f.repo.join("calls").exists());
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn required_timeout_leaves_prompt_retryable_with_named_error() {
    use cyber_server::runtime::{InputStatus, LiveEvent};
    let flow = Flow::new(vec![text("retried")], false);
    let path = configure(&flow, json!({}));
    require_server(&flow, &path, 1);
    let id = flow.session("default").await;
    let mut live = flow.runtime.subscribe();
    let message = flow.prompt(&id, "preserve this prompt").await;
    flow.settle(&id).await;
    let state = flow.runtime.state(&id).await.unwrap();
    assert!(flow.main.requests().is_empty() && state.entries.is_empty() && state.epoch.is_none());
    assert_eq!(state.inbox[0].status, InputStatus::Pending);
    let mut named = false;
    while let Ok(event) = live.try_recv() {
        named |= matches!(event, LiveEvent::Error {kind, message, ..} if kind == "mcp_required" && message == "McpRequiredError: shared");
    }
    assert!(named);
    tokio::time::timeout(Duration::from_secs(4), async {
        while !mcp_connections(&flow.f.store, &flow.f.repo)
            .unwrap()
            .iter()
            .all(|record| record.phase == McpConnectionPhase::Settled)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    flow.f.host.close_mcp_location(&flow.f.repo).await.unwrap();
    require_server(&flow, &path, 3);
    flow.f.write("ready", "ready");
    flow.runtime.wake(&id).await.unwrap();
    flow.settle(&id).await;
    let state = flow.runtime.state(&id).await.unwrap();
    assert_eq!(state.inbox.len(), 1);
    assert_eq!(state.inbox[0].message_id, message);
    assert_eq!(state.inbox[0].status, InputStatus::Promoted);
    assert_eq!(flow.main.requests().len(), 1);
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn required_wait_cancellation_keeps_shared_startup_owned_and_prompt_pending() {
    use cyber_server::runtime::InputStatus;
    let flow = Flow::new(vec![text("done")], false);
    let path = configure(&flow, json!({}));
    require_server(&flow, &path, 30);
    let id = flow.session("default").await;
    flow.prompt(&id, "continue after interruption").await;
    wait_for_spawn(&flow).await;
    tokio::time::timeout(Duration::from_secs(1), flow.runtime.interrupt(&id))
        .await
        .unwrap()
        .unwrap();
    assert!(flow.main.requests().is_empty());
    assert_eq!(
        flow.runtime.state(&id).await.unwrap().inbox[0].status,
        InputStatus::Pending
    );
    assert_eq!(
        flow.f.host.mcp_status(&flow.f.repo).unwrap()[0].status,
        cyber_server::runtime::McpConnectionStatus::Connecting
    );
    ready(&flow, &id).await;
    flow.runtime.wake(&id).await.unwrap();
    flow.settle(&id).await;
    assert_eq!(flow.main.requests().len(), 1);
    assert_eq!(flow.f.read("spawned").lines().count(), 1);
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn per_server_cap_bounds_success_error_and_structured_text_without_losing_full_output() {
    let flow = Flow::new(vec![text("unused")], false);
    let path = configure(&flow, json!({}));
    let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["mcp"]["shared"]["output_token_limit"] = json!(2);
    std::fs::write(&path, config.to_string()).unwrap();
    flow.f.set_config(config);
    flow.f.write("tool-text", &"🦀".repeat(20));
    let id = flow.session("default").await;
    let defs = ready(&flow, &id).await;
    for name in ["read", "error", "structured"] {
        let def = defs
            .iter()
            .find(|def| def.spec.name == format!("mcp__shared__{name}"))
            .unwrap();
        let outcome = invoke(&flow, &id, def, "default").await;
        let output = match outcome {
            ToolOutcome::Ok(output) if name == "read" => output,
            ToolOutcome::Failed(output) if name == "error" => output,
            ToolOutcome::Structured { output, value } if name == "structured" => {
                assert_eq!(value, json!({"answer":42}));
                output
            }
            other => panic!("unexpected {name} outcome: {other:?}"),
        };
        assert!(output.starts_with(&format!("{}\n[output truncated:", "🦀".repeat(8))));
        let path = output
            .rsplit("full output at ")
            .next()
            .unwrap()
            .trim_end_matches(']');
        assert_eq!(std::fs::read_to_string(path).unwrap(), "🦀".repeat(20));
    }
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn explicit_server_timeout_bounds_runtime_calls_and_settles_native_owner() {
    let flow = Flow::new(vec![text("unused")], false);
    let path = configure(&flow, json!({}));
    let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["mcp"]["shared"]["timeout"] = json!(1);
    std::fs::write(&path, config.to_string()).unwrap();
    flow.f.set_config(config);
    let id = flow.session("default").await;
    let defs = ready(&flow, &id).await;
    let def = defs
        .iter()
        .find(|def| def.spec.name == "mcp__shared__read")
        .unwrap();
    flow.f.write("block-call", "block");
    let outcome =
        tokio::time::timeout(Duration::from_secs(3), invoke(&flow, &id, def, "default")).await;
    flow.runtime.shutdown().await;
    assert!(matches!(outcome,Ok(ToolOutcome::Failed(error)) if error.contains("timed out")));
    let owners = mcp_connections(&flow.f.store, &flow.f.repo).unwrap();
    assert!(
        owners
            .iter()
            .all(|owner| owner.phase == McpConnectionPhase::Settled),
        "connection phases: {:?}",
        owners
            .iter()
            .map(|owner| (&owner.id, &owner.phase))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn location_close_allows_sigterm_cleanup_before_forcing_the_owned_group() {
    let flow = Flow::new(vec![text("unused")], false);
    flow.f.write("graceful", "enabled");
    flow.f.write("graceful-children", "enabled");
    configure(&flow, json!({}));
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    let started = tokio::time::Instant::now();
    flow.f.host.close_mcp_location(&flow.f.repo).await.unwrap();
    assert!(started.elapsed() >= Duration::from_secs(5));
    assert_eq!(flow.f.read("graceful-finished"), "SIGTERM cleanup");
    assert_eq!(flow.f.read("child-finished"), "cleanup");
    tokio::time::sleep(Duration::from_millis(2250)).await;
    assert!(!flow.f.repo.join("escaped").exists());
    let owners = mcp_connections(&flow.f.store, &flow.f.repo).unwrap();
    assert!(
        owners
            .iter()
            .all(|owner| owner.phase == McpConnectionPhase::Settled),
        "connection phases: {:?}",
        owners
            .iter()
            .map(|owner| (&owner.id, &owner.phase))
            .collect::<Vec<_>>()
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn location_close_graces_multiple_servers_concurrently() {
    let flow = Flow::new(vec![text("unused")], false);
    let path = configure(&flow, json!({}));
    let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["mcp"]["other"] = config["mcp"]["shared"].clone();
    std::fs::write(&path, config.to_string()).unwrap();
    flow.f.set_config(config);
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while flow
            .f
            .host
            .definitions(&turn(&flow, &id))
            .iter()
            .filter(|tool| tool.spec.name.starts_with("mcp__"))
            .count()
            != 8
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(8),
        flow.f.host.close_mcp_location(&flow.f.repo),
    )
    .await;
    // If the observation times out, finish the retained close before asserting.
    if result.is_err() {
        flow.f.host.close_mcp_location(&flow.f.repo).await.unwrap();
    }
    assert!(result.is_ok());
    result.unwrap().unwrap();
    let owners = mcp_connections(&flow.f.store, &flow.f.repo).unwrap();
    assert!(
        owners
            .iter()
            .all(|owner| owner.phase == McpConnectionPhase::Settled),
        "connection phases: {:?}",
        owners
            .iter()
            .map(|owner| (&owner.id, &owner.phase))
            .collect::<Vec<_>>()
    );
    flow.runtime.shutdown().await;
}

fn form_params() -> Value {
    json!({"message":"Choose the deployment environment","requestedSchema":{"type":"object","properties":{"environment":{"type":"string","enum":["staging","prod"]}},"required":["environment"]}})
}

async fn elicitation_flow(interactive: bool) -> (Flow, String, PathBuf) {
    let flow = Flow::new(
        vec![
            call("c1", "mcp__shared__read", json!({})),
            text("done"),
            text("later"),
        ],
        interactive,
    );
    let path = configure(&flow, json!({}));
    flow.f.write("elicitation.json", &form_params().to_string());
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    flow.prompt(&id, "use the server").await;
    (flow, id, path)
}

#[tokio::test]
async fn elicitation_unattended_declines_without_pending_question_or_values() {
    let (flow, id, _) = elicitation_flow(false).await;
    flow.settle(&id).await;
    let reply: Value = serde_json::from_str(&flow.f.read("elicitation-reply")).unwrap();
    assert_eq!(
        reply,
        json!({"jsonrpc":"2.0","id":"form","result":{"action":"decline"}})
    );
    assert!(flow.runtime.pending_requests(Some(&id)).is_empty());
    let capabilities: Value =
        serde_json::from_str(&flow.f.read("elicitation-capabilities")).unwrap();
    assert_eq!(capabilities["elicitation"], json!({"form":{}}));
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn elicitation_question_flow_accepts_declines_and_cancels_with_server_attribution() {
    use cyber_server::runtime::{PendingKind, QuestionReply};
    for action in ["Accept", "Decline", "Cancel"] {
        let (flow, id, _) = elicitation_flow(true).await;
        let pending = flow.pending(&id).await;
        let PendingKind::Question { questions } = pending.kind else {
            panic!("expected form");
        };
        assert_eq!(pending.call_id, "c1");
        assert!(
            questions
                .iter()
                .all(|question| question.question.contains("MCP server shared"))
        );
        flow.runtime
            .answer_question(
                &pending.id,
                QuestionReply::Answers {
                    answers: vec![
                        vec![questions[0].options[0].label.clone()],
                        vec![action.into()],
                    ],
                },
            )
            .await
            .unwrap();
        flow.settle(&id).await;
        let reply: Value = serde_json::from_str(&flow.f.read("elicitation-reply")).unwrap();
        assert_eq!(reply["result"]["action"], action.to_lowercase());
        if action == "Accept" {
            assert_eq!(reply["result"]["content"], json!({"environment":"staging"}));
        } else {
            assert!(reply["result"].get("content").is_none());
        }
        assert!(flow.runtime.pending_requests(Some(&id)).is_empty());
        flow.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn elicitation_revoked_definition_cancels_before_sending_answers() {
    use cyber_server::runtime::{PendingKind, QuestionReply};
    let (flow, id, path) = elicitation_flow(true).await;
    let pending = flow.pending(&id).await;
    let PendingKind::Question { questions } = pending.kind else {
        panic!("expected form");
    };
    let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["mcp"]["shared"]["enabled"] = json!(false);
    std::fs::write(&path, config.to_string()).unwrap();
    flow.runtime
        .answer_question(
            &pending.id,
            QuestionReply::Answers {
                answers: vec![
                    vec![questions[0].options[0].label.clone()],
                    vec!["Accept".into()],
                ],
            },
        )
        .await
        .unwrap();
    flow.settle(&id).await;
    let reply: Value = serde_json::from_str(&flow.f.read("elicitation-reply")).unwrap();
    assert_eq!(reply["result"], json!({"action":"cancel"}));
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn location_close_cancels_owned_elicitation_questions_and_settles_native_owner() {
    let (flow, id, _) = elicitation_flow(true).await;
    flow.pending(&id).await;
    flow.f.host.close_mcp_location(&flow.f.repo).await.unwrap();
    flow.settle(&id).await;
    assert!(flow.runtime.pending_requests(Some(&id)).is_empty());
    assert!(!flow.f.repo.join("elicitation-reply").exists());
    let owners = mcp_connections(&flow.f.store, &flow.f.repo).unwrap();
    assert!(
        owners
            .iter()
            .all(|owner| owner.phase == McpConnectionPhase::Settled),
        "connection phases: {:?}",
        owners
            .iter()
            .map(|owner| (&owner.id, &owner.phase))
            .collect::<Vec<_>>()
    );
    flow.prompt(&id, "open the next Turn").await;
    flow.settle(&id).await;
    ready(&flow, &id).await;
    let next_turn = mcp_connections(&flow.f.store, &flow.f.repo).unwrap();
    assert!(
        next_turn
            .iter()
            .any(|owner| !owners.iter().any(|old| old.id == owner.id)
                && owner.phase != McpConnectionPhase::Settled)
    );
    flow.f.host.close_mcp_location(&flow.f.repo).await.unwrap();
    let later = flow.session("default").await;
    ready(&flow, &later).await;
    let reopened = mcp_connections(&flow.f.store, &flow.f.repo).unwrap();
    assert!(
        reopened
            .iter()
            .any(|owner| !owners.iter().any(|old| old.id == owner.id)
                && owner.phase != McpConnectionPhase::Settled)
    );
    assert!(!flow.f.repo.join("elicitation-reply").exists());
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn elicitation_human_time_pauses_call_inactivity_and_session_interrupt_cleans_requests() {
    use cyber_server::runtime::{PendingKind, QuestionReply};
    let flow = Flow::new(
        vec![call("c1", "mcp__shared__read", json!({})), text("done")],
        true,
    );
    let path = configure(&flow, json!({}));
    let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["mcp"]["tool_timeout"] = json!(1);
    std::fs::write(&path, config.to_string()).unwrap();
    flow.f.set_config(config);
    flow.f.write("elicitation.json", &form_params().to_string());
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    flow.prompt(&id, "ask").await;
    let request = flow.pending(&id).await;
    let PendingKind::Question { questions } = request.kind else {
        panic!("expected form");
    };
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(flow.runtime.pending_requests(Some(&id)).len(), 1);
    flow.runtime
        .answer_question(
            &request.id,
            QuestionReply::Answers {
                answers: vec![
                    vec![questions[0].options[0].label.clone()],
                    vec!["Accept".into()],
                ],
            },
        )
        .await
        .unwrap();
    flow.settle(&id).await;
    assert!(flow.output(&id, "c1").await.contains("accept"));
    flow.runtime.shutdown().await;
    let (flow, id, _) = elicitation_flow(true).await;
    flow.pending(&id).await;
    flow.runtime.interrupt(&id).await.unwrap();
    flow.settle(&id).await;
    assert!(flow.runtime.pending_requests(Some(&id)).is_empty());
    assert!(!flow.f.repo.join("elicitation-reply").exists());
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn sampling_routes_to_small_model_only_after_its_own_permission_and_bills_the_server() {
    use cyber_server::runtime::NoSnapshots;
    for (enabled, permission, expected) in [
        (true, "allow", true),
        (true, "deny", false),
        (false, "allow", false),
    ] {
        let flow = Flow::with_models(
            support::Fixture::new(),
            vec![call("c1", "mcp__shared__read", json!({})), text("done")],
            false,
            Arc::new(NoSnapshots),
            vec![("test/summary", vec![text("nested answer")])],
        );
        let path = configure(&flow, json!({}));
        let mut config: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        config["mcp"]["sampling"] = json!({"enabled":enabled});
        config["permissions"] = json!({"mcp_sampling":permission});
        std::fs::write(&path, config.to_string()).unwrap();
        flow.f.set_config(config);
        flow.f.write("sampling.json", &json!({"messages":[{"role":"user","content":{"type":"text","text":"private server input"}}],"maxTokens":42,"modelPreferences":{"hints":[{"name":"unconfigured"}]}}).to_string());
        let id = flow.session("default").await;
        ready(&flow, &id).await;
        flow.prompt(&id, "use the server").await;
        flow.settle(&id).await;
        let reply: Value = serde_json::from_str(&flow.f.read("sampling-reply")).unwrap();
        let requests = flow.requests("test/summary");
        assert_eq!(requests.len(), usize::from(expected));
        let events = flow.f.store.read_events(&id, -1, 500).unwrap().events;
        let usage: Vec<_> = events
            .iter()
            .filter(|event| event.kind == "usage.recorded.1")
            .collect();
        if expected {
            assert_eq!(reply["result"]["content"]["text"], "nested answer");
            assert_eq!(reply["result"]["model"], "summary");
            assert!(requests[0].tools_disabled && requests[0].tools.is_empty());
            assert_eq!(requests[0].max_output_tokens, Some(42));
            assert_eq!(
                requests[0].messages,
                vec![cyber_llm::Message::user_text("private server input")]
            );
            assert_eq!(usage.len(), 1);
            assert_eq!(usage[0].data["purpose"], "mcp_sampling:shared");
            assert_eq!(usage[0].data["call_id"], "c1");
        } else {
            assert!(reply.get("error").is_some());
            assert!(usage.is_empty());
            if !enabled {
                assert_eq!(reply["error"]["code"], -32601);
            }
        }
        assert!(flow.runtime.pending_requests(Some(&id)).is_empty());
        flow.f.host.shutdown().await;
    }
}

#[tokio::test]
async fn sampling_approval_rechecks_configuration_and_location_close_withdraws_its_request() {
    use cyber_server::runtime::{NoSnapshots, PendingKind, PermissionReply};
    for close in [false, true] {
        let flow = Flow::with_models(
            support::Fixture::new(),
            vec![call("c1", "mcp__shared__read", json!({})), text("done")],
            true,
            Arc::new(NoSnapshots),
            vec![("test/summary", vec![text("must not run")])],
        );
        let path = configure(&flow, json!({}));
        let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        config["mcp"]["sampling"] = json!({"enabled":true});
        config["mcp"]["tool_timeout"] = json!(1);
        config["permissions"] = json!({"mcp_sampling":"ask"});
        std::fs::write(&path, config.to_string()).unwrap();
        flow.f.set_config(config.clone());
        flow.f.write("sampling.json", &json!({"messages":[{"role":"user","content":{"type":"text","text":"private"}}],"maxTokens":42}).to_string());
        let id = flow.session("default").await;
        ready(&flow, &id).await;
        flow.prompt(&id, "ask").await;
        let pending = flow.pending(&id).await;
        assert!(
            matches!(&pending.kind, PendingKind::Permission(ask) if ask.action == "mcp_sampling" && ask.resources == ["shared"])
        );
        if close {
            tokio::time::timeout(
                Duration::from_secs(5),
                flow.f.host.close_mcp_location(&flow.f.repo),
            )
            .await
            .unwrap()
            .unwrap();
        } else {
            tokio::time::sleep(Duration::from_millis(1200)).await;
            config["mcp"]["sampling"]["enabled"] = json!(false);
            std::fs::write(&path, config.to_string()).unwrap();
            flow.f.set_config(config);
            flow.runtime
                .reply_permission(&pending.id, PermissionReply::Once)
                .await
                .unwrap();
        }
        flow.settle(&id).await;
        assert!(flow.requests("test/summary").is_empty());
        assert!(flow.runtime.pending_requests(Some(&id)).is_empty());
        if close {
            assert!(!flow.f.repo.join("sampling-reply").exists());
            assert!(
                mcp_connections(&flow.f.store, &flow.f.repo)
                    .unwrap()
                    .iter()
                    .all(|owner| owner.phase == McpConnectionPhase::Settled)
            );
        } else {
            let reply: Value = serde_json::from_str(&flow.f.read("sampling-reply")).unwrap();
            assert!(reply.get("error").is_some());
        }
        flow.runtime.shutdown().await;
    }
}

struct NativeSamplingModels {
    base: Arc<dyn cyber_server::runtime::ModelResolver>,
    adapter: Arc<dyn cyber_llm::Adapter>,
}
impl cyber_server::runtime::ModelResolver for NativeSamplingModels {
    fn role(&self, role: cyber_llm::catalog::ModelRole) -> Option<String> {
        if role == cyber_llm::catalog::ModelRole::Small {
            Some("native/small".into())
        } else {
            self.base.role(role)
        }
    }
    fn resolve(&self, reference: &str) -> Result<cyber_server::runtime::ResolvedModel, String> {
        if reference != "native/small" {
            return self.base.resolve(reference);
        }
        Ok(cyber_server::runtime::ResolvedModel {
            adapter: self.adapter.clone(),
            provider: "native".into(),
            model: "small".into(),
            template: cyber_llm::LlmRequest {
                model: "small".into(),
                max_output_tokens: Some(100),
                ..Default::default()
            },
            context_limit: 200000,
            cost: None,
            prefers_apply_patch: false,
        })
    }
}

async fn stalled_sampling_provider() -> (
    String,
    tokio::sync::oneshot::Receiver<()>,
    tokio::task::JoinHandle<Value>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (send, received) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = sampling_provider_request(&mut socket).await;
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n: keepalive\n\n").await.unwrap();
        let _ = send.send(());
        let mut byte = [0];
        let read = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut byte))
            .await
            .unwrap();
        assert!(
            matches!(read, Ok(0) | Err(_)),
            "sampling provider socket remained open"
        );
        request
    });
    (format!("http://{address}/v1"), received, task)
}
async fn sampling_provider_request(socket: &mut tokio::net::TcpStream) -> Value {
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let n = socket.read(&mut chunk).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&bytes[..end]);
            let length: usize = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|value| value.trim().parse().unwrap())
                })
                .unwrap();
            if bytes.len() >= end + 4 + length {
                return serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap();
            }
        }
        assert!(bytes.len() <= 2 * 1024 * 1024);
    }
}

#[tokio::test]
async fn sampling_native_provider_timeout_and_location_close_stop_sockets_before_settlement() {
    use cyber_llm::adapters::{ApiKind, Endpoint, adapter};
    use cyber_server::runtime::NoSnapshots;
    for kind in [
        ApiKind::OpenaiCompatible,
        ApiKind::OpenaiResponses,
        ApiKind::Anthropic,
    ] {
        for close in [false, true] {
            let (url, received, served) = stalled_sampling_provider().await;
            let native: Arc<dyn cyber_llm::Adapter> = Arc::from(adapter(
                kind,
                Endpoint::new(url, Some("private-provider-credential".into())),
            ));
            let flow = Flow::with_resolver_factory(
                support::Fixture::new(),
                vec![call("c1", "mcp__shared__read", json!({})), text("done")],
                false,
                Arc::new(NoSnapshots),
                vec![],
                move |base| {
                    Arc::new(NativeSamplingModels {
                        base,
                        adapter: native,
                    })
                },
            );
            let path = configure(&flow, json!({}));
            let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            config["mcp"]["sampling"] = json!({"enabled":true});
            config["mcp"]["tool_timeout"] = json!(if close { 30 } else { 1 });
            config["permissions"] = json!({"mcp_sampling":"allow"});
            std::fs::write(&path, config.to_string()).unwrap();
            flow.f.set_config(config);
            flow.f.write("sampling.json", &json!({"messages":[{"role":"user","content":{"type":"text","text":"private-server-input"}}],"maxTokens":42}).to_string());
            let id = flow.session("default").await;
            ready(&flow, &id).await;
            flow.prompt(&id, "sample").await;
            tokio::time::timeout(Duration::from_secs(5), received)
                .await
                .unwrap()
                .unwrap();
            if close {
                tokio::time::timeout(
                    Duration::from_secs(5),
                    flow.f.host.close_mcp_location(&flow.f.repo),
                )
                .await
                .unwrap()
                .unwrap();
            }
            flow.settle(&id).await;
            let request = tokio::time::timeout(Duration::from_secs(5), served)
                .await
                .unwrap()
                .unwrap();
            assert!(request.get("tools").is_none());
            assert!(request.to_string().contains("private-server-input"));
            let events = flow.f.store.read_events(&id, -1, 500).unwrap().events;
            let usage: Vec<_> = events
                .iter()
                .filter(|event| event.kind == "usage.recorded.1")
                .collect();
            assert_eq!(usage.len(), 1);
            assert_eq!(usage[0].data["purpose"], "mcp_sampling:shared");
            assert!(usage[0].data["cost"].is_null());
            let recorded = events
                .iter()
                .map(|e| e.data.to_string())
                .collect::<String>();
            assert!(!recorded.contains("private-provider-credential"));
            assert!(!recorded.contains("private-server-input"));
            assert!(flow.runtime.pending_requests(Some(&id)).is_empty());
            if !close {
                let reply: Value = serde_json::from_str(&flow.f.read("sampling-reply")).unwrap();
                assert!(
                    reply["error"]["message"]
                        .as_str()
                        .unwrap()
                        .contains("timed out")
                );
            }
            flow.runtime.shutdown().await;
        }
    }
}

#[tokio::test]
async fn sampling_checks_session_and_ancestor_budgets_after_approval() {
    use cyber_server::runtime::{AuxiliaryUsage, CreateSession, NoSnapshots, PermissionReply};
    for ancestor in [false, true] {
        let flow = Flow::with_models(
            support::Fixture::new(),
            vec![call("c1", "mcp__shared__read", json!({})), text("done")],
            true,
            Arc::new(NoSnapshots),
            vec![("test/summary", vec![text("must not run")])],
        );
        let path = configure(&flow, json!({}));
        let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        config["mcp"]["sampling"] = json!({"enabled":true});
        config["permissions"] = json!({"mcp_sampling":"ask"});
        std::fs::write(&path, config.to_string()).unwrap();
        flow.f.set_config(config);
        flow.f.write("sampling.json",&json!({"messages":[{"role":"user","content":{"type":"text","text":"private"}}],"maxTokens":42}).to_string());
        let root = flow
            .runtime
            .create_session(CreateSession {
                directory: flow.f.repo.display().to_string(),
                model: "test/main".into(),
                mode: Some("default".into()),
                budget: Some(
                    serde_json::from_value(json!({"max_tokens":150,"enforcement":"soft"})).unwrap(),
                ),
                ..Default::default()
            })
            .await
            .unwrap()
            .id;
        let id = if ancestor {
            flow.runtime
                .create_session(CreateSession {
                    directory: flow.f.repo.display().to_string(),
                    model: "test/main".into(),
                    mode: Some("default".into()),
                    parent_id: Some(root.clone()),
                    ..Default::default()
                })
                .await
                .unwrap()
                .id
        } else {
            root.clone()
        };
        ready(&flow, &id).await;
        flow.prompt(&id, "sample").await;
        let pending = flow.pending(&id).await;
        flow.runtime
            .operation_asker(&id)
            .await
            .unwrap()
            .record_model_usage(AuxiliaryUsage {
                provider: "test".into(),
                model: "test/main".into(),
                purpose: "fixture".into(),
                call_id: None,
                duration_ms: 0,
                usage: cyber_llm::Usage {
                    input: 50,
                    ..Default::default()
                },
                cost: Some(0.0),
            })
            .await
            .unwrap();
        flow.runtime
            .reply_permission(&pending.id, PermissionReply::Once)
            .await
            .unwrap();
        flow.settle(&id).await;
        assert!(flow.requests("test/summary").is_empty());
        let reply: Value = serde_json::from_str(&flow.f.read("sampling-reply")).unwrap();
        assert_eq!(reply["error"]["message"], "Sampling budget refused");
        let events = flow.f.store.read_events(&id, -1, 500).unwrap().events;
        assert!(
            events
                .iter()
                .any(|e| e.kind == "budget.exceeded.1" && e.data["scope_id"] == root)
        );
        assert!(!events.iter().any(|e|e.kind == "usage.recorded.1" && e.data["purpose"] == "mcp_sampling:shared"));
        assert!(flow.runtime.pending_requests(Some(&id)).is_empty());
        flow.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn sampling_records_usage_but_withholds_response_when_the_soft_budget_is_exceeded() {
    use cyber_server::runtime::{CreateSession, NoSnapshots};
    let flow = Flow::with_models(
        support::Fixture::new(),
        vec![call("c1", "mcp__shared__read", json!({})), text("done")],
        false,
        Arc::new(NoSnapshots),
        vec![("test/summary", vec![text("private-nested-answer")])],
    );
    let path = configure(&flow, json!({}));
    let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["mcp"]["sampling"] = json!({"enabled":true});
    config["permissions"] = json!({"mcp_sampling":"allow"});
    std::fs::write(&path, config.to_string()).unwrap();
    flow.f.set_config(config);
    flow.f.write("sampling.json",&json!({"messages":[{"role":"user","content":{"type":"text","text":"private"}}],"maxTokens":42}).to_string());
    let id = flow
        .runtime
        .create_session(CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            mode: Some("default".into()),
            budget: Some(
                serde_json::from_value(json!({"max_tokens":150,"enforcement":"soft"})).unwrap(),
            ),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    ready(&flow, &id).await;
    flow.prompt(&id, "sample").await;
    flow.settle(&id).await;
    assert_eq!(flow.requests("test/summary").len(), 1);
    let reply: Value = serde_json::from_str(&flow.f.read("sampling-reply")).unwrap();
    assert_eq!(reply["error"]["message"], "Sampling budget exceeded");
    assert!(!reply.to_string().contains("private-nested-answer"));
    let events = flow.f.store.read_events(&id, -1, 500).unwrap().events;
    assert!(events.iter().any(|e| e.kind == "usage.recorded.1"
        && e.data["purpose"] == "mcp_sampling:shared"
        && e.data["usage"]["input"] == 100));
    assert!(
        !events
            .iter()
            .any(|e| e.data.to_string().contains("private-nested-answer"))
    );
    assert_eq!(
        flow.runtime.session_usage(&id).unwrap().own.total_tokens,
        215
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn sampling_malformed_and_immediately_denied_callbacks_cannot_extend_call_inactivity() {
    for malformed in [false, true] {
        let flow = Flow::new(vec![text("unused")], false);
        let path = configure(&flow, json!({}));
        let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        config["mcp"]["sampling"] = json!({"enabled":true});
        config["mcp"]["tool_timeout"] = json!(1);
        config["permissions"] = json!({"mcp_sampling":"deny"});
        std::fs::write(&path, config.to_string()).unwrap();
        flow.f.set_config(config);
        flow.f.write("sampling.json",&if malformed {json!({"messages":[],"maxTokens":0})} else {json!({"messages":[{"role":"user","content":{"type":"text","text":"private"}}],"maxTokens":42})}.to_string());
        flow.f.write("sampling-loop", "loop");
        let id = flow.session("default").await;
        let defs = ready(&flow, &id).await;
        let def = defs
            .iter()
            .find(|def| def.spec.name == "mcp__shared__read")
            .unwrap();
        let result =
            tokio::time::timeout(Duration::from_secs(3), invoke(&flow, &id, def, "default")).await;
        assert!(matches!(result,Ok(ToolOutcome::Failed(error)) if error.contains("timed out")));
        assert!(
            mcp_connections(&flow.f.store, &flow.f.repo)
                .unwrap()
                .iter()
                .all(|owner| owner.phase == McpConnectionPhase::Settled)
        );
        assert!(flow.requests("test/summary").is_empty());
        flow.runtime.shutdown().await;
    }
}

const HOOK_AUDIT_SERVER: &str = r#"
import json,sys,os,pathlib,time
for line in sys.stdin:
    request=json.loads(line)
    if 'id' not in request: continue
    if request['method']=='initialize':
        with open('hook-servers','a') as f: f.write(str(os.getpid())+'\n')
        result={'protocolVersion':'2025-11-25','capabilities':{'tools':{}},'serverInfo':{'name':'audit','version':'1'}}
    elif request['method']=='tools/list':
        schema=json.loads(pathlib.Path('hook-schema').read_text()) if pathlib.Path('hook-schema').exists() else {'type':'object'}
        result={'tools':[{'name':'record','inputSchema':schema},{'name':'read','inputSchema':{'type':'object'},'annotations':{'readOnlyHint':True}}]}
    else:
        if request['params']['name']=='read':
            print(json.dumps({'jsonrpc':'2.0','id':'sample','method':'sampling/createMessage','params':{'messages':[{'role':'user','content':{'type':'text','text':'audit sample'}}],'maxTokens':42}}),flush=True)
            reply=json.loads(sys.stdin.readline())
            pathlib.Path('sampling-reply').write_text(json.dumps(reply))
            result={'content':[{'type':'text','text':json.dumps(reply)}]}
        else:
            pathlib.Path('hook-input').write_text(json.dumps(request['params']['arguments']))
            while pathlib.Path('hook-block').exists(): time.sleep(.01)
            result=json.loads(pathlib.Path('hook-answer').read_text())
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}),flush=True)
"#;

fn configure_mcp_hook(flow: &Flow, hooks: Value) -> config::Resolved {
    let path = configure(flow, hooks);
    let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["mcp"]["shared"]["args"] = json!(["-u", "-c", HOOK_AUDIT_SERVER]);
    std::fs::write(path, value.to_string()).unwrap();
    flow.f.set_config(value);
    reload_mcp_hook(flow)
}

fn reload_mcp_hook(flow: &Flow) -> config::Resolved {
    let home = flow.f.dir.path();
    let env = HashMap::from([(
        "CYBER_HOME".into(),
        home.join("cyber").display().to_string(),
    )]);
    let paths = Paths::resolve(&env, home);
    config::load(&LoadRequest {
        location: &flow.f.repo,
        paths: &paths,
        env: &env,
        home,
        profile: None,
        overrides: &[],
        flags: json!({}),
    })
    .unwrap()
}

#[tokio::test]
async fn mcp_hook_blocks_builtin_effects_through_a_dedicated_recorded_connection() {
    let flow = Flow::new(
        vec![
            call(
                "c1",
                "write",
                json!({"path":"blocked.txt","content":"unsafe"}),
            ),
            text("done"),
        ],
        false,
    );
    configure_mcp_hook(
        &flow,
        json!({"PreToolUse":[{"matcher":"write","hooks":[{"type":"mcp_tool","server":"shared","tool":"record","fail_closed":true}]}]}),
    );
    flow.f.write("hook-answer",&json!({"content":[{"type":"text","text":"{\"decision\":\"deny\",\"reason\":\"audit policy\"}"}]}).to_string());
    let id = flow.session("bypass").await;
    ready(&flow, &id).await;
    flow.prompt(&id, "write").await;
    flow.settle(&id).await;
    assert!(!flow.f.repo.join("blocked.txt").exists());
    let input: Value = serde_json::from_str(&flow.f.read("hook-input")).unwrap();
    assert_eq!(input["tool_name"], "write");
    assert_eq!(input["tool_input"]["path"], "blocked.txt");
    let receipts = flow.runtime.hook_executions(&id, 20).unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(
        receipts[0].outcome,
        Some(cyber_core::hooks::HookOutcome::Blocked)
    );
    assert_eq!(receipts[0].acknowledged, Some(true));
    assert!(receipts[0].io.is_none());
    let servers = flow.f.read("hook-servers");
    let pids: std::collections::HashSet<_> = servers.lines().collect();
    assert!(
        pids.len() >= 2,
        "hook borrowed the shared native connection"
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn mcp_hook_synthetic_templates_preserve_types_and_once_without_session_admission() {
    use cyber_core::hooks::{HookEvent, HookLocation};
    use cyber_tools::hook_commands::HookCommandRunner;
    let flow = Flow::new(vec![text("unused")], false);
    let resolved = configure_mcp_hook(
        &flow,
        json!({"PreToolUse":[{"matcher":"write","hooks":[{"type":"mcp_tool","server":"shared","tool":"record","once":true,"arguments":{"count":"${tool_input.count}","input":"${tool_input}","label":"call ${tool_name}"}}]}]}),
    );
    flow.f.write(
        "hook-answer",
        &json!({"content":[{"type":"text","text":"{\"decision\":\"allow\"}"}]}).to_string(),
    );
    let trust = TrustStore::new(flow.f.dir.path().join("trust.json"));
    let settings = HookCommandRunner {
        resolved: &resolved,
        trust: &trust,
        invocation_trust: None,
        home: flow.f.dir.path(),
        temp_dir: &flow.f.dir.path().join("hook-scratch"),
        shell: "bash",
        helper: None,
        credential_env_names: &[],
    };
    let event = HookEvent::synthetic(
        "PreToolUse",
        HookLocation {
            directory: flow.f.repo.clone(),
            workspace: None,
        },
        "global".into(),
        "build".into(),
        "default".into(),
        1,
        json!({"tool_name":"write","tool_input":{"count":2}}),
    )
    .unwrap();
    let first = flow
        .f
        .host
        .test_hooks(
            &settings,
            event.clone(),
            CancellationToken::new(),
            &|_, _| {},
        )
        .await
        .unwrap();
    assert!(first.complete);
    assert_eq!(
        first.results[0].report.outcome,
        cyber_core::hooks::HookOutcome::Ok
    );
    let input: Value = serde_json::from_str(&flow.f.read("hook-input")).unwrap();
    assert_eq!(
        input,
        json!({"count":2,"input":{"count":2},"label":"call write"})
    );
    let second = flow
        .f
        .host
        .test_hooks(&settings, event, CancellationToken::new(), &|_, _| {})
        .await
        .unwrap();
    assert!(second.complete);
    assert_eq!(
        second.results[0].report.outcome,
        cyber_core::hooks::HookOutcome::Skipped
    );
    assert_eq!(flow.f.read("hook-servers").lines().count(), 1);
    assert_eq!(
        flow.f
            .store
            .read(
                |conn| Ok(conn.query_row("SELECT COUNT(*) FROM session", [], |row| row
                    .get::<_, i64>(0))?)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        std::fs::read_dir(flow.f.dir.path().join("hook-scratch"))
            .unwrap()
            .count(),
        0
    );
    flow.runtime.shutdown().await;
}

fn mcp_hook_event(flow: &Flow) -> cyber_core::hooks::HookEvent {
    cyber_core::hooks::HookEvent::synthetic(
        "PreToolUse",
        cyber_core::hooks::HookLocation {
            directory: flow.f.repo.clone(),
            workspace: None,
        },
        "global".into(),
        "build".into(),
        "default".into(),
        1,
        json!({"tool_name":"write","tool_input":{"private":"private hook input"}}),
    )
    .unwrap()
}

fn mcp_hook_settings<'a>(
    flow: &'a Flow,
    resolved: &'a config::Resolved,
    trust: &'a TrustStore,
    scratch: &'a std::path::Path,
) -> cyber_tools::hook_commands::HookCommandRunner<'a> {
    cyber_tools::hook_commands::HookCommandRunner {
        resolved,
        trust,
        invocation_trust: None,
        home: flow.f.dir.path(),
        temp_dir: scratch,
        shell: "bash",
        helper: None,
        credential_env_names: &[],
    }
}

#[tokio::test]
async fn mcp_hook_rejects_ambiguous_error_and_non_json_replies_without_retaining_private_io() {
    use cyber_core::hooks::HookOutcome;
    let replies = [
        json!({"content":[]}),
        json!({"content":[{"type":"text","text":"{}"},{"type":"text","text":"{}"}]}),
        json!({"content":[{"type":"text","text":"private invalid output"}]}),
        json!({"isError":true,"content":[{"type":"text","text":"{\"decision\":\"allow\"}"}]}),
    ];
    for fail_closed in [false, true] {
        for reply in &replies {
            let flow = Flow::new(vec![text("unused")], false);
            let resolved = configure_mcp_hook(
                &flow,
                json!({"PreToolUse":[{"hooks":[{"type":"mcp_tool","server":"shared","tool":"record","fail_closed":fail_closed}]}]}),
            );
            flow.f.write("hook-answer", &reply.to_string());
            let trust = TrustStore::new(flow.f.dir.path().join("trust.json"));
            let scratch = flow.f.dir.path().join("hook-scratch");
            let settings = mcp_hook_settings(&flow, &resolved, &trust, &scratch);
            let result = flow
                .f
                .host
                .test_hooks(
                    &settings,
                    mcp_hook_event(&flow),
                    CancellationToken::new(),
                    &|_, _| {},
                )
                .await
                .unwrap();
            assert!(!result.complete);
            let report = &result.results[0].report;
            assert_eq!(report.outcome, HookOutcome::Error);
            assert!(report.acknowledged);
            assert_eq!(
                report.decision.decision,
                fail_closed.then_some(cyber_core::hooks::HookAction::Deny)
            );
            let receipts: Vec<String> = flow
                .f
                .store
                .read(|conn| {
                    let mut query = conn.prepare("SELECT data FROM hook_test_execution")?;
                    Ok(query
                        .query_map([], |row| row.get::<_, String>(0))?
                        .collect::<Result<_, _>>()?)
                })
                .unwrap();
            assert_eq!(receipts.len(), 1);
            for secret in ["private hook input", "private invalid output"] {
                assert!(!receipts[0].contains(secret));
            }
            assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
            flow.runtime.shutdown().await;
        }
    }
}

#[tokio::test]
async fn mcp_hook_cancellation_and_live_server_changes_settle_before_scratch_cleanup() {
    use cyber_core::hooks::HookOutcome;
    for change in ["cancel", "server", "hook"] {
        let revoke = change != "cancel";
        let flow = Flow::new(vec![text("unused")], false);
        let resolved = configure_mcp_hook(
            &flow,
            json!({"PreToolUse":[{"hooks":[{"type":"mcp_tool","server":"shared","tool":"record","fail_closed":true}]}]}),
        );
        flow.f.write("hook-block", "blocked");
        flow.f.write(
            "hook-answer",
            &json!({"content":[{"type":"text","text":"{\"decision\":\"allow\"}"}]}).to_string(),
        );
        let trust = TrustStore::new(flow.f.dir.path().join("trust.json"));
        let scratch = flow.f.dir.path().join("hook-scratch");
        let settings = mcp_hook_settings(&flow, &resolved, &trust, &scratch);
        let event = mcp_hook_event(&flow);
        let stop = CancellationToken::new();
        let execution = flow
            .f
            .host
            .test_hooks(&settings, event, stop.clone(), &|_, _| {});
        let mutate = async {
            tokio::time::timeout(Duration::from_secs(5), async {
                while !flow.f.repo.join("hook-input").exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            assert!(std::fs::read_dir(&scratch).unwrap().next().is_some());
            if revoke {
                let path = flow.f.dir.path().join("cyber/config/cyber.jsonc");
                let mut config: Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                if change == "server" {
                    config["mcp"]["shared"]["env"] = json!({"NEW_BINDING":"changed"});
                } else {
                    config["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"] = json!(2);
                }
                std::fs::write(path, config.to_string()).unwrap();
                std::fs::remove_file(flow.f.repo.join("hook-block")).unwrap();
            } else {
                stop.cancel();
            }
        };
        let (result, ()) = tokio::join!(execution, mutate);
        let result = result.unwrap();
        assert!(!result.complete);
        let report = &result.results[0].report;
        assert!(report.acknowledged);
        assert_eq!(
            report.outcome,
            if revoke {
                HookOutcome::Error
            } else {
                HookOutcome::Skipped
            }
        );
        assert_ne!(
            report.decision.decision,
            Some(cyber_core::hooks::HookAction::Allow)
        );
        assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
        let receipt: String = flow
            .f
            .store
            .read(|conn| {
                Ok(conn.query_row("SELECT data FROM hook_test_execution", [], |row| row.get(0))?)
            })
            .unwrap();
        let receipt: cyber_server::runtime::HookExecutionRecord =
            serde_json::from_str(&receipt).unwrap();
        assert_eq!(receipt.acknowledged, Some(true));
        assert_eq!(receipt.outcome, Some(report.outcome));
        flow.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn mcp_hook_schema_filter_and_absolute_timeout_fail_before_accepting_a_decision() {
    use cyber_core::hooks::HookOutcome;
    for case in ["schema", "filter", "timeout"] {
        let flow = Flow::new(vec![text("unused")], false);
        let mut resolved = configure_mcp_hook(
            &flow,
            json!({"PreToolUse":[{"hooks":[{"type":"mcp_tool","server":"shared","tool":"record","timeout":1,"fail_closed":true}]}]}),
        );
        if case == "schema" {
            flow.f.write("hook-schema", &json!({"type":"object","required":["required_argument"],"properties":{"required_argument":{"type":"string"}}}).to_string());
        } else if case == "filter" {
            let path = flow.f.dir.path().join("cyber/config/cyber.jsonc");
            let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            config["mcp"]["shared"]["tools"] = json!({"deny":["record"]});
            std::fs::write(path, config.to_string()).unwrap();
            resolved = reload_mcp_hook(&flow);
        } else {
            flow.f.write("hook-block", "blocked");
        }
        flow.f.write(
            "hook-answer",
            &json!({"content":[{"type":"text","text":"{\"decision\":\"allow\"}"}]}).to_string(),
        );
        let trust = TrustStore::new(flow.f.dir.path().join("trust.json"));
        let scratch = flow.f.dir.path().join("hook-scratch");
        let settings = mcp_hook_settings(&flow, &resolved, &trust, &scratch);
        let started = std::time::Instant::now();
        let result = flow
            .f
            .host
            .test_hooks(
                &settings,
                mcp_hook_event(&flow),
                CancellationToken::new(),
                &|_, _| {},
            )
            .await
            .unwrap();
        assert!(!result.complete);
        let report = &result.results[0].report;
        assert_eq!(
            report.outcome,
            if case == "timeout" {
                HookOutcome::Timeout
            } else {
                HookOutcome::Error
            }
        );
        assert!(report.acknowledged);
        assert_eq!(
            report.decision.decision,
            Some(cyber_core::hooks::HookAction::Deny)
        );
        assert!(started.elapsed() < Duration::from_secs(4));
        assert_eq!(flow.f.repo.join("hook-input").exists(), case == "timeout");
        assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
        flow.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn mcp_hook_permission_callback_uses_dedicated_connection_without_reentrant_deadlock() {
    use cyber_server::runtime::NoSnapshots;
    let flow = Flow::with_models(
        support::Fixture::new(),
        vec![call("c1", "mcp__shared__read", json!({})), text("done")],
        true,
        Arc::new(NoSnapshots),
        vec![("test/summary", vec![text("approved nested answer")])],
    );
    configure_mcp_hook(
        &flow,
        json!({"PermissionRequest":[{"matcher":"mcp__shared__read","hooks":[{"type":"mcp_tool","server":"shared","tool":"record","fail_closed":true}]}]}),
    );
    let path = flow.f.dir.path().join("cyber/config/cyber.jsonc");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["mcp"]["sampling"] = json!({"enabled":true});
    config["permissions"] = json!({"mcp_sampling":"ask"});
    std::fs::write(path, config.to_string()).unwrap();
    flow.f.set_config(config);
    flow.f.write(
        "hook-answer",
        &json!({"content":[{"type":"text","text":"{\"decision\":\"allow\"}"}]}).to_string(),
    );
    let id = flow.session("default").await;
    ready(&flow, &id).await;
    flow.prompt(&id, "read").await;
    tokio::time::timeout(Duration::from_secs(5), flow.settle(&id))
        .await
        .unwrap();
    assert!(flow.runtime.pending_requests(Some(&id)).is_empty());
    assert_eq!(flow.requests("test/summary").len(), 1);
    let reply: Value = serde_json::from_str(&flow.f.read("sampling-reply")).unwrap();
    assert_eq!(reply["result"]["content"]["text"], "approved nested answer");
    let receipts = flow.runtime.hook_executions(&id, 20).unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].acknowledged, Some(true));
    let servers = flow.f.read("hook-servers");
    assert!(
        servers
            .lines()
            .collect::<std::collections::HashSet<_>>()
            .len()
            >= 2
    );
    flow.runtime.shutdown().await;
}
