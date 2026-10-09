//! Real configured local MCP launches, confinement and unknown scratch ownership.
#![cfg(unix)]
use cyber_core::config::{self, LoadRequest, McpSettings, Resolved};
use cyber_core::paths::Paths;
use cyber_core::trust::TrustStore;
use cyber_tools::mcp::LocalLauncher;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

const SERVER: &str = r#"
import json,sys,os,pathlib,time
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request: continue
    if request['method'] == 'initialize':
        result = {'protocolVersion':'2025-11-25','capabilities':{'tools':{}},'serverInfo':{'name':'fixture','version':'1'}}
    elif request['method'] == 'tools/list':
        result = {'tools':[{'name':'record','inputSchema':{'type':'object'},'annotations':{'readOnlyHint':False}}, {'name':'denied','inputSchema':{'type':'object'}}]}
    else:
        writable = True
        try: pathlib.Path('called').write_text(request['params']['name'])
        except OSError: writable = False
        credential_read = True
        try: pathlib.Path(os.environ['CREDENTIAL_FILE']).read_text()
        except OSError: credential_read = False
        result = {'content':[{'type':'text','text':json.dumps({'cwd':os.getcwd(),'explicit':os.environ.get('EXPLICIT'),'tmp':os.environ.get('TMPDIR'),'proxy':os.environ.get('HTTP_PROXY'),'path':os.environ.get('PATH'),'writable':writable,'credential_read':credential_read})}]}
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}),flush=True)
"#;

