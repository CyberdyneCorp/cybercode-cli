//! PTY attach over a real TCP listener (`background-tasks` → PTY sessions): retained
//! output, cursors, live output, input and single-use tickets.

mod support;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyber_server::http::{
    self, AgentInfo, AppState, CommandInfo, HttpOptions, ModelInfo, Services,
};
use cyber_server::runtime::{PtyFeed, PtySpec, Runtime, ToolDef, TurnContext};
use futures::future::BoxFuture;
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use support::{Harness, Setup};
use tokio_tungstenite::tungstenite::Message;

const PASSWORD: &str = "secret";

struct NoServices;

impl Services for NoServices {
    fn models(&self, _location: &Path) -> BoxFuture<'_, Result<Vec<ModelInfo>, String>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn default_model(&self, _location: &Path) -> Option<String> {
        Some("test/main".into())
    }
    fn agents(&self, _location: &Path) -> Vec<AgentInfo> {
        Vec::new()
    }
    fn tools(&self, _turn: &TurnContext) -> Vec<ToolDef> {
        Vec::new()
    }
    fn commands(&self, _location: &Path) -> Vec<CommandInfo> {
        Vec::new()
    }
    fn find_files(&self, _location: &Path, _query: &str, _limit: usize) -> Vec<String> {
        Vec::new()
    }
    fn expand_command(&self, _location: &Path, _name: &str, _arguments: &str) -> Option<String> {
        None
    }
}

/// A password-protected server on a loopback port.
async fn serve(h: &Harness) -> String {
    let state = AppState {
        service: None,
        runtime: h.runtime.clone(),
        remote_tools: Arc::default(),
        store: Arc::clone(&h.store),
        services: Arc::new(NoServices),
        options: Arc::new(HttpOptions {
            version: "t".into(),
            password: Some(PASSWORD.into()),
            cors_origins: Vec::new(),
            default_directory: h.repo.clone(),
            features: Vec::new(),
        }),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(http::serve_tcp(
        http::router(state),
        listener,
        std::future::pending(),
    ));
    format!("127.0.0.1:{}", addr.port())
}

/// A PTY whose input is recorded.
fn pty(runtime: &Runtime, session: &str) -> (PtyFeed, Arc<Mutex<Vec<u8>>>) {
    let input = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&input);
    let feed = runtime.register_pty(PtySpec {
        session_id: session.into(),
        job_id: "job_pty".into(),
        command: "python3".into(),
        cols: 80,
        rows: 24,
        input: Box::new(move |bytes| {
            sink.lock().unwrap().extend_from_slice(bytes);
            Ok(())
        }),
    });
    (feed, input)
}

async fn ticket(addr: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{addr}/api/v1/jobs/job_pty/pty/ticket"))
        .basic_auth("cyber", Some(PASSWORD))
        .send()
        .await
        .unwrap()
}

async fn issue(addr: &str) -> String {
    let response = ticket(addr).await;
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["data"]["expires_in"], 60);
    body["data"]["ticket"].as_str().unwrap().to_string()
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Attach without credentials other than the ticket.
async fn attach(addr: &str, ticket: &str, cursor: Option<i64>) -> Result<Socket, u16> {
    let cursor = cursor.map(|c| format!("&cursor={c}")).unwrap_or_default();
    let url = format!("ws://{addr}/api/v1/jobs/job_pty/pty/connect?ticket={ticket}{cursor}");
    match tokio_tungstenite::connect_async(url).await {
        Ok((socket, _)) => Ok(socket),
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            Err(response.status().as_u16())
        }
        Err(other) => panic!("connect: {other}"),
    }
}

async fn next(socket: &mut Socket) -> Message {
    tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await
        .expect("a frame")
        .expect("open")
        .unwrap()
}

fn control(message: Message) -> Value {
    serde_json::from_str(message.to_text().unwrap()).unwrap()
}

/// Binary frames until `want` bytes arrived.
async fn output(socket: &mut Socket, want: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    while bytes.len() < want {
        match next(socket).await {
            Message::Binary(chunk) => bytes.extend_from_slice(&chunk),
            other => panic!("expected output, got {other:?}"),
        }
    }
    bytes
}

