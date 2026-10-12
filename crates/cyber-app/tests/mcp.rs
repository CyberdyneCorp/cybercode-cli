//! Public MCP close uses the application's actual shared native owner.
#[path = "../../cyber-tools/tests/support/mcp_http.rs"]
mod mcp_http;

use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use cyber_server::runtime::ToolHost as _;
use cyber_server::runtime::{McpConnectionOwner, McpConnectionPhase, mcp_connections};
use serde_json::json;
use std::path::Path;

async fn application(root: &Path) -> App {
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
        interactive: true,
        password: Some("mcp-test-password".into()),
    })
    .await
    .unwrap()
}

async fn listener(
    app: &App,
) -> (
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api/v1/mcp/close", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let router = cyber_server::http::router(app.state.clone());
    let server = tokio::spawn(async move {
        cyber_server::http::serve_tcp(router, listener, async {
            let _ = stopped.await;
        })
        .await
        .unwrap();
    });
    (url, stop, server)
}

#[tokio::test]
async fn public_close_auth_location_precedence_and_unknown_conflict() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let sibling = directory.join("sibling");
    std::fs::create_dir(&sibling).unwrap();
    let app = application(&directory).await;
    let mut owner = McpConnectionOwner::admit(
        app.store.clone(),
        &directory,
        &directory,
        "unknown",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .unwrap();
    owner.preparing().unwrap();
    drop(owner);
    let records = mcp_connections(&app.store, &directory).unwrap();
    assert_eq!(records[0].phase, McpConnectionPhase::Unknown);
    let (url, stop, server) = listener(&app).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    assert_eq!(
        client.post(&url).send().await.unwrap().status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client
            .post(&url)
            .basic_auth("cyber", Some("wrong"))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let invalid = client
        .post(&url)
        .basic_auth("cyber", Some("mcp-test-password"))
        .header(
            "x-cyber-directory",
            directory.join("missing").to_str().unwrap(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), reqwest::StatusCode::BAD_REQUEST);
    let body = client
        .post(&url)
        .basic_auth("cyber", Some("mcp-test-password"))
        .json(&json!({"force":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(body.status(), reqwest::StatusCode::BAD_REQUEST);
    let response = client
        .post(&url)
        .basic_auth("cyber", Some("mcp-test-password"))
        .query(&[("location[directory]", sibling.to_str().unwrap())])
        .header("x-cyber-directory", directory.to_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let value: serde_json::Value = response.json().await.unwrap();
    assert_eq!(value["data"], json!({"closed":true}));
    assert_eq!(value["location"]["directory"], sibling.to_str().unwrap());
    let first = client
        .post(&url)
        .basic_auth("cyber", Some("mcp-test-password"))
        .header("idempotency-key", "location-close-routing")
        .header("x-cyber-directory", sibling.to_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), reqwest::StatusCode::OK);
    let different = client
        .post(&url)
        .basic_auth("cyber", Some("mcp-test-password"))
        .header("idempotency-key", "location-close-routing")
        .send()
        .await
        .unwrap();
    assert_eq!(different.status(), reqwest::StatusCode::CONFLICT);
    let conflict = client
        .post(&url)
        .basic_auth("cyber", Some("mcp-test-password"))
        .send()
        .await
        .unwrap();
    assert_eq!(conflict.status(), reqwest::StatusCode::CONFLICT);
    let error: serde_json::Value = conflict.json().await.unwrap();
    assert_eq!(error["_tag"], "ConflictError");
    assert_eq!(mcp_connections(&app.store, &directory).unwrap(), records);
    stop.send(()).unwrap();
    server.await.unwrap();
    app.runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn authorized_public_close_settles_actual_native_mcp_process() {
    use cyber_server::runtime::CreateSession;
    use std::time::Duration;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let app = application(&directory).await;
    let mut updates = app.runtime.subscribe();
    let script = r#"
import json,sys
for line in sys.stdin:
    req=json.loads(line)
    if 'id' not in req: continue
    if req['method']=='initialize': result={'protocolVersion':'2025-11-25','capabilities':{'tools':{}},'serverInfo':{'name':'native','version':'1'}}
    else: result={'tools':[]}
    print(json.dumps({'jsonrpc':'2.0','id':req['id'],'result':result}),flush=True)
"#;
    std::fs::write(app.paths.config.join("cyber.json"), json!({
        "sandbox":{"policy":"full-access"},
        "providers":{"test":{"api":{"type":"openai-compatible","url":"http://127.0.0.1:9/v1","settings":{"auth":"none"}},"models":{"main":{}}}},
        "mcp":{"native":{"type":"local","command":"/usr/bin/python3","args":["-u","-c",script]}}
    }).to_string()).unwrap();
    app.runtime
        .create_session(CreateSession {
            directory: directory.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if mcp_connections(&app.store, &directory)
                .unwrap()
                .iter()
                .any(|record| record.phase == McpConnectionPhase::Running)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let (url, stop, server) = listener(&app).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    assert_eq!(
        client.post(&url).send().await.unwrap().status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        mcp_connections(&app.store, &directory).unwrap()[0].phase,
        McpConnectionPhase::Running
    );
    for attempt in 0..2 {
        let response = client
            .post(&url)
            .basic_auth("cyber", Some("mcp-test-password"))
            .header("idempotency-key", "native-close")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        if attempt == 1 {
            assert_eq!(response.headers()["idempotent-replayed"], "true");
        }
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["data"]["closed"], true);
    }
    let records = mcp_connections(&app.store, &directory).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].phase, McpConnectionPhase::Settled);
    assert_eq!(records[0].acknowledged, Some(true));
    let mut phases = Vec::new();
    while let Ok(event) = updates.try_recv() {
        if let cyber_server::runtime::LiveEvent::McpStatusChanged { update, seq } = event {
            assert_eq!(update.connection_id, records[0].id);
            assert_eq!(seq, phases.len() as i64);
            phases.push(update.phase);
        }
    }
    assert_eq!(
        phases,
        vec![
            McpConnectionPhase::Admitted,
            McpConnectionPhase::Preparing,
            McpConnectionPhase::Launching,
            McpConnectionPhase::Running,
            McpConnectionPhase::Settled
        ]
    );
    app.runtime
        .create_session(CreateSession {
            directory: directory.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if mcp_connections(&app.store, &directory)
                .unwrap()
                .iter()
                .any(|record| record.phase == McpConnectionPhase::Running)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let replay = client
        .post(&url)
        .basic_auth("cyber", Some("mcp-test-password"))
        .header("idempotency-key", "native-close")
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), reqwest::StatusCode::OK);
    assert_eq!(replay.headers()["idempotent-replayed"], "true");
    assert!(
        mcp_connections(&app.store, &directory)
            .unwrap()
            .iter()
            .any(|record| record.phase == McpConnectionPhase::Running)
    );
    assert_eq!(
        client
            .post(&url)
            .basic_auth("cyber", Some("mcp-test-password"))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK
    );
    let records = mcp_connections(&app.store, &directory).unwrap();
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .all(|record| record.phase == McpConnectionPhase::Settled
                && record.acknowledged == Some(true))
    );
    stop.send(()).unwrap();
    server.await.unwrap();
    app.runtime.shutdown().await;
}

#[tokio::test]
async fn independent_mcp_events_are_scoped_committed_and_redacted() {
    use futures::StreamExt;
    use std::time::Duration;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let sibling = directory.join("sibling");
    std::fs::create_dir(&sibling).unwrap();
    let app = application(&directory).await;
    let (close_url, stop, server) = listener(&app).await;
    let url = close_url.replace("/mcp/close", "/event");
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let mut streams = Vec::new();
    for location in [&directory, &sibling] {
        let response = client
            .get(&url)
            .basic_auth("cyber", Some("mcp-test-password"))
            .query(&[("location[directory]", location.to_str().unwrap())])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let mut stream = response.bytes_stream();
        tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        streams.push(stream);
    }
    let digest = format!("sha256:{}", "a".repeat(64));
    let mut ids = Vec::new();
    for location in [&directory, &sibling] {
        let mut owner = McpConnectionOwner::admit_observed(
            app.store.clone(),
            location,
            location,
            "observed",
            &digest,
            Some(app.runtime.mcp_status_observer()),
        )
        .unwrap();
        ids.push(owner.record().id.clone());
        owner.preparing().unwrap();
        owner
            .finish(false, "PRIVATE credential and command".into())
            .unwrap();
    }
    for (index, stream) in streams.iter_mut().enumerate() {
        let mut text = String::new();
        let frames = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                text.push_str(std::str::from_utf8(&stream.next().await.unwrap().unwrap()).unwrap());
                let frames: Vec<serde_json::Value> = text
                    .lines()
                    .filter_map(|line| line.strip_prefix("data: "))
                    .filter_map(|line| serde_json::from_str(line).ok())
                    .collect();
                if frames.len() >= 3 {
                    break frames;
                }
            }
        })
        .await
        .unwrap();
        assert!(!text.contains("PRIVATE"));
        assert!(!text.contains("owner_key"));
        for (seq, frame) in frames.iter().enumerate() {
            assert_eq!(frame["type"], "mcp.status.changed.1");
            assert_eq!(frame["durable"]["aggregateID"], ids[index]);
            assert_eq!(frame["durable"]["seq"], seq);
            assert_eq!(frame["data"]["connection_id"], ids[index]);
            assert_eq!(
                frame["location"],
                [&directory, &sibling][index].to_str().unwrap()
            );
            assert!(frame["data"].get("session_id").is_none());
        }
        assert_eq!(frames[2]["data"]["phase"], "unknown");
        assert_eq!(frames[2]["data"]["acknowledged"], false);
    }
    let sessions: i64 = app
        .store
        .read(|db| Ok(db.query_row("SELECT count(*) FROM session", [], |row| row.get(0))?))
        .unwrap();
    assert_eq!(sessions, 0);
    drop(streams);
    stop.send(()).unwrap();
    server.await.unwrap();
    app.runtime.shutdown().await;
}

