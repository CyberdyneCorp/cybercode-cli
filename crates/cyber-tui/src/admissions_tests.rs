use super::*;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

struct Mock {
    client: Client,
    log: Arc<Mutex<Vec<(String, Value)>>>,
    stop: Arc<AtomicBool>,
    actor: Option<thread::JoinHandle<()>>,
}
impl Mock {
    fn new(responses: Vec<fn(&str) -> String>, saved: Option<std::path::PathBuf>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = Client::http(&format!("http://{}", listener.local_addr().unwrap()), None);
        listener.set_nonblocking(true).unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (record, stopping) = (log.clone(), stop.clone());
        let actor = thread::spawn(move || {
            let mut responses = responses.into_iter();
            while !stopping.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let Some((head, body)) = read(&mut stream) else {
                    continue;
                };
                let path = head.split_whitespace().nth(1).unwrap();
                let id = path.trim_end_matches("/stop").rsplit('/').next().unwrap();
                if let Some(dir) = &saved {
                    assert!(dir.join("admissions").join(format!("{id}.json")).is_file());
                }
                let response = responses.next().expect("unexpected extra API request")(id);
                record.lock().unwrap().push((head, body));
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.len(),
                    response
                );
            }
        });
        Self {
            client,
            log,
            stop,
            actor: Some(actor),
        }
    }
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Err(panic) = self.actor.take().unwrap().join()
            && !thread::panicking()
        {
            std::panic::resume_unwind(panic);
        }
    }
}
fn read(stream: &mut TcpStream) -> Option<(String, Value)> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 2048];
    let boundary = loop {
        let n = stream.read(&mut buffer).ok()?;
        if n == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..n]);
        if let Some(index) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let header = String::from_utf8(bytes[..boundary].to_vec()).unwrap();
    let length = header
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while bytes.len() < boundary + length {
        let n = stream.read(&mut buffer).ok()?;
        if n == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..n]);
    }
    Some((
        header.lines().next().unwrap().into(),
        serde_json::from_slice(&bytes[boundary..]).unwrap_or(Value::Null),
    ))
}
fn request() -> Request {
    Request {
        id: "op_test".into(),
        source: "ses_1".into(),
        directory: "/repo".into(),
    }
}
fn record(id: &str, status: &str, phase: &str, job: Option<&str>) -> Value {
    json!({"id":id,"session_id":"ses_1","status":status,"phase":phase,"job_id":job})
}
fn pending(id: &str) -> String {
    json!({"data":record(id,"pending","reserved",None)}).to_string()
}
fn admitted(id: &str) -> String {
    json!({"data":record(id,"admitted","launching",Some("job_owned"))}).to_string()
}
fn cancelled(id: &str) -> String {
    json!({"data":record(id,"cancelled","reserved",None)}).to_string()
}
fn updated(msg: Msg) -> (Request, Result<Value, String>) {
    let Msg::AdmissionUpdated {
        request, result, ..
    } = msg
    else {
        panic!("Expected admission result")
    };
    (request, result)
}