struct Fixture {
    _root: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
    temp: PathBuf,
    paths: Paths,
    env: HashMap<String, String>,
}
impl Fixture {
    fn new(policy: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let repo = root.path().join("repo");
        let temp = root.path().join("scratch");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(home.join(".aws")).unwrap();
        std::fs::write(home.join(".aws/credential"), "private credential").unwrap();
        let env = HashMap::from([(
            "CYBER_HOME".into(),
            root.path().join("cyber").display().to_string(),
        )]);
        let paths = Paths::resolve(&env, &home);
        paths.ensure().unwrap();
        std::fs::write(paths.config.join("cyber.jsonc"),json!({"sandbox":{"policy":policy,"network":"off","apply_to":["mcp"]},"mcp":{"audit":{"type":"local","command":"/usr/bin/python3","args":["-u","-c",SERVER],"env":{"EXPLICIT":"reviewed","CREDENTIAL_FILE":home.join(".aws/credential"),"TMPDIR":"/unowned","HTTP_PROXY":"http://unowned.invalid:9"},"tools":{"allow":["record"],"deny":["denied"]}}}}).to_string()).unwrap();
        Self {
            _root: root,
            repo,
            home,
            temp,
            paths,
            env,
        }
    }
    fn load(&self) -> Resolved {
        config::load(&LoadRequest {
            location: &self.repo,
            paths: &self.paths,
            env: &self.env,
            home: &self.home,
            profile: None,
            overrides: &[],
            flags: json!({}),
        })
        .unwrap()
    }
    fn trust(&self) -> TrustStore {
        TrustStore::new(self.paths.trust_file())
    }
    fn configure(&self, update: impl FnOnce(&mut Value)) {
        let path = self.paths.config.join("cyber.jsonc");
        let mut value: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        update(&mut value);
        std::fs::write(path, value.to_string()).unwrap();
    }
    fn launcher<'a>(
        &'a self,
        loaded: &'a Resolved,
        trust: &'a TrustStore,
        helper: Option<&'a std::path::Path>,
    ) -> LocalLauncher<'a> {
        LocalLauncher {
            resolved: loaded,
            trust,
            location: &self.repo,
            home: &self.home,
            temp_dir: &self.temp,
            helper,
            credential_env_names: &[],
        }
    }
}
fn result(value: Value) -> Value {
    serde_json::from_str(value["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn configured_launch_enforces_environment_filters_confinement_and_cleanup() {
    let fixture = Fixture::new("workspace-write");
    let loaded = fixture.load();
    let trust = fixture.trust();
    let helper = cyber_sandbox::find_helper();
    let mut launcher = fixture.launcher(&loaded, &trust, helper.as_deref());
    let credentials = ["PATH".into()];
    launcher.credential_env_names = &credentials;
    let mut server = launcher.connect("audit", || Ok(())).await.unwrap();
    let scratch = server.scratch_path().to_path_buf();
    assert_eq!(server.tools().len(), 1);
    assert_eq!(server.tools()[0].exposed_name, "mcp__audit__record");
    assert_eq!(server.metadata()["serverInfo"]["name"], "fixture");
    assert!(
        server
            .settle_after_shutdown::<()>(|| panic!("unverified shutdown cannot commit cleanup"))
            .is_err()
    );
    assert!(
        server
            .call_tool("denied", json!({}), Duration::from_secs(3))
            .await
            .is_err()
    );
    assert!(!fixture.repo.join("called").exists());
    let value = result(
        server
            .call_tool("record", json!({}), Duration::from_secs(3))
            .await
            .unwrap(),
    );
    assert_eq!(value["explicit"], "reviewed");
    assert_eq!(value["proxy"], Value::Null);
    assert_eq!(value["path"], Value::Null);
    assert_eq!(value["tmp"], scratch.display().to_string());
    assert_eq!(value["writable"], true);
    assert_eq!(value["credential_read"], false);
    assert!(server.shutdown().await.0);
    assert!(scratch.exists());
    assert!(
        server
            .settle_after_shutdown(|| Err::<(), _>("receipt commit refused".into()))
            .is_err()
    );
    assert!(scratch.exists());
    server.settle_after_shutdown(|| Ok(())).unwrap();
    drop(server);
    assert!(!scratch.exists());
}

#[tokio::test]
async fn refresh_cannot_rebind_an_old_exposed_name_to_a_different_remote_tool() {
    let fixture = Fixture::new("full-access");
    let script = SERVER.replace("'name':'record'", "'name':'record.file'").replace("elif request['method'] == 'tools/list':", "elif request['method'] == 'tools/list':\n        if pathlib.Path('catalog.json').exists():\n            result = json.loads(pathlib.Path('catalog.json').read_text())\n            print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}),flush=True)\n            continue");
    fixture.configure(|value| {
        value["mcp"]["audit"]["args"] = json!(["-u", "-c", script]);
        value["mcp"]["audit"]["tools"]["allow"] = json!(["*"]);
    });
    let loaded = fixture.load();
    let trust = fixture.trust();
    let mut server = fixture
        .launcher(&loaded, &trust, None)
        .connect("audit", || Ok(()))
        .await
        .unwrap();
    let original = server.tools()[0].exposed_name.clone();
    std::fs::write(
        fixture.repo.join("catalog.json"),
        json!({"tools":[{"name":"record_file","inputSchema":{}}]}).to_string(),
    )
    .unwrap();
    server.refresh_tools(Duration::from_secs(1)).await.unwrap();
    let replacement = server.tools()[0].exposed_name.clone();
    assert_ne!(replacement, original);
    let stale = server
        .call_exposed_tool(&original, json!({}), Duration::from_secs(1))
        .await
        .unwrap_err();
    assert_eq!(stale.to_string(), format!("Stale tool call: {original}"));
    assert!(!fixture.repo.join("called").exists());
    server
        .call_exposed_tool(&replacement, json!({}), Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(fixture.repo.join("called")).unwrap(),
        "record_file"
    );
    std::fs::remove_file(fixture.repo.join("catalog.json")).unwrap();
    server.refresh_tools(Duration::from_secs(1)).await.unwrap();
    assert_eq!(server.tools()[0].exposed_name, original);
    assert!(
        server
            .call_exposed_tool(&replacement, json!({}), Duration::from_secs(1))
            .await
            .is_err()
    );
    assert!(server.shutdown().await.0);
    server.settle_after_shutdown(|| Ok(())).unwrap();
}

#[tokio::test]
async fn exposed_calls_route_to_original_names_and_refreshed_tools_become_stale() {
    let fixture = Fixture::new("full-access");
    let script = SERVER
        .replace("'name':'record'", "'name':'record.file'")
        .replace(
            "result = {'tools':[",
            "result = {'tools':([] if pathlib.Path('refresh-empty').exists() else [",
        )
        .replace(
            "{'name':'denied','inputSchema':{'type':'object'}}]}",
            "{'name':'denied','inputSchema':{'type':'object'}}])}",
        );
    fixture.configure(|value| {
        value["mcp"]["audit"]["args"] = json!(["-u", "-c", script]);
        value["mcp"]["audit"]["tools"]["allow"] = json!(["*"]);
    });
    let loaded = fixture.load();
    let trust = fixture.trust();
    let mut server = fixture
        .launcher(&loaded, &trust, None)
        .connect("audit", || Ok(()))
        .await
        .unwrap();
    assert_eq!(server.tools()[0].exposed_name, "mcp__audit__record_file");
    server
        .call_exposed_tool("mcp__audit__record_file", json!({}), Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(fixture.repo.join("called")).unwrap(),
        "record.file"
    );
    std::fs::remove_file(fixture.repo.join("called")).unwrap();
    std::fs::write(fixture.repo.join("refresh-empty"), "").unwrap();
    server.refresh_tools(Duration::from_secs(1)).await.unwrap();
    assert!(server.tools().is_empty());
    let stale = server
        .call_exposed_tool("mcp__audit__record_file", json!({}), Duration::from_secs(1))
        .await
        .unwrap_err();
    assert_eq!(
        stale.to_string(),
        "Stale tool call: mcp__audit__record_file"
    );
    assert!(!fixture.repo.join("called").exists());
    assert!(server.shutdown().await.0);
    server.settle_after_shutdown(|| Ok(())).unwrap();
}

#[tokio::test]
async fn initialization_and_listing_share_the_configured_startup_deadline() {
    let fixture = Fixture::new("full-access");
    let script = SERVER
        .replace(
            "result = {'protocolVersion'",
            "time.sleep(0.65)\n        result = {'protocolVersion'",
        )
        .replace(
            "elif request['method'] == 'tools/list':",
            "elif request['method'] == 'tools/list':\n        time.sleep(0.65)",
        );
    fixture.configure(|value| {
        value["mcp"]["audit"]["timeout"] = json!(1);
        value["mcp"]["audit"]["args"] = json!(["-u", "-c", script]);
    });
    let loaded = fixture.load();
    let trust = fixture.trust();
    let started = tokio::time::Instant::now();
    let error = match fixture
        .launcher(&loaded, &trust, None)
        .connect("audit", || Ok(()))
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("listing received a fresh startup deadline"),
    };
    assert!(error.acknowledged);
    assert!(error.diagnostic.contains("timed out"));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(error.scratch.unwrap().exists());
}

#[tokio::test]
async fn invalid_initial_catalog_closes_native_server_and_retains_stderr_and_scratch() {
    let fixture = Fixture::new("full-access");
    let script = SERVER
        .replace("for line in sys.stdin:", "pathlib.Path('server.pid').write_text(str(os.getpid()))\nfor line in sys.stdin:")
        .replace("elif request['method'] == 'tools/list':", "elif request['method'] == 'tools/list':\n        print('listing diagnostic',file=sys.stderr,flush=True)\n        print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'tools':'invalid'}}),flush=True)\n        continue\n    elif False:");
    fixture.configure(|value| value["mcp"]["audit"]["args"] = json!(["-u", "-c", script]));
    let loaded = fixture.load();
    let trust = fixture.trust();
    let error = match fixture
        .launcher(&loaded, &trust, None)
        .connect("audit", || Ok(()))
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("malformed listing entered ready state"),
    };
    assert!(error.acknowledged);
    assert!(error.scratch.unwrap().exists());
    assert!(
        String::from_utf8(error.stderr.bytes)
            .unwrap()
            .contains("listing diagnostic")
    );
    let pid = std::fs::read_to_string(fixture.repo.join("server.pid")).unwrap();
    let probe = std::process::Command::new("/bin/kill")
        .args(["-0", &pid])
        .output()
        .unwrap();
    assert!(
        !probe.status.success(),
        "acknowledged server must no longer exist"
    );
}

