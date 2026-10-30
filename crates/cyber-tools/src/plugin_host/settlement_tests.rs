//! Actual child-held pipe lifetime through the production host supervisors.
use super::*;
use crate::tools::process::{Process, ReadStream, WriteStream};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

struct Fixture {
    _root: tempfile::TempDir,
    started: launch::Started,
    input: WriteStream,
    output: BufReader<ReadStream>,
    live: Arc<Live>,
    host_input: ReadStream,
    host_output: WriteStream,
    leader: i32,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_proxy(None).await
    }

    async fn with_proxy(proxy: Option<cyber_sandbox::proxy::Proxy>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let scratch = root.path().join("scratch");
        std::fs::create_dir(&scratch).unwrap();
        // The leader exits. Its same-group child holds stdout open and waits for
        // the owner's stdin EOF, providing deterministic cleanup even in controls.
        let script = "import os\nleader=os.getpid()\nif os.fork() == 0:\n os.write(1,('child-ready '+str(leader)+'\\n').encode())\n while os.read(0,1): os.write(1,b'child-alive\\n')\n os._exit(0)\nos._exit(0)\n";
        let mut process = Process::spawn_with_stdin(
            "/usr/bin/python3",
            &["-c".into(), script.into()],
            None,
            |command| {
                command
                    .env_clear()
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .kill_on_drop(true);
            },
        )
        .await
        .unwrap();
        let input = process.stdin().unwrap();
        let mut output = BufReader::new(process.stdout().unwrap());
        let mut ready = String::new();
        tokio::time::timeout(Duration::from_secs(3), output.read_line(&mut ready))
            .await
            .unwrap()
            .unwrap();
        let leader: i32 = ready
            .trim()
            .strip_prefix("child-ready ")
            .unwrap()
            .parse()
            .unwrap();
        assert!(leader > 0);
        let (core_out, host_in) = tokio::io::duplex(4096);
        let (host_out, core_in) = tokio::io::duplex(4096);
        let handler: Handler = Arc::new(|_, _| Box::pin(async { Ok(Value::Null) }));
        let live = Arc::new(Live {
            connection: Connection::start(Box::new(core_out), Box::new(core_in), handler),
            tools: Vec::new(),
            started: Instant::now(),
        });
        Self {
            _root: root,
            started: launch::Started::new(
                process,
                launch::Scratch { path: scratch },
                proxy,
                "settlement-fixture",
            ),
            input,
            output,
            live,
            host_input: Box::new(host_in),
            host_output: Box::new(host_out),
            leader,
        }
    }
}

fn entry(location: PathBuf) -> Entry {
    Entry {
        name: "settlement-fixture".into(),
        digest: "fixture".into(),
        location,
        state: Mutex::new(State::Starting),
        stop: CancellationToken::new(),
        denials: Default::default(),
        crashes: Default::default(),
        settings: Mutex::default(),
        calls: Mutex::default(),
        task: Mutex::default(),
    }
}

/// Always close the controlled child's input before asserting so a missing-fix
/// test cannot leave a daemon behind. No PID inspection or reused-PID kill.
async fn assert_child_closed(mut output: BufReader<ReadStream>, input: WriteStream) {
    let mut remaining = Vec::new();
    let ended =
        tokio::time::timeout(Duration::from_secs(2), output.read_to_end(&mut remaining)).await;
    drop(input);
    if ended.is_err() {
        tokio::time::timeout(Duration::from_secs(3), output.read_to_end(&mut remaining))
            .await
            .unwrap()
            .unwrap();
    }
    assert!(
        matches!(ended, Ok(Ok(_))),
        "plugin descendant retained stdout after host settlement"
    );
}

#[tokio::test]
async fn crashed_host_stops_its_same_group_child_and_releases_scratch() {
    let f = Fixture::new().await;
    let scratch = f.started.scratch().to_path_buf();
    let e = entry(f._root.path().to_path_buf());
    let outcome = tokio::time::timeout(
        Duration::from_secs(3),
        supervise(&e, Arc::clone(&f.live), f.started),
    )
    .await
    .unwrap();
    assert_child_closed(f.output, f.input).await;
    f.live.connection.closed.cancel();
    assert!(outcome.is_some());
    assert!(!scratch.exists());
    // Keep the protocol pipes open until after supervision, so EOF on that
    // independent connection cannot select another cleanup branch.
    drop((f.host_input, f.host_output));
}

#[tokio::test]
async fn acknowledged_shutdown_stops_a_same_group_child_after_leader_exit() {
    let f = Fixture::new().await;
    let scratch = f.started.scratch().to_path_buf();
    let peer = tokio::spawn(async move {
        let mut input = BufReader::new(f.host_input);
        let mut request = String::new();
        input.read_line(&mut request).await.unwrap();
        let request: Value = serde_json::from_str(&request).unwrap();
        assert_eq!(request["method"], "shutdown");
        let response = json!({"jsonrpc":"2.0", "id":request["id"], "result":null});
        let mut output = f.host_output;
        output
            .write_all(format!("{response}\n").as_bytes())
            .await
            .unwrap();
    });
    tokio::time::timeout(Duration::from_secs(6), shutdown(&f.live, f.started))
        .await
        .unwrap();
    peer.await.unwrap();
    assert_child_closed(f.output, f.input).await;
    assert!(!scratch.exists());
}

