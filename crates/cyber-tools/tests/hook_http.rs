//! Real POSTs, bounded decisions and owned transport settlement without Sessions.
mod support;
use std::collections::HashMap;
use std::time::Duration;

use cyber_core::config::{self, LoadRequest, Resolved};
use cyber_core::hooks::{HookEvent, HookLocation, HookOutcome};
use cyber_core::paths::Paths;
use cyber_core::trust::TrustStore;
use cyber_server::runtime::{HookExecutionRecord, HookExecutionStatus, SubtreeStopStatus};
use cyber_tools::hook_http::HookHttpRunner;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

const POINTER: &str = "/hooks/PreToolUse/0/hooks/0";

struct Fixture {
    f: support::Fixture,
    resolved: Resolved,
    trust: TrustStore,
    event: HookEvent,
}
impl Fixture {
    fn new(handler: Value, sandbox: Value) -> Self {
        let f = support::Fixture::new();
        let (resolved, trust) = configuration(&f, handler, sandbox);
        let event = HookEvent::synthetic(
            "PreToolUse",
            HookLocation {
                directory: f.repo.clone(),
                workspace: None,
            },
            "global".into(),
            "build".into(),
            "default".into(),
            1,
            json!({"tool_name":"write","tool_input":{"private":"private event input"}}),
        )
        .unwrap();
        Self {
            f,
            resolved,
            trust,
            event,
        }
    }
    fn runner(&self) -> HookHttpRunner<'_> {
        HookHttpRunner {
            resolved: &self.resolved,
            trust: &self.trust,
            invocation_trust: None,
            home: self.f.dir.path(),
        }
    }
    fn receipts(&self) -> Vec<HookExecutionRecord> {
        self.f
            .store
            .read(|conn| {
                let mut query = conn.prepare("SELECT data FROM hook_test_execution ORDER BY id")?;
                query
                    .query_map([], |row| row.get::<_, String>(0))?
                    .map(|row| Ok(serde_json::from_str(&row?).unwrap()))
                    .collect()
            })
            .unwrap()
    }
    fn assert_no_sessions(&self) {
        let count: i64 = self
            .f
            .store
            .read(|conn| Ok(conn.query_row("SELECT count(*) FROM session", [], |row| row.get(0))?))
            .unwrap();
        assert_eq!(count, 0);
    }
}
fn configuration(f: &support::Fixture, handler: Value, sandbox: Value) -> (Resolved, TrustStore) {
    let env = HashMap::from([
        (
            "CYBER_HOME".into(),
            f.dir.path().join("cyber").display().to_string(),
        ),
        ("POLICY_TOKEN".into(), "private header credential".into()),
    ]);
    let paths = Paths::resolve(&env, f.dir.path());
    paths.ensure().unwrap();
    std::fs::write(
        paths.config.join("cyber.jsonc"),
        json!({"sandbox":sandbox,"hooks":{"PreToolUse":[{"hooks":[handler]}]}}).to_string(),
    )
    .unwrap();
    let resolved = config::load(&LoadRequest {
        location: &f.repo,
        paths: &paths,
        env: &env,
        home: f.dir.path(),
        profile: None,
        overrides: &[],
        flags: json!({}),
    })
    .unwrap();
    f.set_config(resolved.value.clone());
    (resolved, TrustStore::new(paths.trust_file()))
}

async fn request(socket: &mut TcpStream) -> (String, Value) {
    let mut bytes = Vec::new();
    let header_end = loop {
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break end + 4;
        }
        let mut buffer = [0; 4096];
        let count = socket.read(&mut buffer).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&buffer[..count]);
    };
    let head = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
    let length = head
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|value| value.trim().parse::<usize>().unwrap())
        })
        .unwrap();
    while bytes.len() < header_end + length {
        let mut buffer = [0; 4096];
        let count = socket.read(&mut buffer).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&buffer[..count]);
    }
    (
        head,
        serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap(),
    )
}

async fn server(
    status: u16,
    body: Vec<u8>,
    extra: &str,
) -> (String, tokio::task::JoinHandle<(String, Value)>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/policy", listener.local_addr().unwrap());
    let extra = extra.to_string();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let captured = request(&mut socket).await;
        let head = format!(
            "HTTP/1.1 {status} Response\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
            body.len()
        );
        let _ = socket.write_all(head.as_bytes()).await;
        let _ = socket.write_all(&body).await;
        captured
    });
    (url, task)
}

async fn waiting_server() -> (String, oneshot::Receiver<()>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/policy", listener.local_addr().unwrap());
    let (received, receiver) = oneshot::channel();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        request(&mut socket).await;
        received.send(()).unwrap();
        let mut byte = [0];
        let closed = tokio::time::timeout(Duration::from_secs(3), socket.read(&mut byte))
            .await
            .unwrap();
        assert!(matches!(closed, Ok(0) | Err(_)));
    });
    (url, receiver, task)
}