#[tokio::test]
async fn unlisted_tool_is_refused_before_peer_effects_even_when_filter_allows_it() {
    let fixture = Fixture::new("full-access");
    fixture.configure(|value| value["mcp"]["audit"]["tools"]["allow"] = json!(["*"]));
    let loaded = fixture.load();
    let trust = fixture.trust();
    let mut server = fixture
        .launcher(&loaded, &trust, None)
        .connect("audit", || Ok(()))
        .await
        .unwrap();
    assert!(
        server
            .call_tool("unlisted", json!({}), Duration::from_secs(1))
            .await
            .is_err()
    );
    assert!(!fixture.repo.join("called").exists());
    assert!(server.shutdown().await.0);
    server.settle_after_shutdown(|| Ok(())).unwrap();
}

#[tokio::test]
async fn opted_in_mcp_cannot_write_checkout_siblings_or_extra_roots() {
    let fixture = Fixture::new("workspace-write");
    let location = fixture.repo.join("nested");
    std::fs::create_dir(&location).unwrap();
    let script = SERVER.replace(
        "writable = True",
        "writable = True\n        for target in ['OUTSIDE','EXTRA']:\n            try: pathlib.Path(os.environ[target]).write_text('escaped')\n            except OSError: pass",
    );
    fixture.configure(|value| {
        value["sandbox"]["writable_roots"] = json!([fixture.home]);
        value["mcp"]["audit"]["args"] = json!(["-u", "-c", script]);
        value["mcp"]["audit"]["env"]["OUTSIDE"] = json!(fixture.repo.join("sibling"));
        value["mcp"]["audit"]["env"]["EXTRA"] = json!(fixture.home.join("extra"));
    });
    let loaded = fixture.load();
    let trust = fixture.trust();
    let helper = cyber_sandbox::find_helper();
    let mut launcher = fixture.launcher(&loaded, &trust, helper.as_deref());
    launcher.location = &location;
    let mut server = launcher.connect("audit", || Ok(())).await.unwrap();
    let value = result(
        server
            .call_tool("record", json!({}), Duration::from_secs(3))
            .await
            .unwrap(),
    );
    assert_eq!(value["writable"], true);
    assert!(!fixture.repo.join("sibling").exists());
    assert!(!fixture.home.join("extra").exists());
    assert!(location.join("called").exists());
    assert!(server.shutdown().await.0);
    server.settle_after_shutdown(|| Ok(())).unwrap();
}