async fn wait_removed(path: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn abandoned_initialization_keeps_scratch_until_cleanup_waits() {
    let f = Fixture::new().await;
    let scratch = f.started.scratch().to_path_buf();
    drop(f.started);
    // This is a current-thread runtime. No cleanup task can have been polled
    // between dropping the owner and recording this observation.
    let retained_before_wait = scratch.exists();
    assert_child_closed(f.output, f.input).await;
    wait_removed(&scratch).await;
    f.live.connection.closed.cancel();
    assert!(
        retained_before_wait,
        "scratch removed before cleanup waiter ran"
    );
}

#[tokio::test]
async fn cancelled_finish_does_not_cancel_owned_cleanup() {
    let f = Fixture::new().await;
    let scratch = f.started.scratch().to_path_buf();
    let mut finish = Box::pin(f.started.finish());
    assert!(futures::poll!(&mut finish).is_pending());
    drop(finish);
    assert_child_closed(f.output, f.input).await;
    wait_removed(&scratch).await;
    f.live.connection.closed.cancel();
}

#[allow(unsafe_code)]
fn reap_owned_leader(pid: i32) {
    loop {
        // SAFETY: this PID was captured by the fixed fixture before fork and
        // names this test's direct, still-unreaped child. No signal is sent.
        let result = unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) };
        if result == -1 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
        {
            continue;
        }
        assert_eq!(result, pid);
        break;
    }
}

async fn assert_alive_then_release(mut output: BufReader<ReadStream>, mut input: WriteStream) {
    let mut reply = String::new();
    let receipt = tokio::time::timeout(Duration::from_secs(3), async {
        input.write_all(b"p").await?;
        output.read_line(&mut reply).await
    })
    .await;
    drop(input);
    let mut remaining = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), output.read_to_end(&mut remaining))
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(receipt, Ok(Ok(_))) && reply == "child-alive\n",
        "unconfirmed wait signalled the cached process group"
    );
}

#[tokio::test]
async fn unconfirmed_wait_retains_resources_without_signalling_cached_group() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let signal = Arc::clone(&entered);
    let decide: cyber_sandbox::proxy::Decide = Arc::new(move |_| {
        let signal = Arc::clone(&signal);
        Box::pin(async move {
            signal.notify_one();
            std::future::pending::<bool>().await
        })
    });
    let proxy = cyber_sandbox::proxy::Proxy::start(decide).await.unwrap();
    let cyber_sandbox::proxy::Endpoint::Tcp(port) = proxy.endpoint else {
        unreachable!()
    };
    let mut client = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    client
        .write_all(b"CONNECT fixture.invalid:443 HTTP/1.1\r\n\r\n")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    let mut f = Fixture::with_proxy(Some(proxy)).await;
    let scratch = f.started.scratch().to_path_buf();
    reap_owned_leader(f.leader);
    assert!(f.started.wait().await.is_err());
    let result = f.started.finish().await;
    let retained = scratch.exists();
    let mut byte = [0];
    let closed = tokio::time::timeout(Duration::from_secs(3), client.read(&mut byte)).await;
    assert_alive_then_release(f.output, f.input).await;
    f.live.connection.closed.cancel();
    assert!(result.is_err());
    assert!(retained, "unconfirmed wait deleted host scratch");
    assert!(
        matches!(closed, Ok(Ok(0)))
            || matches!(closed, Ok(Err(ref error))
        if error.kind() == std::io::ErrorKind::ConnectionReset),
        "unconfirmed host retained network authority: {closed:?}"
    );
}

#[tokio::test]
async fn unconfirmed_supervision_stops_the_immediate_restart_loop() {
    let f = Fixture::new().await;
    let scratch = f.started.scratch().to_path_buf();
    let e = entry(f._root.path().to_path_buf());
    reap_owned_leader(f.leader);
    let outcome = tokio::time::timeout(
        Duration::from_secs(3),
        supervise(&e, Arc::clone(&f.live), f.started),
    )
    .await
    .unwrap();
    let failed = matches!(&*e.state.lock().unwrap_or_else(PoisonError::into_inner),
        State::Failed(reason) if reason.contains("cleanup is unconfirmed"));
    let retained = scratch.exists();
    assert_alive_then_release(f.output, f.input).await;
    assert!(
        outcome.is_none(),
        "unconfirmed supervision requested a crash restart"
    );
    assert!(
        failed && retained,
        "unconfirmed supervision did not preserve failure and resources"
    );
    assert!(f.live.connection.closed.is_cancelled());
}

#[tokio::test]
async fn aborting_cleanup_runtime_retains_unconfirmed_resources() {
    let f = Fixture::new().await;
    let scratch = f.started.scratch().to_path_buf();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            drop(f.started);
        });
        // Its queued cleanup was never polled. Dropping this runtime aborts it.
        drop(runtime);
    })
    .join()
    .unwrap();
    let retained = scratch.exists();
    assert_child_closed(f.output, f.input).await;
    f.live.connection.closed.cancel();
    assert!(retained, "aborted cleanup deleted unconfirmed host scratch");
}

#[tokio::test]
async fn dropping_owner_without_a_runtime_retains_unconfirmed_resources() {
    let f = Fixture::new().await;
    let scratch = f.started.scratch().to_path_buf();
    std::thread::spawn(move || drop(f.started)).join().unwrap();
    let retained = scratch.exists();
    assert_child_closed(f.output, f.input).await;
    f.live.connection.closed.cancel();
    assert!(retained, "missing cleanup runtime deleted host scratch");
}