#[tokio::test]
async fn posts_flat_event_substituted_headers_and_parses_private_durable_decision() {
    let (url,served)=server(200,br#"{"decision":"deny","reason":"remote policy","updated_input":{"path":"changed"},"unknown":1}"#.to_vec(),"").await;
    let f = Fixture::new(
        json!({"type":"http","url":url,"id":"policy","headers":{"Authorization":"Bearer {env:POLICY_TOKEN}","content-type":"text/plain"}}),
        json!({}),
    );
    let report = f
        .runner()
        .run_test(&f.f.host, POINTER, &f.event, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(report.outcome, HookOutcome::Blocked);
    assert_eq!(report.decision.updated_input.unwrap()["path"], "changed");
    assert_eq!(report.ignored_fields, vec!["unknown"]);
    let (head, body) = served.await.unwrap();
    assert!(head.starts_with("POST /policy HTTP/1.1"));
    assert!(
        head.to_ascii_lowercase()
            .contains("content-type: application/json")
    );
    assert!(head.contains("Bearer private header credential"));
    assert_eq!(body, f.event.as_json().clone());
    let records = f.receipts();
    assert_eq!(records[0].status, HookExecutionStatus::Completed);
    assert!(records[0].io.is_none());
    let facts =
        f.f.store
            .read_events(&records[0].id, -1, 10)
            .unwrap()
            .events;
    for private in ["private event input", "private header credential"] {
        assert!(!serde_json::to_string(&facts).unwrap().contains(private));
    }
    f.assert_no_sessions();
}

#[tokio::test]
async fn status_nonobject_malformed_oversized_and_redirect_responses_respect_fail_closed() {
    for (status, body, extra) in [
        (500, b"failure".to_vec(), ""),
        (200, b"[]".to_vec(), ""),
        (200, b"{ malformed".to_vec(), ""),
        (200, vec![b' '; 1024 * 1024 + 1], ""),
        (
            302,
            b"redirect".to_vec(),
            "Location: http://127.0.0.1:9/forbidden\r\n",
        ),
    ] {
        let (url, served) = server(status, body, extra).await;
        let f = Fixture::new(
            json!({"type":"http","url":url,"fail_closed":true}),
            json!({}),
        );
        let report = f
            .runner()
            .run_test(&f.f.host, POINTER, &f.event, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(report.outcome, HookOutcome::Error);
        assert_eq!(
            report.decision.decision,
            Some(cyber_core::hooks::HookAction::Deny)
        );
        assert!(report.acknowledged && !report.must_stop);
        served.await.unwrap();
        f.assert_no_sessions();
    }
}

#[tokio::test]
async fn scoped_network_off_and_denied_allowlist_refuse_before_upstream_connection() {
    for sandbox in [
        json!({"network":"off"}),
        json!({"network":"proxy","allowed_domains":[]}),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut f = Fixture::new(
            json!({"type":"http","url":format!("http://{}/policy",listener.local_addr().unwrap())}),
            sandbox,
        );
        f.resolved.value["hooks"]["sandbox_all"] = json!(true);
        let report = f
            .runner()
            .run_test(&f.f.host, POINTER, &f.event, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(report.outcome, HookOutcome::Error);
        assert!(report.decision.decision.is_none());
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
        f.assert_no_sessions();
    }
}

#[tokio::test]
async fn scoped_allowlisted_post_and_opted_in_io_still_omit_headers() {
    let (url, served) = server(200, br#"{"decision":"allow"}"#.to_vec(), "").await;
    let mut f = Fixture::new(
        json!({"type":"http","url":url,"headers":{"Authorization":"private header credential"}}),
        json!({"network":"proxy","allowed_domains":["127.0.0.1"]}),
    );
    f.resolved.value["hooks"]["sandbox_all"] = json!(true);
    f.resolved.value["telemetry"] = json!({"log_hook_io":true});
    f.runner()
        .run_test(&f.f.host, POINTER, &f.event, CancellationToken::new())
        .await
        .unwrap();
    served.await.unwrap();
    let records = f.receipts();
    let io = records[0].io.as_ref().unwrap();
    assert!(io.stdin.contains("private event input"));
    assert_eq!(io.stdout, r#"{"decision":"allow"}"#);
    assert!(
        !serde_json::to_string(&records)
            .unwrap()
            .contains("private header credential")
    );
    f.assert_no_sessions();
}

#[tokio::test]
async fn timeout_is_nonblocking_unless_fail_closed_and_settles_local_transport() {
    for closed in [false, true] {
        let (url, _received, served) = waiting_server().await;
        let f = Fixture::new(
            json!({"type":"http","url":url,"timeout":1,"fail_closed":closed}),
            json!({}),
        );
        let report = f
            .runner()
            .run_test(&f.f.host, POINTER, &f.event, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(report.outcome, HookOutcome::Timeout);
        assert!(report.acknowledged);
        assert_eq!(report.decision.decision.is_some(), closed);
        served.await.unwrap();
        f.assert_no_sessions();
    }
}

#[tokio::test]
async fn session_subtree_stop_cancels_and_acknowledges_owned_http_transport() {
    let flow = support::flow::Flow::new(Vec::new(), false);
    let session = flow.session("default").await;
    let (url, received, served) = waiting_server().await;
    let (resolved, trust) = configuration(&flow.f, json!({"type":"http","url":url}), json!({}));
    let event = HookEvent::new(
        "PreToolUse",
        cyber_core::hooks::HookIdentity {
            session_id: session.clone(),
            location: HookLocation {
                directory: flow.f.repo.clone(),
                workspace: None,
            },
            project_id: "global".into(),
            agent: "build".into(),
            mode: "default".into(),
        },
        1,
        Default::default(),
    )
    .unwrap();
    let runner = HookHttpRunner {
        resolved: &resolved,
        trust: &trust,
        invocation_trust: None,
        home: flow.f.dir.path(),
    };
    let mut running =
        Box::pin(runner.run_recorded(&flow.runtime, POINTER, &event, CancellationToken::new()));
    tokio::select! { result=&mut running=>panic!("HTTP request ended early: {result:?}"), result=received=>result.unwrap() }
    let (report, stopped) = tokio::join!(running, flow.runtime.stop_subtree(&session));
    assert!(report.unwrap().must_stop);
    assert_eq!(stopped.unwrap().status, SubtreeStopStatus::Acknowledged);
    served.await.unwrap();
    assert_eq!(
        flow.runtime.hook_executions(&session, 10).unwrap()[0].status,
        HookExecutionStatus::Completed
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn disposed_owned_http_request_retains_unknown_receipt() {
    let (url, received, served) = waiting_server().await;
    let f = Fixture::new(json!({"type":"http","url":url}), json!({}));
    let runner = f.runner();
    let mut running =
        Box::pin(runner.run_test(&f.f.host, POINTER, &f.event, CancellationToken::new()));
    tokio::select! { result=&mut running=>panic!("HTTP request ended early: {result:?}"), result=received=>result.unwrap() }
    drop(running);
    served.await.unwrap();
    let record = f.receipts().remove(0);
    assert_eq!(record.status, HookExecutionStatus::Unknown);
    assert!(record.must_stop);
    f.assert_no_sessions();
}

#[tokio::test]
async fn once_http_handler_is_not_posted_again_within_test_invocation() {
    let (url, served) = server(200, br#"{"decision":"allow"}"#.to_vec(), "").await;
    let f = Fixture::new(json!({"type":"http","url":url,"once":true}), json!({}));
    let runner = f.runner();
    assert_eq!(
        runner
            .run_test(&f.f.host, POINTER, &f.event, CancellationToken::new())
            .await
            .unwrap()
            .outcome,
        HookOutcome::Ok
    );
    served.await.unwrap();
    assert_eq!(
        runner
            .run_test(&f.f.host, POINTER, &f.event, CancellationToken::new())
            .await
            .unwrap()
            .outcome,
        HookOutcome::Skipped
    );
    assert_eq!(f.receipts().len(), 1);
    f.assert_no_sessions();
}

#[tokio::test]
async fn invalid_or_authority_changing_headers_refuse_before_network_without_credentials_in_errors()
{
    for headers in [
        json!({"Host":"foreign.example"}),
        json!({"Content-Length":"1"}),
        json!({"Authorization":"private credential\r\ninjection"}),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let f = Fixture::new(
            json!({"type":"http","url":format!("http://{}/policy",listener.local_addr().unwrap()),"headers":headers,"fail_closed":true}),
            json!({}),
        );
        let report = f
            .runner()
            .run_test(&f.f.host, POINTER, &f.event, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(report.outcome, HookOutcome::Error);
        assert!(!report.diagnostic.unwrap().contains("private credential"));
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
        f.assert_no_sessions();
    }
}

#[tokio::test]
async fn streaming_response_without_declared_length_remains_bounded() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/policy", listener.local_addr().unwrap());
    let served = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        request(&mut socket).await;
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let _ = socket.write_all(&vec![b' '; 1024 * 1024 + 1]).await;
    });
    let f = Fixture::new(json!({"type":"http","url":url}), json!({}));
    let report = f
        .runner()
        .run_test(&f.f.host, POINTER, &f.event, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(report.outcome, HookOutcome::Error);
    assert!(report.diagnostic.unwrap().contains("exceeds 1 MiB"));
    assert!(report.decision.decision.is_none());
    served.await.unwrap();
    f.assert_no_sessions();
}