#[tokio::test]
async fn opted_in_proxy_allows_owned_requests_and_blocks_direct_network() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let upstream = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let count = socket.read(&mut request).await.unwrap();
        assert!(
            std::str::from_utf8(&request[..count])
                .unwrap()
                .starts_with("GET /probe ")
        );
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .unwrap();
    });
    let fixture = Fixture::new("workspace-write");
    let script = SERVER.replace(
        "writable = True",
        &format!(
            r#"writable = True
        import socket, urllib.request
        direct = False
        try:
            connection = socket.create_connection(('127.0.0.1',{port}),timeout=1)
            connection.close()
            direct = True
        except OSError: pass
        body = urllib.request.urlopen('http://127.0.0.1:{port}/probe',timeout=3).read().decode()
        os.environ['EXPLICIT'] = json.dumps({{'direct':direct,'body':body}})"#
        ),
    );
    fixture.configure(|value| {
        value["sandbox"]["network"] = json!("proxy");
        value["sandbox"]["allowed_domains"] = json!(["127.0.0.1"]);
        value["mcp"]["audit"]["args"] = json!(["-u", "-c", script]);
        value["mcp"]["audit"]["env"]["NO_PROXY"] = json!("127.0.0.1");
    });
    let loaded = fixture.load();
    let trust = fixture.trust();
    let helper = cyber_sandbox::find_helper();
    let mut server = fixture
        .launcher(&loaded, &trust, helper.as_deref())
        .connect("audit", || Ok(()))
        .await
        .unwrap();
    let call = server
        .call_tool("record", json!({}), Duration::from_secs(5))
        .await;
    assert!(server.shutdown().await.0);
    server.settle_after_shutdown(|| Ok(())).unwrap();
    let value = result(call.unwrap());
    let network: Value = serde_json::from_str(value["explicit"].as_str().unwrap()).unwrap();
    assert_eq!(network, json!({"direct":false,"body":"ok"}));
    assert!(
        value["proxy"]
            .as_str()
            .unwrap()
            .starts_with("http://127.0.0.1:")
    );
    tokio::time::timeout(Duration::from_secs(3), upstream)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn default_sandbox_scope_leaves_mcp_unconfined() {
    let fixture = Fixture::new("read-only");
    fixture.configure(|value| {
        value["sandbox"].as_object_mut().unwrap().remove("apply_to");
    });
    let loaded = fixture.load();
    let trust = fixture.trust();
    let mut server = fixture
        .launcher(&loaded, &trust, None)
        .connect("audit", || Ok(()))
        .await
        .unwrap();
    let value = result(
        server
            .call_tool("record", json!({}), Duration::from_secs(3))
            .await
            .unwrap(),
    );
    assert_eq!(value["writable"], true);
    assert_eq!(value["credential_read"], true);
    assert!(fixture.repo.join("called").exists());
    let scratch = server.scratch_path().to_path_buf();
    assert!(server.shutdown().await.0);
    server.settle_after_shutdown(|| Ok(())).unwrap();
    drop(server);
    assert!(!scratch.exists());
}

#[tokio::test]
async fn read_only_policy_stays_read_only_and_disposal_preserves_unknown_scratch() {
    let fixture = Fixture::new("read-only");
    let loaded = fixture.load();
    let trust = fixture.trust();
    let helper = cyber_sandbox::find_helper();
    let mut server = fixture
        .launcher(&loaded, &trust, helper.as_deref())
        .connect("audit", || Ok(()))
        .await
        .unwrap();
    let value = result(
        server
            .call_tool("record", json!({}), Duration::from_secs(3))
            .await
            .unwrap(),
    );
    assert_eq!(value["writable"], false);
    assert!(!fixture.repo.join("called").exists());
    let scratch = server.scratch_path().to_path_buf();
    drop(server);
    assert!(scratch.exists());
}

#[tokio::test]
async fn project_approval_is_rechecked_after_durable_admission_before_spawn() {
    let fixture = Fixture::new("full-access");
    std::fs::write(
        fixture.repo.join("cyber.jsonc"),
        json!({"mcp":{"audit":{"env":{"EXPLICIT":"project"}}}}).to_string(),
    )
    .unwrap();
    let trust = fixture.trust();
    let untrusted = fixture.load();
    trust
        .approve(
            &untrusted.trust.checkout_root,
            untrusted.trust.digest.as_deref().unwrap(),
        )
        .unwrap();
    let loaded = fixture.load();
    let digest = McpSettings::from_config(&loaded.value).unwrap().servers["audit"]
        .digest("audit")
        .unwrap();
    let helper = cyber_sandbox::find_helper();
    let launcher = fixture.launcher(&loaded, &trust, helper.as_deref());
    assert!(
        launcher
            .connect("audit", || panic!(
                "unapproved server cannot reach admission"
            ))
            .await
            .is_err()
    );
    trust.approve_mcp(&fixture.repo, &digest).unwrap();
    let called = std::cell::Cell::new(false);
    let error = match launcher
        .connect("audit", || {
            called.set(true);
            trust.revoke_mcp(&fixture.repo, &digest).unwrap();
            Ok(())
        })
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("revoked launch accepted"),
    };
    assert!(error.acknowledged);
    assert!(called.get());
    assert!(!error.retry_safe());
    assert!(!fixture.repo.join("called").exists());
    assert!(error.scratch.unwrap().exists());
}

#[tokio::test]
async fn native_launch_failure_retains_unknown_scratch_without_credential_diagnostics() {
    let fixture = Fixture::new("full-access");
    let mut loaded = fixture.load();
    loaded.value["mcp"]["audit"]["command"] = json!("/not-installed/private-credential");
    let trust = fixture.trust();
    let error = match fixture
        .launcher(&loaded, &trust, None)
        .connect("audit", || Ok(()))
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("invalid launch accepted"),
    };
    assert!(!error.acknowledged);
    assert!(error.scratch.unwrap().exists());
    assert!(!error.diagnostic.contains("private-credential"));
}

