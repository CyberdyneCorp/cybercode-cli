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
    assert!(!fixture.repo.join("called").exists());
    assert_eq!(std::fs::read_dir(&fixture.temp).unwrap().count(), 0);
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
