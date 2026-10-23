//! Actual authenticated TCP formatter discovery through application services.
use cyber_app::{App, AppOptions, registered_lsp_status};
use cyber_core::paths::{DatabaseLocation, Paths};
use serde_json::{Value, json};
use std::path::PathBuf;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    app: App,
    url: String,
    client: reqwest::Client,
    stop: tokio::sync::oneshot::Sender<()>,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let paths = Paths {
            data: root.join("data"),
            config: root.join("config"),
            state: root.join("state"),
            cache: root.join("cache"),
            tmp: root.join("tmp"),
        };
        paths.ensure().unwrap();
        std::fs::create_dir_all(paths.cache.join("bin")).unwrap();
        let candidate = paths.cache.join("bin/prettier");
        std::fs::write(&candidate, "Do not execute discovery candidates").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let app = App::build(AppOptions {
            paths,
            home: root.join("home"),
            database: DatabaseLocation::Memory,
            default_directory: root.clone(),
            sandbox_policy: None,
            snapshots: false,
            interactive: true,
            password: Some("status-password".into()),
            mcp_credentials: None,
            remote_skill_fetcher: None,
            workflow_cores: None,
            isolation: None,
        })
        .await
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/api/v1/formatters",
            listener.local_addr().unwrap()
        );
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let router = cyber_server::http::router(app.state.clone());
        let server = tokio::spawn(async move {
            cyber_server::http::serve_tcp(router, listener, async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
        });
        Self {
            _temp: temp,
            root,
            app,
            url,
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            stop,
            server,
        }
    }
    fn get(&self) -> reqwest::RequestBuilder {
        self.client
            .get(&self.url)
            .basic_auth("cyber", Some("status-password"))
    }
    fn settings(&self, value: Value) {
        std::fs::write(self.root.join("config/cyber.jsonc"), value.to_string()).unwrap();
    }
    async fn close(self) {
        self.stop.send(()).unwrap();
        self.server.await.unwrap();
        self.app.runtime.shutdown().await;
    }
}
fn row<'a>(body: &'a Value, id: &str) -> &'a Value {
    body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .unwrap()
}