#[tokio::test]
async fn revocation_blocks_tool_calls_on_an_already_connected_project_server() {
    let fixture = Fixture::new("workspace-write");
    std::fs::write(
        fixture.repo.join("cyber.jsonc"),
        json!({"mcp":{"audit":{"env":{"EXPLICIT":"project"}}}}).to_string(),
    )
    .unwrap();
    let trust = fixture.trust();
    let pending = fixture.load();
    trust
        .approve(
            &pending.trust.checkout_root,
            pending.trust.digest.as_deref().unwrap(),
        )
        .unwrap();
    let loaded = fixture.load();
    let digest = McpSettings::from_config(&loaded.value).unwrap().servers["audit"]
        .digest("audit")
        .unwrap();
    trust.approve_mcp(&fixture.repo, &digest).unwrap();
    let helper = cyber_sandbox::find_helper();
    let mut server = fixture
        .launcher(&loaded, &trust, helper.as_deref())
        .connect("audit", || Ok(()))
        .await
        .unwrap();
    trust.revoke_mcp(&fixture.repo, &digest).unwrap();
    assert!(
        server
            .call_tool("record", json!({}), Duration::from_secs(3))
            .await
            .is_err()
    );
    assert!(!fixture.repo.join("called").exists());
    assert!(server.shutdown().await.0);
    server.settle_after_shutdown(|| Ok(())).unwrap();
}

#[tokio::test]
async fn initialization_progress_cannot_extend_absolute_configured_startup_deadline() {
    let fixture = Fixture::new("full-access");
    fixture.configure(|loaded| {
    loaded["mcp"]["audit"]["timeout"] = json!(1);
    loaded["mcp"]["audit"]["args"] = json!([
        "-u",
        "-c",
        "import json,sys,time; r=json.loads(sys.stdin.readline());\nwhile True:\n print(json.dumps({'jsonrpc':'2.0','method':'notifications/progress','params':{'progressToken':r['id'],'progress':1}}),flush=True); time.sleep(0.02)"
    ]);
    });
    let loaded = fixture.load();
    let trust = fixture.trust();
    let started = tokio::time::Instant::now();
    let error = match fixture
        .launcher(&loaded, &trust, None)
        .connect("audit", || Ok(()))
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("endless initialization accepted"),
    };
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(error.acknowledged);
    assert!(error.scratch.unwrap().exists());
}

fn ownership_store() -> std::sync::Arc<cyber_store::Store> {
    std::sync::Arc::new(
        cyber_store::Store::open(cyber_store::StoreOptions::new(
            cyber_core::paths::DatabaseLocation::Memory,
            cyber_server::runtime::Runtime::registry(),
        ))
        .unwrap(),
    )
}

#[tokio::test]
async fn owned_launch_records_preparation_and_settles_before_releasing_scratch() {
    use cyber_server::runtime::{McpConnectionPhase, mcp_connections};
    use cyber_tools::mcp::McpLocationPin;
    let fixture = Fixture::new("full-access");
    let loaded = fixture.load();
    let trust = fixture.trust();
    let store = ownership_store();
    let launcher = fixture.launcher(&loaded, &trust, None);
    let mut server = launcher
        .connect_owned("audit", store.clone(), |id| {
            let store = &store;
            let directory = &fixture.repo;
            async move {
                let records = mcp_connections(store, directory).unwrap();
                assert_eq!(records[0].id, id);
                assert_eq!(records[0].phase, McpConnectionPhase::Preparing);
                Ok(McpLocationPin::unmanaged())
            }
        })
        .await
        .unwrap();
    let id = server.record().id.clone();
    assert!(id.starts_with("mcs_"));
    assert_eq!(server.record().phase, McpConnectionPhase::Running);
    assert!(
        launcher
            .connect_owned("audit", store.clone(), |_| async {
                panic!("duplicate admission must refuse before native preparation")
            })
            .await
            .is_err()
    );
    server
        .call_exposed_tool("mcp__audit__record", json!({}), Duration::from_secs(2))
        .await
        .unwrap();
    let scratch = server.scratch_path().to_path_buf();
    let (record, _) = server.shutdown().await.unwrap();
    assert_eq!(record.phase, McpConnectionPhase::Settled);
    assert_eq!(record.acknowledged, Some(true));
    assert_eq!(mcp_connections(&store, &fixture.repo).unwrap()[0], record);
    assert!(server.tools().is_empty());
    assert!(
        server
            .call_tool("record", json!({}), Duration::from_secs(1))
            .await
            .is_err()
    );
    let seq = store.aggregate_seq(&id).unwrap();
    server.shutdown().await.unwrap();
    assert_eq!(store.aggregate_seq(&id).unwrap(), seq);
    assert!(scratch.exists());
    drop(server);
    assert!(!scratch.exists());
    assert_eq!(
        store
            .read(|connection| connection
                .query_row("SELECT COUNT(*) FROM session", [], |row| row
                    .get::<_, i64>(0))
                .map_err(Into::into))
            .unwrap(),
        0
    );
    let mut next = launcher
        .connect_owned("audit", store.clone(), |_| async {
            Ok(McpLocationPin::unmanaged())
        })
        .await
        .unwrap();
    assert_ne!(next.record().id, id);
    next.shutdown().await.unwrap();
}