#[tokio::test]
async fn configured_status_is_read_only_redacted_and_preserves_unknown_ownership() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let sibling = directory.join("sibling");
    std::fs::create_dir(&sibling).unwrap();
    let app = application(&directory).await;
    let config = app.paths.config.join("cyber.json");
    std::fs::write(&config, json!({"mcp":{
        "disabled":{"type":"local","command":"PRIVATE-command","enabled":false,"env":{"TOKEN":"PRIVATE-token"}},
        "waiting":{"type":"local","command":"PRIVATE-command"},
        "lost":{"type":"local","command":"PRIVATE-command"},
        "foreign":{"type":"local","command":"PRIVATE-command"},
        "remote":{"type":"remote","url":"http://127.0.0.1:9/mcp","headers":{"Authorization":"PRIVATE-token"}}
    }}).to_string()).unwrap();
    let digest = format!("sha256:{}", "a".repeat(64));
    for name in ["lost", "removed"] {
        let mut owner =
            McpConnectionOwner::admit(app.store.clone(), &directory, &directory, name, &digest)
                .unwrap();
        owner.preparing().unwrap();
        drop(owner);
    }
    let mut foreign = McpConnectionOwner::admit(
        app.store.clone(),
        &directory,
        &directory,
        "foreign",
        &digest,
    )
    .unwrap();
    foreign.preparing().unwrap();
    foreign.launching(Vec::new()).unwrap();
    foreign.connected().unwrap();
    let before: i64 = app
        .store
        .read(|db| Ok(db.query_row("SELECT count(*) FROM event", [], |row| row.get(0))?))
        .unwrap();
    let (close_url, stop, server) = listener(&app).await;
    let url = close_url.replace("/mcp/close", "/mcp");
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    assert_eq!(
        client.get(&url).send().await.unwrap().status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let response = client
        .get(&url)
        .basic_auth("cyber", Some("mcp-test-password"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let text = response.text().await.unwrap();
    assert!(!text.contains("PRIVATE"));
    let body: serde_json::Value = serde_json::from_str(&text).unwrap();
    let servers = body["data"].as_array().unwrap();
    assert_eq!(servers.len(), 6);
    let find = |name: &str| {
        servers
            .iter()
            .find(|server| server["name"] == name)
            .unwrap()
    };
    assert_eq!(find("disabled")["status"], "disabled");
    assert_eq!(find("waiting")["status"], "failed");
    assert!(find("waiting")["connection"].is_null());
    assert_eq!(
        find("remote")["error"],
        "MCP server has not started for this Location"
    );
    assert_eq!(find("lost")["status"], "failed");
    assert_eq!(find("foreign")["status"], "failed");
    assert_eq!(find("foreign")["connection"]["phase"], "running");
    assert!(
        find("foreign")["error"]
            .as_str()
            .unwrap()
            .contains("no retained runtime actor")
    );
    assert_eq!(find("lost")["connection"]["phase"], "unknown");
    assert_eq!(find("removed")["configured"], false);
    assert!(
        servers
            .iter()
            .all(|server| server["tools"].as_array().unwrap().is_empty())
    );
    let other: serde_json::Value = client
        .get(&url)
        .basic_auth("cyber", Some("mcp-test-password"))
        .query(&[("location[directory]", sibling.to_str().unwrap())])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(other["data"].as_array().unwrap().len(), 5);
    assert!(
        other["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|server| server["connection"].is_null())
    );
    std::fs::write(&config, "{ invalid PRIVATE-config }").unwrap();
    let invalid = client
        .get(&url)
        .basic_auth("cyber", Some("mcp-test-password"))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(!invalid.text().await.unwrap().contains("PRIVATE"));
    let after: i64 = app
        .store
        .read(|db| Ok(db.query_row("SELECT count(*) FROM event", [], |row| row.get(0))?))
        .unwrap();
    assert_eq!(before, after);
    let sessions: i64 = app
        .store
        .read(|db| Ok(db.query_row("SELECT count(*) FROM session", [], |row| row.get(0))?))
        .unwrap();
    assert_eq!(sessions, 0);
    drop(foreign);
    stop.send(()).unwrap();
    server.await.unwrap();
    app.runtime.shutdown().await;
}

async fn wait_running(app: &App, directory: &Path) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if mcp_connections(&app.store, directory)
                .unwrap()
                .iter()
                .any(|record| record.phase == McpConnectionPhase::Running)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("remote MCP server never reached running");
}

async fn open_session(app: &App, directory: &Path) {
    app.runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: directory.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap();
}

fn provider() -> serde_json::Value {
    json!({"test":{"api":{"type":"openai-compatible","url":"http://127.0.0.1:9/v1","settings":{"auth":"none"}},"models":{"main":{}}}})
}

#[tokio::test]
async fn remote_streamable_http_server_runs_through_app_host() {
    let http = mcp_http::McpHttp::start(mcp_http::Behavior::default()).await;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let app = application(&directory).await;
    let mut updates = app.runtime.subscribe();
    std::fs::write(
        app.paths.config.join("cyber.json"),
        json!({
            "providers":provider(),
            "mcp":{"remote":{"type":"remote","url":http.url,"headers":{"authorization":"Bearer PRIVATE-HEADER"}}}
        })
        .to_string(),
    )
    .unwrap();
    open_session(&app, &directory).await;
    wait_running(&app, &directory).await;
    let (close_url, stop, server) = listener(&app).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let status = client
        .get(close_url.replace("/mcp/close", "/mcp"))
        .basic_auth("cyber", Some("mcp-test-password"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!status.contains("PRIVATE"), "{status}");
    let status: serde_json::Value = serde_json::from_str(&status).unwrap();
    let remote = &status["data"][0];
    assert_eq!(remote["name"], "remote");
    assert_eq!(remote["status"], "connected", "{remote}");
    assert_eq!(remote["connection"]["phase"], "running");
    assert_eq!(remote["tools"], json!(["mcp__remote__echo"]));
    let response = client
        .post(&close_url)
        .basic_auth("cyber", Some("mcp-test-password"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let deletes = http.deletes();
    assert_eq!(deletes.len(), 1);
    assert_eq!(deletes[0].header("mcp-session-id"), Some(mcp_http::SESSION));
    let records = mcp_connections(&app.store, &directory).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].phase, McpConnectionPhase::Settled);
    assert_eq!(records[0].acknowledged, Some(true));
    let mut phases = Vec::new();
    while let Ok(event) = updates.try_recv() {
        if let cyber_server::runtime::LiveEvent::McpStatusChanged { update, .. } = event {
            phases.push(update.phase);
        }
    }
    assert_eq!(
        phases,
        vec![
            McpConnectionPhase::Admitted,
            McpConnectionPhase::Preparing,
            McpConnectionPhase::Launching,
            McpConnectionPhase::Running,
            McpConnectionPhase::Settled
        ]
    );
    stop.send(()).unwrap();
    server.await.unwrap();
    app.runtime.shutdown().await;
}

#[tokio::test]
async fn project_remote_server_is_not_contacted_before_digest_approval() {
    let http = mcp_http::McpHttp::start(mcp_http::Behavior::default()).await;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let app = application(&directory).await;
    std::fs::write(
        app.paths.config.join("cyber.json"),
        json!({"providers":provider()}).to_string(),
    )
    .unwrap();
    std::fs::write(
        directory.join("cyber.jsonc"),
        json!({"mcp":{"remote":{"type":"remote","url":http.url}}}).to_string(),
    )
    .unwrap();
    let loader = cyber_app::hook_config_loader(app.paths.clone(), directory.join("home"));
    let trust = cyber_core::trust::TrustStore::new(app.paths.trust_file());
    let idle = || tokio::time::sleep(std::time::Duration::from_millis(300));
    open_session(&app, &directory).await;
    idle().await;
    assert!(http.requests().is_empty());
    let resolved = loader(&directory).unwrap();
    trust
        .approve(
            &resolved.trust.checkout_root,
            resolved.trust.digest.as_deref().unwrap(),
        )
        .unwrap();
    open_session(&app, &directory).await;
    idle().await;
    assert!(http.requests().is_empty());
    assert!(mcp_connections(&app.store, &directory).unwrap().is_empty());
    let resolved = loader(&directory).unwrap();
    let selection = cyber_tools::mcp::inspect_server(&resolved, &directory, "remote").unwrap();
    assert!(selection.requires_approval);
    trust
        .approve_mcp(&resolved.trust.checkout_root, &selection.digest)
        .unwrap();
    open_session(&app, &directory).await;
    wait_running(&app, &directory).await;
    assert!(!http.posts("initialize").is_empty());
    app.host.close_mcp_location(&directory).await.unwrap();
    assert_eq!(http.deletes().len(), 1);
    app.runtime.shutdown().await;
}

async fn wait_record(
    app: &App,
    directory: &Path,
    ready: impl Fn(&cyber_server::runtime::McpConnectionRecord) -> bool,
) -> cyber_server::runtime::McpConnectionRecord {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(record) = mcp_connections(&app.store, directory)
                .unwrap()
                .into_iter()
                .find(|record| ready(record))
            {
                return record;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("remote MCP server never reached the expected record")
}

async fn mcp_status_body(app: &App) -> String {
    let (close_url, stop, server) = listener(app).await;
    let body = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(close_url.replace("/mcp/close", "/mcp"))
        .basic_auth("cyber", Some("mcp-test-password"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    stop.send(()).unwrap();
    server.await.unwrap();
    body
}

#[tokio::test]
async fn remote_unauthorized_server_reports_needs_auth() {
    use cyber_server::runtime::{Admission, Delivery, LiveEvent, McpConnectionStatus};
    let http = mcp_http::McpHttp::start(mcp_http::Behavior {
        status: Some(401),
        legacy: true,
        ..mcp_http::Behavior::default()
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let app = application(&directory).await;
    let mut updates = app.runtime.subscribe();
    std::fs::write(
        app.paths.config.join("cyber.json"),
        json!({
            "providers":provider(),
            "mcp":{"remote":{"type":"remote","url":http.url,"required":true,"headers":{"authorization":"Bearer PRIVATE-HEADER"}}}
        })
        .to_string(),
    )
    .unwrap();
    let session = app
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: directory.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let record = wait_record(&app, &directory, |record| {
        record.phase == McpConnectionPhase::Settled
    })
    .await;
    assert_eq!(record.status, McpConnectionStatus::NeedsAuth);
    assert_eq!(record.acknowledged, Some(true));

    let status = mcp_status_body(&app).await;
    assert!(!status.contains("PRIVATE"), "{status}");
    let status: serde_json::Value = serde_json::from_str(&status).unwrap();
    let remote = &status["data"][0];
    assert_eq!(remote["name"], "remote");
    assert_eq!(remote["status"], "needs_auth", "{remote}");
    assert_eq!(
        remote["error"],
        "MCP server remote needs authentication; run `cyber mcp auth remote` or /mcp"
    );
    assert_eq!(remote["tools"], json!([]));
    assert_eq!(http.requests().len(), 1);
    assert!(
        http.gets().is_empty(),
        "authentication failure fell back to SSE"
    );

    // required: true refuses the first Turn without contacting the server again.
    app.runtime
        .admit(&session.id, Admission::text("hello", Delivery::Queue))
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        app.runtime.wait_idle(&session.id),
    )
    .await
    .expect("drain did not settle");
    let mut statuses = Vec::new();
    let mut required = false;
    while let Ok(event) = updates.try_recv() {
        let text = serde_json::to_string(&event).unwrap();
        assert!(!text.contains("PRIVATE"), "{text}");
        match event {
            LiveEvent::McpStatusChanged { update, .. } => statuses.push(update.status),
            LiveEvent::Error { kind, message, .. } => {
                required |= kind == "mcp_required" && message == "McpRequiredError: remote";
            }
            _ => {}
        }
    }
    assert_eq!(statuses.last(), Some(&McpConnectionStatus::NeedsAuth));
    assert!(required, "required needs_auth server did not fail the Turn");
    assert_eq!(http.requests().len(), 1);
    app.host.close_mcp_location(&directory).await.unwrap();
    app.runtime.shutdown().await;
}

#[tokio::test]
async fn remote_legacy_sse_fallback_runs_through_app_host() {
    let http = mcp_http::McpHttp::start(mcp_http::Behavior {
        status: Some(405),
        legacy: true,
        ..mcp_http::Behavior::default()
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let app = application(&directory).await;
    std::fs::write(
        app.paths.config.join("cyber.json"),
        json!({
            "providers":provider(),
            "mcp":{"remote":{"type":"remote","url":http.url,"headers":{"authorization":"Bearer PRIVATE-HEADER"}}}
        })
        .to_string(),
    )
    .unwrap();
    open_session(&app, &directory).await;
    wait_running(&app, &directory).await;
    let status = mcp_status_body(&app).await;
    assert!(!status.contains("PRIVATE"), "{status}");
    let status: serde_json::Value = serde_json::from_str(&status).unwrap();
    assert_eq!(status["data"][0]["status"], "connected", "{status}");
    assert_eq!(status["data"][0]["tools"], json!(["mcp__remote__echo"]));
    assert_eq!(http.gets().len(), 1);
    assert_eq!(
        http.gets()[0].header("authorization"),
        Some("Bearer PRIVATE-HEADER")
    );
    app.host.close_mcp_location(&directory).await.unwrap();
    let records = mcp_connections(&app.store, &directory).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].phase, McpConnectionPhase::Settled);
    assert_eq!(records[0].acknowledged, Some(true));
    assert!(http.deletes().is_empty());
    app.runtime.shutdown().await;
}

fn remote_turn(directory: &Path, session: &str) -> cyber_server::runtime::TurnContext {
    cyber_server::runtime::TurnContext {
        session_id: session.into(),
        directory: directory.display().to_string(),
        agent: "build".into(),
        mode: "default".into(),
        prefers_apply_patch: false,
        rules: serde_json::Value::Null,
    }
}

/// Wait until the app publishes an MCP definition named `name` other than `previous`.
async fn wait_definition(
    app: &App,
    turn: &cyber_server::runtime::TurnContext,
    name: &str,
    previous: Option<&cyber_server::runtime::ToolDef>,
) -> cyber_server::runtime::ToolDef {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            if let Some(definition) = app.host.definitions(turn).into_iter().find(|definition| {
                definition.spec.name == name
                    && previous
                        .is_none_or(|previous| previous.registration != definition.registration)
            }) {
                return definition;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{name} was never published"))
}

async fn invoke_remote(
    app: &App,
    turn: &cyber_server::runtime::TurnContext,
    definition: &cyber_server::runtime::ToolDef,
    text: &str,
) -> cyber_server::runtime::ToolOutcome {
    let invocation = cyber_server::runtime::Invocation {
        registration: definition.registration.clone(),
        session_id: turn.session_id.clone(),
        directory: turn.directory.clone(),
        agent: turn.agent.clone(),
        mode: turn.mode.clone(),
        message_id: "msg_remote".into(),
        call_id: format!("call_{text}"),
        name: definition.spec.name.clone(),
        input: json!({"text":text}),
        attempt: 1,
        operation_key: format!("op_{text}"),
        asker: cyber_server::runtime::Asker::detached(),
        rules: serde_json::Value::Null,
    };
    app.host
        .execute(invocation, tokio_util::sync::CancellationToken::new())
        .await
}

/// A streaming remote server configured through the app, with one open session.
async fn streaming_app(
    behavior: mcp_http::Behavior,
) -> (
    mcp_http::McpHttp,
    tempfile::TempDir,
    std::path::PathBuf,
    App,
    String,
) {
    let http = mcp_http::McpHttp::start(mcp_http::Behavior {
        stream: true,
        ..behavior
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let app = application(&directory).await;
    std::fs::write(
        app.paths.config.join("cyber.json"),
        json!({
            "providers":provider(),
            "mcp":{"remote":{"type":"remote","url":http.url,"headers":{"authorization":"Bearer PRIVATE-HEADER"}}}
        })
        .to_string(),
    )
    .unwrap();
    let session = app
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: directory.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    wait_running(&app, &directory).await;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !http.stream_open() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the app never opened the GET event stream");
    (http, root, directory, app, session.id)
}

#[tokio::test]
async fn remote_closed_stream_reopens_while_a_call_runs() {
    use cyber_server::runtime::{LiveEvent, ToolOutcome};
    let (http, _root, directory, app, session) = streaming_app(mcp_http::Behavior {
        sse: true,
        delay: std::time::Duration::from_millis(400),
        ..mcp_http::Behavior::default()
    })
    .await;
    let turn = remote_turn(&directory, &session);
    let definition = wait_definition(&app, &turn, "mcp__remote__echo", None).await;
    let mut updates = app.runtime.subscribe();
    // An intermediary idle cutoff ends the GET stream in the middle of a tool call.
    let (outcome, ()) = tokio::join!(
        invoke_remote(&app, &turn, &definition, "during reopen"),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            http.drop_stream();
        }
    );
    assert_eq!(outcome, ToolOutcome::Ok("during reopen".into()));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while http.gets().len() < 2 || !http.stream_open() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the app never reopened the GET event stream");
    assert_eq!(
        http.gets()[1].header("mcp-session-id"),
        Some(mcp_http::SESSION)
    );
    assert_eq!(
        invoke_remote(&app, &turn, &definition, "after reopen").await,
        ToolOutcome::Ok("after reopen".into())
    );
    assert_eq!(http.posts("initialize").len(), 1);
    let records = mcp_connections(&app.store, &directory).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].phase, McpConnectionPhase::Running);
    while let Ok(event) = updates.try_recv() {
        assert!(
            !matches!(event, LiveEvent::McpStatusChanged { .. }),
            "a reopened stream changed MCP status"
        );
    }
    app.host.close_mcp_location(&directory).await.unwrap();
    app.runtime.shutdown().await;
}

#[tokio::test]
async fn remote_expired_session_reconnects_with_events() {
    use cyber_server::runtime::{LiveEvent, McpConnectionStatus, ToolOutcome};
    let (http, _root, directory, app, session) = streaming_app(mcp_http::Behavior::default()).await;
    let turn = remote_turn(&directory, &session);
    let old = wait_definition(&app, &turn, "mcp__remote__echo", None).await;
    let mut updates = app.runtime.subscribe();
    // The reopened GET stream finds the session gone: that is loss.
    http.expire_session();
    http.drop_stream();
    let new = wait_definition(&app, &turn, "mcp__remote__echo", Some(&old)).await;
    let records = mcp_connections(&app.store, &directory).unwrap();
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
    assert_eq!(http.posts("initialize").len(), 2);
    let second = format!("{}-2", mcp_http::SESSION);
    assert_eq!(
        http.gets().last().unwrap().header("mcp-session-id"),
        Some(second.as_str())
    );
    // An expired session is not deleted.
    assert!(http.deletes().is_empty());
    let mut statuses = Vec::new();
    let mut connections = Vec::new();
    while let Ok(event) = updates.try_recv() {
        let text = serde_json::to_string(&event).unwrap();
        assert!(!text.contains("PRIVATE"), "{text}");
        if let LiveEvent::McpStatusChanged { update, .. } = event {
            statuses.push(update.status);
            if !connections.contains(&update.connection_id) {
                connections.push(update.connection_id);
            }
        }
    }
    assert_eq!(connections.len(), 2, "{statuses:?}");
    assert_eq!(
        statuses.first(),
        Some(&McpConnectionStatus::Failed),
        "{statuses:?}"
    );
    assert_eq!(
        statuses.get(1),
        Some(&McpConnectionStatus::Connecting),
        "{statuses:?}"
    );
    assert_eq!(
        statuses.last(),
        Some(&McpConnectionStatus::Connected),
        "{statuses:?}"
    );
    assert!(matches!(
        invoke_remote(&app, &turn, &old, "stale").await,
        ToolOutcome::Failed(_)
    ));
    assert_eq!(
        invoke_remote(&app, &turn, &new, "after reconnect").await,
        ToolOutcome::Ok("after reconnect".into())
    );
    let call = http.posts("tools/call");
    assert_eq!(call.len(), 1);
    assert_eq!(call[0].header("mcp-session-id"), Some(second.as_str()));
    app.host.close_mcp_location(&directory).await.unwrap();
    app.runtime.shutdown().await;
}

#[tokio::test]
async fn remote_refused_stream_reopen_settles_needs_auth_without_reconnecting() {
    use cyber_server::runtime::McpConnectionStatus;
    let (http, _root, directory, app, _session) =
        streaming_app(mcp_http::Behavior::default()).await;
    http.fail_with(Some(401));
    http.drop_stream();
    let record = wait_record(&app, &directory, |record| {
        record.phase == McpConnectionPhase::Settled
            && record.status == McpConnectionStatus::NeedsAuth
    })
    .await;
    assert_eq!(record.acknowledged, Some(true));
    assert_eq!(http.posts("initialize").len(), 1);
    // Backoff would retry within two seconds; authentication failure must not.
    tokio::time::sleep(std::time::Duration::from_millis(3500)).await;
    assert_eq!(http.posts("initialize").len(), 1);
    assert_eq!(mcp_connections(&app.store, &directory).unwrap().len(), 1);
    let status = mcp_status_body(&app).await;
    assert!(!status.contains("PRIVATE"), "{status}");
    let status: serde_json::Value = serde_json::from_str(&status).unwrap();
    assert_eq!(status["data"][0]["status"], "needs_auth", "{status}");
    app.host.close_mcp_location(&directory).await.unwrap();
    app.runtime.shutdown().await;
}

#[tokio::test]
async fn remote_get_stream_refusal_after_initialize_settles_needs_auth() {
    use cyber_server::runtime::McpConnectionStatus;
    let http = mcp_http::McpHttp::start(mcp_http::Behavior {
        get_status: Some(403),
        ..mcp_http::Behavior::default()
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let app = application(&directory).await;
    std::fs::write(
        app.paths.config.join("cyber.json"),
        json!({
            "providers":provider(),
            "mcp":{"remote":{"type":"remote","url":http.url,"headers":{"authorization":"Bearer PRIVATE-HEADER"}}}
        })
        .to_string(),
    )
    .unwrap();
    open_session(&app, &directory).await;
    let record = wait_record(&app, &directory, |record| {
        record.phase == McpConnectionPhase::Settled
    })
    .await;
    assert_eq!(record.status, McpConnectionStatus::NeedsAuth);
    assert_eq!(record.acknowledged, Some(true));
    tokio::time::sleep(std::time::Duration::from_millis(3500)).await;
    assert_eq!(http.posts("initialize").len(), 1);
    assert_eq!(http.gets().len(), 1);
    assert_eq!(mcp_connections(&app.store, &directory).unwrap().len(), 1);
    let status = mcp_status_body(&app).await;
    assert!(!status.contains("PRIVATE"), "{status}");
    let status: serde_json::Value = serde_json::from_str(&status).unwrap();
    assert_eq!(status["data"][0]["status"], "needs_auth", "{status}");
    app.host.close_mcp_location(&directory).await.unwrap();
    app.runtime.shutdown().await;
}

#[tokio::test]
async fn remote_list_changed_updates_app_definitions() {
    use cyber_server::runtime::ToolOutcome;
    let (http, _root, directory, app, session) = streaming_app(mcp_http::Behavior::default()).await;
    let turn = remote_turn(&directory, &session);
    wait_definition(&app, &turn, "mcp__remote__echo", None).await;
    http.set_tools(&["echo", "fresh"]);
    assert!(http.push(json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"})));
    let fresh = wait_definition(&app, &turn, "mcp__remote__fresh", None).await;
    assert!(
        app.host
            .definitions(&turn)
            .iter()
            .any(|definition| definition.spec.name == "mcp__remote__echo")
    );
    assert_eq!(
        invoke_remote(&app, &turn, &fresh, "fresh call").await,
        ToolOutcome::Ok("fresh call".into())
    );
    assert_eq!(
        http.posts("initialize").len(),
        1,
        "list_changed must not reconnect"
    );
    app.host.close_mcp_location(&directory).await.unwrap();
    app.runtime.shutdown().await;
}

/// A `headers_command` printing `file`: PowerShell on Windows, `/bin/sh` elsewhere.
fn print_command(file: &Path) -> String {
    let path = file.display().to_string();
    #[cfg(windows)]
    return format!(
        "Get-Content -Raw -LiteralPath '{}'",
        path.replace('\'', "''")
    );
    #[cfg(not(windows))]
    format!("cat '{}'", path.replace('\'', "'\\''"))
}

#[tokio::test]
async fn remote_headers_command_through_app_host() {
    use cyber_server::runtime::ToolOutcome;
    let http = mcp_http::McpHttp::start(mcp_http::Behavior::default()).await;
    http.require_authorization(Some("Bearer PRIVATE-CMD-1"));
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let tokens = directory.join("headers.json");
    std::fs::write(&tokens, r#"{"Authorization":"Bearer PRIVATE-CMD-1"}"#).unwrap();
    let app = application(&directory).await;
    let mut updates = app.runtime.subscribe();
    std::fs::write(
        app.paths.config.join("cyber.json"),
        json!({
            "providers":provider(),
            "mcp":{"remote":{"type":"remote","url":http.url,
                "headers_command":print_command(&tokens),"headers_refresh_seconds":1}}
        })
        .to_string(),
    )
    .unwrap();
    let session = app
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: directory.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    wait_running(&app, &directory).await;
    let status = mcp_status_body(&app).await;
    assert!(!status.contains("PRIVATE"), "{status}");
    let status: serde_json::Value = serde_json::from_str(&status).unwrap();
    assert_eq!(status["data"][0]["status"], "connected", "{status}");
    let turn = remote_turn(&directory, &session.id);
    let first = wait_definition(&app, &turn, "mcp__remote__echo", None).await;
    assert_eq!(
        invoke_remote(&app, &turn, &first, "authenticated").await,
        ToolOutcome::Ok("authenticated".into())
    );

    // A refresh that fails is loss; the reconnect re-runs the command for new headers.
    std::fs::write(&tokens, "PRIVATE-BROKEN").unwrap();
    wait_record(&app, &directory, |record| {
        record.phase == McpConnectionPhase::Settled
    })
    .await;
    http.require_authorization(Some("Bearer PRIVATE-CMD-2"));
    std::fs::write(&tokens, r#"{"Authorization":"Bearer PRIVATE-CMD-2"}"#).unwrap();
    let second = wait_definition(&app, &turn, "mcp__remote__echo", Some(&first)).await;
    assert_eq!(
        invoke_remote(&app, &turn, &second, "rotated").await,
        ToolOutcome::Ok("rotated".into())
    );
    let calls = http.posts("tools/call");
    assert_eq!(
        calls.last().unwrap().header("authorization"),
        Some("Bearer PRIVATE-CMD-2")
    );
    assert!(http.posts("initialize").len() >= 2);
    let records = mcp_connections(&app.store, &directory).unwrap();
    let text = serde_json::to_string(&records).unwrap();
    assert!(!text.contains("PRIVATE"), "{text}");
    let status = mcp_status_body(&app).await;
    assert!(!status.contains("PRIVATE"), "{status}");
    let mut seen = 0;
    while let Ok(event) = updates.try_recv() {
        let text = serde_json::to_string(&event).unwrap();
        assert!(!text.contains("PRIVATE"), "{text}");
        seen += 1;
    }
    assert!(seen > 0);
    app.host.close_mcp_location(&directory).await.unwrap();
    app.runtime.shutdown().await;
}

#[tokio::test]
async fn remote_reconnect_refused_by_authorization_settles_needs_auth_and_stops() {
    use cyber_server::runtime::McpConnectionStatus;
    let (http, _root, directory, app, _session) =
        streaming_app(mcp_http::Behavior::default()).await;
    // The reopened GET finds the session gone (loss); the reconnect's initialize is refused.
    http.fail_initialize(Some(401));
    http.expire_session();
    http.drop_stream();
    let record = wait_record(&app, &directory, |record| {
        record.phase == McpConnectionPhase::Settled
            && record.status == McpConnectionStatus::NeedsAuth
    })
    .await;
    assert_eq!(record.acknowledged, Some(true));
    let initializes = http.posts("initialize");
    assert_eq!(initializes.len(), 2);
    assert_eq!(initializes[1].status, 401);
    let records = mcp_connections(&app.store, &directory).unwrap();
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .any(|record| record.phase == McpConnectionPhase::Settled
                && record.status == McpConnectionStatus::Failed
                && record.acknowledged == Some(true))
    );
    // Backoff would retry within two seconds; authentication failure must not.
    tokio::time::sleep(std::time::Duration::from_millis(3500)).await;
    assert_eq!(http.posts("initialize").len(), 2);
    assert_eq!(mcp_connections(&app.store, &directory).unwrap().len(), 2);
    let status = mcp_status_body(&app).await;
    assert!(!status.contains("PRIVATE"), "{status}");
    let status: serde_json::Value = serde_json::from_str(&status).unwrap();
    assert_eq!(status["data"][0]["status"], "needs_auth", "{status}");
    app.host.close_mcp_location(&directory).await.unwrap();
    app.runtime.shutdown().await;
}

/// Configure `mcp` through the app and open one session.
async fn configured_app(
    config: serde_json::Value,
) -> (tempfile::TempDir, std::path::PathBuf, App, String) {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let app = application(&directory).await;
    let mut config = config;
    config["providers"] = provider();
    std::fs::write(app.paths.config.join("cyber.json"), config.to_string()).unwrap();
    let session = app
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: directory.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    (root, directory, app, session.id)
}

#[tokio::test]
async fn remote_expired_header_command_credential_reconnects_instead_of_needs_auth() {
    use cyber_server::runtime::{McpConnectionStatus, ToolOutcome};
    let http = mcp_http::McpHttp::start(mcp_http::Behavior::default()).await;
    http.require_authorization(Some("Bearer PRIVATE-CMD-1"));
    let tokens = tempfile::tempdir().unwrap();
    let tokens = tokens.path().canonicalize().unwrap().join("headers.json");
    std::fs::write(&tokens, r#"{"Authorization":"Bearer PRIVATE-CMD-1"}"#).unwrap();
    let (_root, directory, app, session) = configured_app(json!({
        "mcp":{"remote":{"type":"remote","url":http.url,"headers_command":print_command(&tokens)}}
    }))
    .await;
    let turn = remote_turn(&directory, &session);
    let first = wait_definition(&app, &turn, "mcp__remote__echo", None).await;
    // The credential expires before the (default hourly) refresh window ends.
    http.require_authorization(Some("Bearer PRIVATE-CMD-2"));
    std::fs::write(&tokens, r#"{"Authorization":"Bearer PRIVATE-CMD-2"}"#).unwrap();
    assert!(matches!(
        invoke_remote(&app, &turn, &first, "expired").await,
        ToolOutcome::Failed(_)
    ));
    let second = wait_definition(&app, &turn, "mcp__remote__echo", Some(&first)).await;
    assert_eq!(
        invoke_remote(&app, &turn, &second, "renewed").await,
        ToolOutcome::Ok("renewed".into())
    );
    let records = mcp_connections(&app.store, &directory).unwrap();
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .all(|record| record.status != McpConnectionStatus::NeedsAuth),
        "{records:?}"
    );
    let initializes = http.posts("initialize");
    assert_eq!(initializes.len(), 2);
    assert_eq!(
        initializes[1].header("authorization"),
        Some("Bearer PRIVATE-CMD-2")
    );
    app.host.close_mcp_location(&directory).await.unwrap();
    app.runtime.shutdown().await;
}

#[tokio::test]
async fn sandbox_selected_remote_server_never_runs_headers_command() {
    let http = mcp_http::McpHttp::start(mcp_http::Behavior::default()).await;
    let scratch = tempfile::tempdir().unwrap();
    let scratch = scratch.path().canonicalize().unwrap();
    let marker = scratch.join("command-ran");
    let tokens = scratch.join("headers.json");
    std::fs::write(&tokens, r#"{"Authorization":"Bearer PRIVATE-CMD"}"#).unwrap();
    #[cfg(windows)]
    let touch = format!(
        "New-Item -ItemType File -Force -Path '{}' | Out-Null",
        marker.display().to_string().replace('\'', "''")
    );
    #[cfg(not(windows))]
    let touch = format!("touch '{}'", marker.display());
    let (_root, directory, app, _session) = configured_app(json!({
        "sandbox":{"policy":"workspace-write","network":"on","apply_to":["mcp"]},
        "mcp":{"remote":{"type":"remote","url":http.url,
            "headers_command":format!("{touch}; {}", print_command(&tokens))}}
    }))
    .await;
    let record = wait_record(&app, &directory, |record| {
        record.phase == McpConnectionPhase::Settled
    })
    .await;
    assert_eq!(
        record.status,
        cyber_server::runtime::McpConnectionStatus::Failed
    );
    assert!(!marker.exists(), "headers_command ran outside the sandbox");
    assert!(http.requests().is_empty());
    app.host.close_mcp_location(&directory).await.unwrap();
    app.runtime.shutdown().await;
}

#[tokio::test]
async fn remote_tools_use_filters_permissions_hooks_and_output_caps_through_app_host() {
    use cyber_server::runtime::ToolOutcome;
    let http = mcp_http::McpHttp::start(mcp_http::Behavior::default()).await;
    http.set_tools(&["echo", "secret_dump", "blocked", "hooked"]);
    let deny = r#"echo '{"decision":"deny","reason":"remote hook denial"}'"#;
    let config = json!({
        "permissions":{"mcp__remote__blocked":"deny"},
        "hooks":{"PreToolUse":[{"matcher":"mcp__remote__hooked","hooks":[{"type":"command","command":deny}]}]},
        "mcp":{"remote":{"type":"remote","url":http.url,"tools":{"deny":["secret_*"]},"output_token_limit":2}}
    });
    let (_root, directory, app, session) = configured_app(config.clone()).await;
    let turn = remote_turn(&directory, &session);
    let echo = wait_definition(&app, &turn, "mcp__remote__echo", None).await;
    let hooked = wait_definition(&app, &turn, "mcp__remote__hooked", None).await;
    let names: Vec<_> = app
        .host
        .definitions(&turn)
        .into_iter()
        .map(|definition| definition.spec.name)
        .filter(|name| name.starts_with("mcp__remote__"))
        .collect();
    assert!(
        !names.contains(&"mcp__remote__secret_dump".to_string()),
        "{names:?}"
    );
    assert!(
        !names.contains(&"mcp__remote__blocked".to_string()),
        "{names:?}"
    );

    // The per-server cap bounds model-visible output and keeps the full text.
    let long = "remote-output ".repeat(20);
    let output = match invoke_remote(&app, &turn, &echo, &long).await {
        ToolOutcome::Ok(output) => output,
        other => panic!("unexpected outcome: {other:?}"),
    };
    assert!(output.contains("[output truncated:"), "{output}");
    let full = output
        .rsplit("full output at ")
        .next()
        .unwrap()
        .trim_end_matches(']');
    assert_eq!(std::fs::read_to_string(full).unwrap(), long);

    // A PreToolUse hook denies before any remote effect.
    match invoke_remote(&app, &turn, &hooked, "hooked").await {
        ToolOutcome::Failed(error) => assert!(error.contains("remote hook denial"), "{error}"),
        other => panic!("hook did not deny: {other:?}"),
    }
    // A deny rule added later refuses the published tool at call time.
    let mut config = config;
    config["providers"] = provider();
    config["permissions"]["mcp__remote__echo"] = json!("deny");
    std::fs::write(app.paths.config.join("cyber.json"), config.to_string()).unwrap();
    assert!(matches!(
        invoke_remote(&app, &turn, &echo, "denied").await,
        ToolOutcome::Failed(_)
    ));
    let calls: Vec<_> = http
        .posts("tools/call")
        .iter()
        .map(|call| call.body["params"]["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(calls, vec!["echo".to_string()]);
    app.host.close_mcp_location(&directory).await.unwrap();
    app.runtime.shutdown().await;
}