#[tokio::test]
async fn reattach_without_cursor_replays_buffer_then_live() {
    let h = Harness::new(Setup::default());
    let session = h.session().await;
    let (feed, input) = pty(&h.runtime, &session);
    feed.push(b">>> ");
    feed.push(b"print(2+2)\r\n4\r\n");
    let addr = serve(&h).await;

    let mut first = attach(&addr, &issue(&addr).await, None).await.unwrap();
    assert_eq!(control(next(&mut first).await)["cursor"], 0);
    assert_eq!(output(&mut first, 19).await, b">>> print(2+2)\r\n4\r\n");
    first
        .send(Message::Binary(b"1+1\r".to_vec().into()))
        .await
        .unwrap();
    first.send(Message::Text("\u{3}".into())).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while input.lock().unwrap().len() < 5 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(*input.lock().unwrap(), b"1+1\r\x03");
    drop(first);

    // Reconnecting with no cursor replays the retained buffer, then streams live output.
    let mut again = attach(&addr, &issue(&addr).await, None).await.unwrap();
    assert_eq!(control(next(&mut again).await)["cursor"], 0);
    assert_eq!(output(&mut again, 19).await, b">>> print(2+2)\r\n4\r\n");
    feed.push(b"2\r\n");
    assert_eq!(output(&mut again, 3).await, b"2\r\n");

    // A cursor resumes where a client stopped; -1 tails.
    let mut resumed = attach(&addr, &issue(&addr).await, Some(17)).await.unwrap();
    assert_eq!(control(next(&mut resumed).await)["cursor"], 17);
    assert_eq!(output(&mut resumed, 5).await, b"\r\n2\r\n");
    let mut tail = attach(&addr, &issue(&addr).await, Some(-1)).await.unwrap();
    assert_eq!(control(next(&mut tail).await)["cursor"], 22);
    feed.push(b">>> ");
    assert_eq!(output(&mut tail, 4).await, b">>> ");

    // The program exiting ends every attachment with its final cursor.
    drop(feed);
    assert_eq!(output(&mut again, 4).await, b">>> ");
    let exit = control(next(&mut again).await);
    assert_eq!(exit, json!({"type": "exit", "cursor": 26}));
    assert!(h.runtime.pty("job_pty").is_none());
    assert_eq!(ticket(&addr).await.status(), 404);
}

#[tokio::test]
async fn ticket_is_single_use_and_expires_in_60s() {
    let h = Harness::new(Setup::default());
    let session = h.session().await;
    let (_feed, _input) = pty(&h.runtime, &session);
    let addr = serve(&h).await;

    // Tickets need the server's credentials; the socket needs only the ticket.
    let anonymous = reqwest::Client::new()
        .post(format!("http://{addr}/api/v1/jobs/job_pty/pty/ticket"))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);
    assert_eq!(attach(&addr, "", None).await.err(), Some(401));
    assert_eq!(attach(&addr, "ptyt_forged", None).await.err(), Some(401));

    let once = issue(&addr).await;
    let socket = attach(&addr, &once, None).await.unwrap();
    drop(socket);
    assert_eq!(attach(&addr, &once, None).await.err(), Some(401));

    // A ticket is bound to its PTY, and any attempt consumes it.
    let other = h.runtime.issue_pty_ticket("job_pty").unwrap();
    assert!(!h.runtime.redeem_pty_ticket(&other, "job_other"));
    assert!(!h.runtime.redeem_pty_ticket(&other, "job_pty"));
    assert!(h.runtime.issue_pty_ticket("job_other").is_err());

    tokio::time::pause();
    let early = h.runtime.issue_pty_ticket("job_pty").unwrap();
    let late = h.runtime.issue_pty_ticket("job_pty").unwrap();
    tokio::time::advance(Duration::from_secs(59)).await;
    assert!(h.runtime.redeem_pty_ticket(&early, "job_pty"));
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(!h.runtime.redeem_pty_ticket(&late, "job_pty"));
}