#[tokio::test]
async fn disposed_owned_launch_retains_unknown_receipt_and_scratch() {
    use cyber_server::runtime::{McpConnectionPhase, mcp_connections};
    use cyber_tools::mcp::McpLocationPin;
    let fixture = Fixture::new("full-access");
    let loaded = fixture.load();
    let trust = fixture.trust();
    let store = ownership_store();
    let launcher = fixture.launcher(&loaded, &trust, None);
    let server = launcher
        .connect_owned("audit", store.clone(), |_| async {
            Ok(McpLocationPin::unmanaged())
        })
        .await
        .unwrap();
    let scratch = server.scratch_path().to_path_buf();
    drop(server);
    let records = mcp_connections(&store, &fixture.repo).unwrap();
    assert_eq!(records[0].phase, McpConnectionPhase::Unknown);
    assert_eq!(records[0].acknowledged, Some(false));
    assert!(scratch.exists());
    assert!(
        launcher
            .connect_owned("audit", store, |_| async {
                panic!("unknown owner must fence replacement")
            })
            .await
            .is_err()
    );
}

#[tokio::test]
async fn owned_shutdown_retries_failed_durable_commit_without_reopening_calls() {
    use cyber_tools::mcp::McpLocationPin;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let fixture = Fixture::new("full-access");
    let loaded = fixture.load();
    let trust = fixture.trust();
    let reject = Arc::new(AtomicBool::new(true));
    let rejection = reject.clone();
    let mut registry = cyber_server::runtime::Runtime::registry();
    registry.projector(move |_, event| {
        if event.kind == "mcp.status.changed.1"
            && event.data["record"]["phase"] == "settled"
            && rejection.swap(false, Ordering::SeqCst)
        {
            return Err("injected terminal commit failure".into());
        }
        Ok(())
    });
    let store = Arc::new(
        cyber_store::Store::open(cyber_store::StoreOptions::new(
            cyber_core::paths::DatabaseLocation::Memory,
            registry,
        ))
        .unwrap(),
    );
    let mut server = fixture
        .launcher(&loaded, &trust, None)
        .connect_owned("audit", store.clone(), |_| async {
            Ok(McpLocationPin::unmanaged())
        })
        .await
        .unwrap();
    let scratch = server.scratch_path().to_path_buf();
    let error = server.shutdown().await.unwrap_err();
    assert!(error.acknowledged);
    assert!(!error.retry_safe());
    assert!(
        error
            .diagnostic
            .contains("injected terminal commit failure")
    );
    assert!(scratch.exists());
    assert!(server.tools().is_empty());
    assert!(server.refresh_tools(Duration::from_secs(1)).await.is_err());
    server.shutdown().await.unwrap();
    drop(server);
    assert!(!scratch.exists());
}

#[tokio::test]
async fn owned_preparation_timeout_fences_replacement_without_spawning() {
    use cyber_server::runtime::{McpConnectionPhase, mcp_connections};
    use cyber_tools::mcp::McpLocationPin;
    let fixture = Fixture::new("full-access");
    fixture.configure(|value| value["mcp"]["audit"]["timeout"] = json!(1));
    let loaded = fixture.load();
    let trust = fixture.trust();
    let store = ownership_store();
    let launcher = fixture.launcher(&loaded, &trust, None);
    let error = launcher
        .connect_owned("audit", store.clone(), |_| async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(McpLocationPin::unmanaged())
        })
        .await
        .err()
        .unwrap();
    assert!(!error.acknowledged);
    assert!(error.diagnostic.contains("preparation timed out"));
    assert!(error.scratch.is_none());
    assert!(!fixture.temp.exists());
    assert_eq!(
        mcp_connections(&store, &fixture.repo).unwrap()[0].phase,
        McpConnectionPhase::Unknown
    );
    assert!(
        launcher
            .connect_owned("audit", store, |_| async {
                panic!("timed-out preparation must fence replacement")
            })
            .await
            .is_err()
    );
}

fn malformed_owned_fixture() -> Fixture {
    let fixture = Fixture::new("full-access");
    let script = SERVER.replace(
        "elif request['method'] == 'tools/list':",
        "elif request['method'] == 'tools/list':\n        print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'tools':'invalid'}}),flush=True)\n        continue\n    elif False:",
    );
    fixture.configure(|value| value["mcp"]["audit"]["args"] = json!(["-u", "-c", script]));
    fixture
}

#[tokio::test]
async fn owned_failed_discovery_is_retry_safe_only_after_all_settlement_and_cleanup() {
    use cyber_server::runtime::{McpConnectionPhase, mcp_connections};
    use cyber_tools::mcp::McpLocationPin;
    let fixture = malformed_owned_fixture();
    let loaded = fixture.load();
    let trust = fixture.trust();
    let store = ownership_store();
    let error = match fixture
        .launcher(&loaded, &trust, None)
        .connect_owned("audit", store.clone(), |_| async {
            Ok(McpLocationPin::unmanaged())
        })
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("invalid catalog accepted"),
    };
    assert!(error.acknowledged);
    assert!(error.retry_safe());
    assert!(!error.scratch.as_ref().unwrap().exists());
    assert_eq!(
        mcp_connections(&store, &fixture.repo).unwrap()[0].phase,
        McpConnectionPhase::Settled
    );
    let next = fixture
        .launcher(&loaded, &trust, None)
        .connect_owned("audit", store.clone(), |_| async {
            Ok(McpLocationPin::unmanaged())
        })
        .await;
    assert!(match next {
        Err(error) => error.retry_safe(),
        Ok(_) => false,
    });
    assert_eq!(mcp_connections(&store, &fixture.repo).unwrap().len(), 2);
}

