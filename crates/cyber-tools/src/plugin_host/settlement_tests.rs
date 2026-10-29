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
}

impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let scratch = root.path().join("scratch");
        std::fs::create_dir(&scratch).unwrap();
        // The leader exits. Its same-group child holds stdout open and waits for
        // the owner's stdin EOF, providing deterministic cleanup even in controls.
        let script = "import os\nif os.fork() == 0:\n os.write(1,b'child-ready\\n')\n os.read(0,1)\n os._exit(0)\nos._exit(0)\n";
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
        assert_eq!(ready, "child-ready\n");
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
            started: launch::Started {
                process,
                _scratch: launch::Scratch { path: scratch },
                proxy: None,
            },
            input,
            output,
            live,
            host_input: Box::new(host_in),
            host_output: Box::new(host_out),
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
    let scratch = f.started._scratch.path.clone();
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
    let scratch = f.started._scratch.path.clone();
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