#[tokio::test]
async fn admission_persists_before_post_and_recovers_a_lost_response_without_redispatch() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::new(dir.path());
    let server = Mock::new(vec![|_| "{".into(), admitted], Some(dir.path().into()));
    let (tx, mut rx) = mpsc::channel(4);
    let context = Context {
        store: &store,
        tx: &tx,
    };
    let (request, result) = updated(
        start(
            &server.client,
            "ses_1",
            "/repo",
            json!({"prompt":"find retry logic","agent":"explore"}),
            Some(&context),
        )
        .await
        .unwrap(),
    );
    assert_eq!(result.unwrap()["job_id"], "job_owned");
    assert_eq!(store.admissions(), vec![request.clone()]);
    let Ok(Msg::AdmissionStarted(registered)) = rx.recv().await.unwrap() else {
        panic!("Missing registration")
    };
    assert_eq!(registered, request);
    let log = server.log.lock().unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].0, format!("POST /api/v1{} HTTP/1.1", request.path()));
    assert_eq!(log[1].0, format!("GET /api/v1{} HTTP/1.1", request.path()));
    assert_eq!(log[0].1["prompt"], "find retry logic");
    assert!(log[1].1.is_null());
}
#[tokio::test]
async fn reconnect_looks_up_saved_identity_without_resubmitting_the_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::new(dir.path());
    store.save_admission(&request()).unwrap();
    store.save_admission(&request()).unwrap();
    let mut restored = LocalStore::new(dir.path()).admissions();
    let server = Mock::new(vec![pending], Some(dir.path().into()));
    let (identity, result) = updated(perform(&server.client, restored.pop().unwrap(), false).await);
    assert_eq!(identity, request());
    assert_eq!(result.unwrap()["status"], "pending");
    assert!(server.log.lock().unwrap()[0].0.starts_with("GET "));
    assert!(
        !std::fs::read_to_string(dir.path().join("admissions/op_test.json"))
            .unwrap()
            .contains("prompt")
    );
}
#[test]
fn cancelled_admission_cannot_be_revived_by_delayed_submission_or_transport_error() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::new(dir.path());
    store.save_admission(&request()).unwrap();
    let mut tracker = Admissions::default();
    tracker.register(request());
    tracker.0.get_mut("op_test").unwrap().stopping = true;
    tracker.update(
        request(),
        Ok(record("op_test", "cancelled", "reserved", None)),
        true,
        &store,
    );
    tracker.update(
        request(),
        Ok(record("op_test", "pending", "reserved", None)),
        false,
        &store,
    );
    tracker.update(request(), Err("lost response".into()), false, &store);
    assert_eq!(tracker.0["op_test"].status, "cancelled");
    assert!(tracker.pending("ses_1").is_none());
    assert!(tracker.poll().is_empty());
    assert!(store.admissions().is_empty());
}
#[test]
fn racing_job_handoff_keeps_cancellation_and_recovery_identity_until_acknowledged() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::new(dir.path());
    store.save_admission(&request()).unwrap();
    let mut tracker = Admissions::default();
    tracker.register(request());
    tracker.0.get_mut("op_test").unwrap().stopping = true;
    tracker.update(
        request(),
        Ok(record(
            "op_test",
            "admitted",
            "launching",
            Some("job_owned"),
        )),
        false,
        &store,
    );
    assert_eq!(tracker.0["op_test"].status, "cancelling");
    assert_eq!(store.admissions(), vec![request()]);
    assert!(matches!(
        tracker.poll().as_slice(),
        [Action::Admission { stop: true, .. }]
    ));
    tracker.update(request(), Err("Job still running".into()), true, &store);
    assert_eq!(tracker.0["op_test"].job.as_deref(), Some("job_owned"));
    assert_eq!(store.admissions(), vec![request()]);
    tracker.update(
        request(),
        Ok(record(
            "op_test",
            "admitted",
            "launching",
            Some("job_owned"),
        )),
        true,
        &store,
    );
    assert!(store.admissions().is_empty());
}
#[tokio::test]
async fn cancellation_checks_owned_job_terminal_acknowledgement() {
    for status in ["running", "cancelled"] {
        let reply: fn(&str) -> String = if status == "running" {
            |_| {
                json!({"data":{"id":"job_owned","session_id":"ses_1","child_id":"ses_child","status":"running"}}).to_string()
            }
        } else {
            |_| {
                json!({"data":{"id":"job_owned","session_id":"ses_1","child_id":"ses_child","status":"cancelled"}}).to_string()
            }
        };
        let server = Mock::new(vec![admitted, reply], None);
        let (_, result) = updated(perform(&server.client, request(), true).await);
        assert_eq!(result.is_ok(), status == "cancelled");
        let log = server.log.lock().unwrap();
        assert!(
            log[0]
                .0
                .starts_with("POST /api/v1/sessions/ses_1/delegations/op_test/stop ")
        );
        assert_eq!(log[1].0, "GET /api/v1/jobs/job_owned HTTP/1.1");
        assert!(!log.iter().any(|(path, _)| path.contains("interrupt")));
    }
}
#[tokio::test]
async fn foreign_job_ownership_refuses_cancellation_acknowledgement() {
    let server = Mock::new(
        vec![admitted, |_| {
            json!({"data":{"id":"job_owned","session_id":"ses_foreign","child_id":"ses_child","status":"cancelled"}}).to_string()
        }],
        None,
    );
    let (_, result) = updated(perform(&server.client, request(), true).await);
    assert!(result.unwrap_err().contains("ownership changed"));
    assert_eq!(server.log.lock().unwrap().len(), 2);
}
#[test]
fn foreign_records_and_launching_cancellation_cannot_acknowledge_no_dispatch() {
    for data in [
        record("op_foreign", "pending", "reserved", None),
        record("op_test", "cancelled", "launching", None),
        record("op_test", "admitted", "launching", None),
        json!({"id":"op_test","session_id":"ses_1","status":"pending","phase":"reserved","job_id":42}),
    ] {
        assert!(check(&request(), json!({"data":data})).is_err());
    }
}
#[tokio::test]
async fn failed_local_persistence_refuses_submission() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    std::fs::write(&path, "not a directory").unwrap();
    let store = LocalStore::new(&path);
    let server = Mock::new(vec![], None);
    let (tx, mut rx) = mpsc::channel(4);
    assert!(
        start(
            &server.client,
            "ses_1",
            "/repo",
            json!({"prompt":"test"}),
            Some(&Context {
                store: &store,
                tx: &tx
            })
        )
        .await
        .is_err()
    );
    assert!(server.log.lock().unwrap().is_empty());
    assert!(rx.try_recv().is_err());
}
#[tokio::test]
async fn pending_request_cancellation_uses_only_its_scoped_admission_route() {
    let server = Mock::new(vec![cancelled], None);
    let (_, result) = updated(perform(&server.client, request(), true).await);
    assert_eq!(result.unwrap()["status"], "cancelled");
    assert_eq!(
        server.log.lock().unwrap()[0].0,
        "POST /api/v1/sessions/ses_1/delegations/op_test/stop HTTP/1.1"
    );
}