#[tokio::test]
async fn native_acknowledgement_without_failed_launch_commit_is_not_retry_safe() {
    use cyber_server::runtime::{McpConnectionPhase, mcp_connections};
    use cyber_tools::mcp::McpLocationPin;
    let fixture = malformed_owned_fixture();
    let loaded = fixture.load();
    let trust = fixture.trust();
    let mut registry = cyber_server::runtime::Runtime::registry();
    registry.projector(|_, event| {
        if event.kind == "mcp.status.changed.1" && event.data["record"]["phase"] == "settled" {
            return Err("injected failed startup settlement".into());
        }
        Ok(())
    });
    let store = std::sync::Arc::new(
        cyber_store::Store::open(cyber_store::StoreOptions::new(
            cyber_core::paths::DatabaseLocation::Memory,
            registry,
        ))
        .unwrap(),
    );
    let error = match fixture
        .launcher(&loaded, &trust, None)
        .connect_owned("audit", store.clone(), |_| async {
            Ok(McpLocationPin::unmanaged())
        })
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("invalid catalog accepted"),
    };
    assert!(error.acknowledged);
    assert!(!error.retry_safe());
    assert!(error.scratch.as_ref().unwrap().exists());
    assert_eq!(
        mcp_connections(&store, &fixture.repo).unwrap()[0].phase,
        McpConnectionPhase::Unknown
    );
}

#[tokio::test]
async fn rejected_connected_commit_settles_native_resources_before_retry_permission() {
    use cyber_server::runtime::{McpConnectionPhase, mcp_connections};
    use cyber_tools::mcp::McpLocationPin;
    let fixture = Fixture::new("full-access");
    let loaded = fixture.load();
    let trust = fixture.trust();
    let mut registry = cyber_server::runtime::Runtime::registry();
    registry.projector(|_, event| {
        if event.kind == "mcp.status.changed.1" && event.data["record"]["phase"] == "running" {
            return Err("injected connected commit failure".into());
        }
        Ok(())
    });
    let store = std::sync::Arc::new(
        cyber_store::Store::open(cyber_store::StoreOptions::new(
            cyber_core::paths::DatabaseLocation::Memory,
            registry,
        ))
        .unwrap(),
    );
    let error = match fixture
        .launcher(&loaded, &trust, None)
        .connect_owned("audit", store.clone(), |_| async {
            Ok(McpLocationPin::unmanaged())
        })
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("rejected connected commit accepted"),
    };
    assert!(error.acknowledged);
    assert!(error.retry_safe());
    assert!(!error.scratch.as_ref().unwrap().exists());
    assert_eq!(
        mcp_connections(&store, &fixture.repo).unwrap()[0].phase,
        McpConnectionPhase::Settled
    );
}

#[tokio::test]
async fn authorized_local_launch_advertises_roots_without_private_or_readonly_paths() {
    const ROOT_SERVER: &str = r#"
import json,sys
roots = None
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request: continue
    if request['method'] == 'initialize':
        assert request['params']['capabilities'] == {'roots':{'listChanged':False},'elicitation':{'form':{}}}
        print(json.dumps({'jsonrpc':'2.0','id':'workspace-roots','method':'roots/list'}), flush=True)
        reply = json.loads(sys.stdin.readline())
        assert reply['id'] == 'workspace-roots'
        roots = reply['result']
        result = {'protocolVersion':'2025-11-25','capabilities':{'tools':{}},'serverInfo':{'name':'roots','version':'1'}}
    elif request['method'] == 'tools/list':
        result = {'tools':[{'name':'record','inputSchema':{'type':'object'}}]}
    else:
        result = {'content':[{'type':'text','text':json.dumps(roots)}]}
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}), flush=True)
"#;
    for policy in ["workspace-write", "read-only", "full-access"] {
        let fixture = Fixture::new(policy);
        let extra = fixture.repo.join("extra # Δ");
        std::fs::create_dir(&extra).unwrap();
        let extra_file = fixture.repo.join("writable.txt");
        std::fs::write(&extra_file, "contents").unwrap();
        let cwd = fixture.repo.join("server-cwd");
        std::fs::create_dir(&cwd).unwrap();
        fixture.configure(|config| {
            config["mcp"]["audit"]["args"] = json!(["-u", "-c", ROOT_SERVER]);
            config["sandbox"]["writable_roots"] = json!([extra, extra_file, fixture.repo, extra]);
            config["mcp"]["audit"]["cwd"] = json!(cwd);
            config["sandbox"]["readable_paths"] = json!([fixture.home]);
        });
        let resolved = fixture.load();
        let trust = fixture.trust();
        let helper = cyber_sandbox::find_helper();
        let mut server = fixture
            .launcher(&resolved, &trust, helper.as_deref())
            .connect("audit", || Ok(()))
            .await
            .unwrap();
        let roots = result(
            server
                .call_tool("record", json!({}), Duration::from_secs(3))
                .await
                .unwrap(),
        );
        let roots = roots["roots"].as_array().unwrap();
        assert_eq!(roots.len(), if policy == "read-only" { 1 } else { 3 });
        assert_eq!(
            roots[0]["uri"],
            reqwest::Url::from_directory_path(fixture.repo.canonicalize().unwrap())
                .unwrap()
                .as_str()
        );
        if roots.len() == 3 {
            assert_eq!(
                roots[1]["uri"],
                reqwest::Url::from_directory_path(extra.canonicalize().unwrap())
                    .unwrap()
                    .as_str()
            );
        }
        if roots.len() == 3 {
            assert_eq!(
                roots[2]["uri"],
                reqwest::Url::from_file_path(extra_file.canonicalize().unwrap())
                    .unwrap()
                    .as_str()
            );
        }
        assert!(
            roots
                .iter()
                .all(|root| root["name"] != "home" && root["name"] != "scratch")
        );
        assert!(server.shutdown().await.0);
        server.settle_after_shutdown(|| Ok(())).unwrap();
    }
}

