//! Public MCP close uses the application's actual shared native owner.
use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
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
        "Remote MCP transport is not available"
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