#[tokio::test]
async fn lsp_status_authenticates_observes_actual_roots_and_does_not_start_servers() {
    let f = Fixture::new().await;
    let url = f.url.replace("/formatters", "/lsp");
    assert_eq!(
        f.client.get(&url).send().await.unwrap().status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    assert!(
        registered_lsp_status(&f.app.paths, &f.root)
            .await
            .unwrap()
            .is_none()
    );
    std::fs::write(f.app.paths.state.join("password"), "status-password").unwrap();
    std::fs::write(
        cyber_app::registration_path(&f.app.paths),
        json!({"id":"srv_status","version":"test","url":f.url.replace("/api/v1/formatters", ""),"socket":null,"pid":std::process::id()}).to_string(),
    ).unwrap();
    assert!(
        registered_lsp_status(&f.app.paths, &f.root)
            .await
            .unwrap()
            .unwrap()
            .is_empty()
    );
    let before: Value = f
        .client
        .get(&url)
        .basic_auth("cyber", Some("status-password"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(before["data"], json!([]));
    assert!(f.app.host.lsp_status(&f.root).unwrap().is_empty());
    let file = f.root.join("file.fixture-lsp");
    std::fs::write(&file, "snapshot").unwrap();
    let candidate = f.root.join("cache/bin/fixture-server.exe");
    std::fs::write(
        &candidate,
        "Invalid executable: must fail without running user code",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    f.settings(json!({"lsp":{"fixture":{"command":[candidate],"extensions":[".fixture-lsp"],"env":{"SECRET":"private-lsp-environment"}}},"sandbox":{"network":"off"}}));
    f.app.host.warm_lsp(&f.root, file, "snapshot".into());
    // The broken state follows an admission scan, sandbox preparation and a failed spawn
    // in the background; the limit only guards a hang. On a loaded Windows runner that
    // chain has taken more than the 3s this once allowed.
    let after = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let body: Value = f
                .client
                .get(&url)
                .basic_auth("cyber", Some("status-password"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if body["data"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"] == "fixture" && row["status"] == "broken")
            {
                break body;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(row(&after, "fixture")["root"], f.root.display().to_string());
    assert!(!after.to_string().contains("private-lsp-environment"));
    assert!(!after.to_string().contains("fixture-server.exe"));
    let live = registered_lsp_status(&f.app.paths, &f.root)
        .await
        .unwrap()
        .unwrap();
    assert!(live.iter().any(|entry| entry.id == "fixture"
        && entry.root == f.root
        && matches!(entry.status, cyber_server::http::LspState::Broken)));
    let second = f.root.join("second");
    std::fs::create_dir(&second).unwrap();
    let foreign: Value = f
        .client
        .get(&url)
        .basic_auth("cyber", Some("status-password"))
        .query(&[("location[directory]", second.to_str().unwrap())])
        .header("x-cyber-directory", f.root.to_str().unwrap())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(foreign["data"], json!([]));
    assert!(
        registered_lsp_status(&f.app.paths, &second)
            .await
            .unwrap()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        foreign["location"]["directory"],
        second.display().to_string()
    );
    let invalid = f
        .client
        .get(&url)
        .basic_auth("cyber", Some("status-password"))
        .header(
            "x-cyber-directory",
            f.root.join("missing").to_str().unwrap(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), reqwest::StatusCode::BAD_REQUEST);
    f.close().await;
}

#[tokio::test]
async fn formatter_status_authenticates_and_returns_location_without_execution() {
    let f = Fixture::new().await;
    assert_eq!(
        f.client.get(&f.url).send().await.unwrap().status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.client
            .get(&f.url)
            .basic_auth("cyber", Some("wrong"))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    std::fs::write(
        f.root.join("prettier.config.js"),
        "throw new Error('do not evaluate');",
    )
    .unwrap();
    let response = f.get().send().await.unwrap();
    assert!(response.status().is_success());
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["location"]["directory"], f.root.display().to_string());
    assert_eq!(body["data"].as_array().unwrap().len(), 12);
    assert_eq!(row(&body, "prettier")["enabled"], true);
    assert_eq!(row(&body, "prettier")["detected_by"], "prettier.config.js");
    assert!(
        body["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r.get("command").is_none() && r.get("env").is_none())
    );
    f.close().await;
}

#[tokio::test]
async fn formatter_status_query_wins_and_project_markers_are_location_scoped() {
    let f = Fixture::new().await;
    let first = f.root.join("first");
    let second = f.root.join("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    std::fs::write(first.join(".prettierrc"), "{}").unwrap();
    let body: Value = f
        .get()
        .header("x-cyber-directory", second.to_str().unwrap())
        .query(&[("location[directory]", first.to_str().unwrap())])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["location"]["directory"], first.display().to_string());
    assert_eq!(row(&body, "prettier")["enabled"], true);
    let body: Value = f
        .get()
        .query(&[("location[directory]", second.to_str().unwrap())])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["location"]["directory"], second.display().to_string());
    assert_eq!(row(&body, "prettier")["enabled"], false);
    let response = f
        .get()
        .header(
            "x-cyber-directory",
            f.root.join("missing").to_str().unwrap(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let response = f
        .get()
        .query(&[(
            "location[directory]",
            first.join(".prettierrc").to_str().unwrap(),
        )])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    f.close().await;
}

#[tokio::test]
async fn formatter_status_reloads_disabled_and_forced_settings_without_secret_fields() {
    let f = Fixture::new().await;
    f.settings(json!({"formatters":{"taplo":{"command":["uninstalled-status-taplo","fmt","$FILE"],"extensions":[".toml"],"env":{"SECRET":"integration-private-value"}}}}));
    let body: Value = f.get().send().await.unwrap().json().await.unwrap();
    assert_eq!(row(&body, "taplo")["enabled"], true);
    assert_eq!(row(&body, "taplo")["installed"], false);
    assert_eq!(row(&body, "taplo")["detected_by"], "config");
    assert!(!body.to_string().contains("integration-private-value"));
    assert!(!body.to_string().contains("uninstalled-status-taplo"));
    f.settings(json!({"formatters":false}));
    let body: Value = f.get().send().await.unwrap().json().await.unwrap();
    assert_eq!(body["data"].as_array().unwrap().len(), 12);
    assert!(
        body["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["enabled"] == false)
    );
    f.close().await;
}

#[tokio::test]
async fn formatter_status_withholds_project_commands_and_returns_safe_configuration_errors() {
    let f = Fixture::new().await;
    std::fs::create_dir(f.root.join(".git")).unwrap();
    std::fs::write(
        f.root.join("cyber.jsonc"),
        r#"{"formatters":{"untrusted":{"command":["do-not-run"],"extensions":[".x"]}}}"#,
    )
    .unwrap();
    let body: Value = f.get().send().await.unwrap().json().await.unwrap();
    assert!(
        body["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["id"] != "untrusted")
    );
    f.settings(json!({"formatters":{"invalid":{"command":"integration-private-value"}}}));
    let response = f.get().send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["_tag"], "InvalidRequestError");
    assert!(!body.to_string().contains("integration-private-value"));
    f.close().await;
}

#[tokio::test]
async fn lsp_lifecycle_authenticates_routes_locations_and_reloads_without_startup() {
    let f = Fixture::new().await;
    let base = f.url.replace("/formatters", "/lsp");
    let sibling = f.root.join("sibling");
    std::fs::create_dir(&sibling).unwrap();
    for action in ["close", "reload"] {
        let url = format!("{base}/{action}");
        assert_eq!(
            f.client.post(&url).send().await.unwrap().status(),
            reqwest::StatusCode::UNAUTHORIZED
        );
        let invalid = f
            .client
            .post(&url)
            .basic_auth("cyber", Some("status-password"))
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(invalid.status(), reqwest::StatusCode::BAD_REQUEST);
        let response = f
            .client
            .post(&url)
            .basic_auth("cyber", Some("status-password"))
            .header("x-cyber-directory", f.root.to_str().unwrap())
            .query(&[("location[directory]", sibling.to_str().unwrap())])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["location"]["directory"], sibling.display().to_string());
        assert_eq!(
            body["data"],
            json!({"closed":true,"reloaded":action=="reload"})
        );
        assert!(f.app.host.lsp_status(&sibling).unwrap().is_empty());
        assert!(f.app.host.lsp_status(&f.root).unwrap().is_empty());
        let invalid = f
            .client
            .post(&url)
            .basic_auth("cyber", Some("status-password"))
            .query(&[(
                "location[directory]",
                f.root.join("missing").to_str().unwrap(),
            )])
            .send()
            .await
            .unwrap();
        assert_eq!(invalid.status(), reqwest::StatusCode::BAD_REQUEST);
    }
    f.close().await;
}

/// A minimal framed server that records its pid and the lifecycle requests it receives.
#[cfg(unix)]
const LIFECYCLE_SERVER: &str = r#"
import json,os,sys
log=open(sys.argv[1],'a')
def record(text):
    log.write(text+'\n'); log.flush()
def send(value):
    body=json.dumps(value).encode()
    sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n'%len(body)).encode()+body)
    sys.stdout.buffer.flush()
record('pid %d'%os.getpid())
# Held until the process exits: the test checks liveness through it, because a
# sandboxed server's own pid is from another PID namespace (bubblewrap on Linux).
import fcntl
life=open(sys.argv[1]+'.alive','w'); fcntl.flock(life,fcntl.LOCK_EX); record('locked')
while True:
    length=None
    while True:
        line=sys.stdin.buffer.readline()
        if not line: record('eof'); sys.exit(0)
        if line==b'\r\n': break
        key,value=line.decode().split(':',1)
        if key.lower()=='content-length': length=int(value)
    msg=json.loads(sys.stdin.buffer.read(length)); method=msg.get('method')
    if method=='initialize':
        send({'jsonrpc':'2.0','id':msg['id'],'result':{'capabilities':{'textDocumentSync':1}}})
    elif method=='shutdown':
        record('shutdown'); send({'jsonrpc':'2.0','id':msg['id'],'result':None})
    elif method=='exit':
        record('exit'); sys.exit(0)
    elif method=='textDocument/didOpen':
        record('open '+msg['params']['textDocument']['uri'])
"#;

#[cfg(unix)]
#[tokio::test]
async fn lsp_status_reports_connected_root_and_close_exits_the_process() {
    let f = Fixture::new().await;
    let url = f.url.replace("/formatters", "/lsp");
    let events = f.root.join("server-events");
    f.settings(json!({
        "lsp":{"fixture":{"command":["python3","-u","-c",LIFECYCLE_SERVER,events],"extensions":[".fixture-lsp"]}},
        "sandbox":{"network":"off"}
    }));
    let file = f.root.join("main.fixture-lsp");
    std::fs::write(&file, "snapshot").unwrap();
    f.app.host.warm_lsp(&f.root, file, "snapshot".into());
    let status = |client: reqwest::Client, url: String| async move {
        client
            .get(&url)
            .basic_auth("cyber", Some("status-password"))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    };
    let connected = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let body = status(f.client.clone(), url.clone()).await;
            if body["data"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"] == "fixture" && row["status"] == "connected")
            {
                break body;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the warmed server connects");
    assert_eq!(
        PathBuf::from(row(&connected, "fixture")["root"].as_str().unwrap()),
        f.root
    );
    assert_eq!(
        PathBuf::from(connected["location"]["directory"].as_str().unwrap()),
        f.root
    );
    let lock = events.with_extension("alive");
    // The server holds an exclusive lock on this file for as long as it runs; its pid
    // would be meaningless here when the sandbox gives it its own PID namespace.
    let alive = || {
        std::fs::read_to_string(&events)
            .unwrap()
            .lines()
            .any(|line| line == "locked")
            && std::fs::File::open(&lock).unwrap().try_lock().is_err()
    };
    assert!(alive());

    let closed: Value = f
        .client
        .post(format!("{url}/close"))
        .basic_auth("cyber", Some("status-password"))
        .header("x-cyber-directory", f.root.to_str().unwrap())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(closed["data"], json!({"closed":true,"reloaded":false}));
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while alive() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("closing the Location ends the server process");
    let log = std::fs::read_to_string(&events).unwrap();
    let shutdown = log.find("shutdown").expect("graceful shutdown request");
    let exit = log.find("\nexit").expect("exit notification");
    assert!(shutdown < exit, "{log}");
    // The settled generation stays observable until reload, and is no longer connected.
    let after = status(f.client.clone(), url.clone()).await;
    assert_eq!(row(&after, "fixture")["status"], "broken");
    f.close().await;
}