#[tokio::test]
async fn cancellation_during_pending_post_survives_a_late_submission_response() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::new(dir.path());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let client = Client::http(&format!("http://{}", listener.local_addr().unwrap()), None);
    let server = thread::spawn(move || {
        let mut submission = accept_request(&listener);
        submission
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let (post, _) = read(&mut submission).unwrap();
        let path = post.split_whitespace().nth(1).unwrap();
        assert!(post.starts_with("POST /api/v1/sessions/ses_1/delegations/op_"));
        let id = path.rsplit('/').next().unwrap();
        let mut stop = accept_request(&listener);
        stop.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let (head, _) = read(&mut stop).unwrap();
        assert_eq!(head, format!("POST {path}/stop HTTP/1.1"));
        reply(&mut stop, &cancelled(id));
        reply(&mut submission, &pending(id));
    });
    let (tx, mut rx) = mpsc::channel(4);
    let (posted, stored) = (client.clone(), store.clone());
    let submission = tokio::spawn(async move {
        start(
            &posted,
            "ses_1",
            "/repo",
            json!({"prompt":"test"}),
            Some(&Context {
                store: &stored,
                tx: &tx,
            }),
        )
        .await
        .unwrap()
    });
    let Ok(Msg::AdmissionStarted(request)) = rx.recv().await.unwrap() else {
        panic!("Missing registration")
    };
    let mut tracker = Admissions::default();
    tracker.register(request.clone());
    tracker.0.get_mut(&request.id).unwrap().stopping = true;
    let (_, result) = updated(perform(&client, request.clone(), true).await);
    tracker.update(request.clone(), result, true, &store);
    assert_eq!(tracker.0[&request.id].status, "cancelled");
    let (_, result) = updated(submission.await.unwrap());
    tracker.update(request.clone(), result, false, &store);
    assert_eq!(tracker.0[&request.id].status, "cancelled");
    assert!(tracker.poll().is_empty());
    assert!(store.admissions().is_empty());
    server.join().unwrap();
}
fn reply(stream: &mut TcpStream, body: &str) {
    write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
}