#[tokio::test]
async fn unavailable_writable_root_refuses_before_native_admission() {
    let fixture = Fixture::new("workspace-write");
    fixture.configure(|config| config["sandbox"]["writable_roots"] = json!(["missing-root"]));
    let resolved = fixture.load();
    let trust = fixture.trust();
    let admitted = std::cell::Cell::new(false);
    let error = match fixture
        .launcher(&resolved, &trust, None)
        .connect("audit", || {
            admitted.set(true);
            Ok(())
        })
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("missing root accepted"),
    };
    assert_eq!(error.diagnostic, "MCP protocol error: MCP root unavailable");
    assert!(error.acknowledged && error.scratch.is_none());
    assert!(!admitted.get());
}

#[tokio::test]
async fn hook_startup_cancellation_acknowledges_initialization_and_discovery_trees() {
    use tokio_util::sync::CancellationToken;
    for phase in ["initialize", "tools/list"] {
        let fixture = Fixture::new("full-access");
        let script = format!(
            r#"
import sys,json,pathlib,time,subprocess
phase={phase:?}
for line in sys.stdin:
    request=json.loads(line)
    if 'id' not in request: continue
    if request['method']==phase:
        pathlib.Path('hook-waiting').write_text('waiting')
        subprocess.Popen([sys.executable,'-c',"import time,pathlib; time.sleep(1.5); pathlib.Path('hook-escape').write_text('escaped')"])
        while True: time.sleep(.1)
    result={{'protocolVersion':'2025-11-25','capabilities':{{'tools':{{}}}},'serverInfo':{{'name':'fixture','version':'1'}}}}
    print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':result}}),flush=True)
"#
        );
        fixture.configure(|config| config["mcp"]["audit"]["args"] = json!(["-u", "-c", script]));
        let resolved = fixture.load();
        let trust = fixture.trust();
        let cancel = CancellationToken::new();
        let launcher = fixture.launcher(&resolved, &trust, None);
        let started = tokio::time::Instant::now();
        let (result, ()) = tokio::join!(
            launcher.connect_for_hook(
                "audit",
                || Ok(()),
                started + Duration::from_secs(30),
                false,
                &cancel
            ),
            async {
                tokio::time::timeout(Duration::from_secs(3), async {
                    while !fixture.repo.join("hook-waiting").exists() {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .unwrap();
                cancel.cancel();
            }
        );
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("cancelled startup connected"),
        };
        assert!(error.acknowledged);
        assert!(error.diagnostic.contains("startup cancelled"));
        assert!(started.elapsed() < Duration::from_secs(3));
        let scratch = error.scratch.unwrap();
        assert!(
            scratch.exists(),
            "caller must commit its receipt before cleanup"
        );
        tokio::time::sleep(Duration::from_millis(1600)).await;
        assert!(!fixture.repo.join("hook-escape").exists());
        std::fs::remove_dir_all(scratch).unwrap();
    }
}

#[tokio::test]
async fn hook_required_sandbox_overrides_full_access_and_ordinary_mcp_opt_out() {
    let fixture = Fixture::new("full-access");
    fixture.configure(|config| config["sandbox"]["apply_to"] = json!([]));
    let resolved = fixture.load();
    let trust = fixture.trust();
    let helper = cyber_sandbox::find_helper();
    let cancel = tokio_util::sync::CancellationToken::new();
    let mut server = fixture
        .launcher(&resolved, &trust, helper.as_deref())
        .connect_for_hook(
            "audit",
            || Ok(()),
            tokio::time::Instant::now() + Duration::from_secs(3),
            true,
            &cancel,
        )
        .await
        .unwrap();
    let observed = result(
        server
            .call_tool("record", json!({}), Duration::from_secs(2))
            .await
            .unwrap(),
    );
    assert_eq!(observed["credential_read"], false);
    assert_eq!(observed["writable"], true);
    assert!(server.shutdown().await.0);
    server.settle_after_shutdown(|| Ok(())).unwrap();
}

#[tokio::test]
async fn already_cancelled_hook_startup_refuses_before_native_admission_or_scratch() {
    let fixture = Fixture::new("full-access");
    let resolved = fixture.load();
    let trust = fixture.trust();
    let cancel = tokio_util::sync::CancellationToken::new();
    cancel.cancel();
    let admitted = std::cell::Cell::new(false);
    let result = fixture
        .launcher(&resolved, &trust, None)
        .connect_for_hook(
            "audit",
            || {
                admitted.set(true);
                Ok(())
            },
            tokio::time::Instant::now() + Duration::from_secs(30),
            false,
            &cancel,
        )
        .await;
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("cancelled startup connected"),
    };
    assert!(error.acknowledged && error.scratch.is_none());
    assert!(!admitted.get());
    assert!(!fixture.temp.exists());
}