#[test]
fn recorded_job_cannot_be_replaced_by_a_later_admission_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::new(dir.path());
    store.save_admission(&request()).unwrap();
    let mut tracker = Admissions::default();
    tracker.register(request());
    tracker.0.get_mut("op_test").unwrap().stopping = true;
    tracker.update(
        request(),
        Ok(record(
            "op_test",
            "admitted",
            "launching",
            Some("job_owned"),
        )),
        false,
        &store,
    );
    tracker.update(
        request(),
        Ok(record(
            "op_test",
            "admitted",
            "launching",
            Some("job_foreign"),
        )),
        true,
        &store,
    );
    assert_eq!(tracker.0["op_test"].status, "unknown");
    assert_eq!(tracker.0["op_test"].job.as_deref(), Some("job_owned"));
    assert_eq!(store.admissions(), vec![request()]);
}

#[tokio::test]
async fn cancellation_deadline_preserves_unknown_admission_for_reconnect() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::new(dir.path());
    store.save_admission(&request()).unwrap();
    let server = Mock::new(
        vec![|id| {
            thread::sleep(Duration::from_millis(3500));
            cancelled(id)
        }],
        None,
    );
    let began = std::time::Instant::now();
    let (_, result) = updated(perform(&server.client, request(), true).await);
    assert!(began.elapsed() < Duration::from_secs(5));
    assert!(result.as_ref().unwrap_err().contains("timed out"));
    let mut tracker = Admissions::default();
    tracker.register(request());
    tracker.update(request(), result, true, &store);
    assert_eq!(tracker.0["op_test"].status, "unknown");
    assert_eq!(store.admissions(), vec![request()]);
}

fn accept_request(listener: &TcpListener) -> TcpStream {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                return stream;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "API request did not arrive"
                );
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("{error}"),
        }
    }
}

#[test]
fn repeated_cancelling_replies_cannot_extend_the_overall_acknowledgement_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::new(dir.path());
    store.save_admission(&request()).unwrap();
    let mut tracker = Admissions::default();
    tracker.register(request());
    let entry = tracker.0.get_mut("op_test").unwrap();
    entry.stopping = true;
    entry.stop_at = Some(std::time::Instant::now() - Duration::from_secs(4));
    entry.busy = false;
    entry.status = "cancelling".into();
    assert!(tracker.poll().is_empty());
    assert_eq!(tracker.0["op_test"].status, "unknown");
    tracker.update(
        request(),
        Ok(record("op_test", "cancelling", "reserved", None)),
        true,
        &store,
    );
    assert_eq!(tracker.0["op_test"].status, "unknown");
    assert!(tracker.poll().is_empty());
    assert_eq!(store.admissions(), vec![request()]);
    tracker.update(
        request(),
        Ok(record("op_test", "cancelled", "reserved", None)),
        true,
        &store,
    );
    assert_eq!(tracker.0["op_test"].status, "cancelled");
    assert!(store.admissions().is_empty());
}

#[test]
fn uncertain_lookup_after_admission_restores_saved_recovery_identity() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::new(dir.path());
    store.save_admission(&request()).unwrap();
    let mut tracker = Admissions::default();
    tracker.register(request());
    tracker.update(
        request(),
        Ok(record(
            "op_test",
            "admitted",
            "launching",
            Some("job_owned"),
        )),
        false,
        &store,
    );
    assert!(store.admissions().is_empty());
    tracker.update(request(), Err("lookup response lost".into()), false, &store);
    assert_eq!(tracker.0["op_test"].job.as_deref(), Some("job_owned"));
    assert_eq!(store.admissions(), vec![request()]);
}
