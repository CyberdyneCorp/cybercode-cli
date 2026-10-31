mod support;

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::Mutex;
#[cfg(any(unix, windows))]
use std::time::Duration;

use cyber_core::worktrees::{
    GitExecution, GitFuture, Managed, Name, Repository, Settings, SetupEvent, SetupSink,
};
#[cfg(unix)]
use cyber_core::worktrees::{RepositoryLock, SetupOutcome};
use serde_json::json;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

struct Git;
impl GitExecution for Git {
    fn run<'a>(&'a self, directory: &'a Path, args: &'a [OsString]) -> GitFuture<'a> {
        Box::pin(async move { invoke(directory, args) })
    }
}

fn invoke(directory: &Path, args: &[OsString]) -> io::Result<Output> {
    let mut command = Command::new("git");
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        );
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    command.current_dir(directory).args(args).output()
}

async fn owned() -> (support::Fixture, Repository, Managed) {
    let fixture = support::Fixture::new();
    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.name", "Worktree test"],
        vec!["config", "user.email", "test@example.invalid"],
    ] {
        assert!(
            invoke(
                &fixture.repo,
                &args.into_iter().map(OsString::from).collect::<Vec<_>>()
            )
            .unwrap()
            .status
            .success()
        );
    }
    fixture.write("tracked.txt", "base");
    for args in [
        vec!["add", "tracked.txt"],
        vec!["commit", "--quiet", "-m", "initial"],
    ] {
        assert!(
            invoke(
                &fixture.repo,
                &args.into_iter().map(OsString::from).collect::<Vec<_>>()
            )
            .unwrap()
            .status
            .success()
        );
    }
    let repository = Repository::discover(&Git, &fixture.repo).await.unwrap();
    let managed = repository
        .create(
            &Git,
            &Settings::default(),
            fixture.dir.path(),
            "prj_test",
            &Name::parse("setup").unwrap(),
        )
        .await
        .unwrap();
    (fixture, repository, managed)
}

#[cfg(all(unix, any(target_os = "macos", target_os = "linux")))]
#[tokio::test]
async fn lsp_native_shutdown_retains_managed_checkout_pins_until_acknowledgement() {
    use cyber_core::worktrees::CheckoutActivity;
    assert!(
        cyber_sandbox::available(),
        "native sandbox prerequisites are required"
    );
    let (fixture, repository, managed) = owned().await;
    let pool = managed_lsp_pool(&fixture, &managed.path);
    let handle = pool
        .ensure("fixture", &managed.path.join("tracked.txt"))
        .unwrap();
    assert_eq!(
        handle
            .request("ping", json!({}), Duration::from_secs(3))
            .await
            .unwrap(),
        true
    );
    let blocked = repository
        .remove(&Git, &CheckoutActivity, &managed, false)
        .await
        .unwrap_err();
    assert!(blocked.to_string().contains("in use by lsp_"));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), pool.close())
            .await
            .is_err()
    );
    assert!(
        repository
            .remove(&Git, &CheckoutActivity, &managed, false)
            .await
            .is_err()
    );
    assert!(pool.close().await.unwrap()[0].acknowledged);
    repository
        .remove(&Git, &CheckoutActivity, &managed, false)
        .await
        .unwrap();
    assert!(!managed.path.exists());
}

#[cfg(all(unix, any(target_os = "macos", target_os = "linux")))]
fn managed_lsp_pool(fixture: &support::Fixture, location: &Path) -> cyber_tools::lsp::Pool {
    use cyber_core::paths::Paths;
    use cyber_tools::lsp::{LaunchOptions, LocalLauncher};
    let root = fixture.dir.path().canonicalize().unwrap();
    let paths = Paths {
        config: root.join("config"),
        data: root.join("data"),
        cache: root.join("cache"),
        state: root.join("state"),
        tmp: root.join("lsp-tmp"),
    };
    paths.ensure().unwrap();
    let script = root.join("server.py");
    std::fs::write(&script, r#"
import sys,json,time
documents=[]
closed=[]
watched=[]
while True:
    length=None
    while True:
        line=sys.stdin.buffer.readline()
        if not line: sys.exit(0)
        if line==b'\r\n': break
        if line.lower().startswith(b'content-length:'): length=int(line.split(b':',1)[1])
    msg=json.loads(sys.stdin.buffer.read(length))
    if msg['method']=='textDocument/didOpen': documents.append(msg['params']['textDocument']['text'])
    if msg['method']=='textDocument/didClose': closed.append(msg['params']['textDocument']['uri'])
    if msg['method']=='workspace/didChangeWatchedFiles': watched.extend(msg['params']['changes'])
    if msg['method']=='shutdown': time.sleep(60)
    if msg['method']=='publish-diagnostics':
        body=json.dumps({'jsonrpc':'2.0','method':'textDocument/publishDiagnostics','params':msg['params']}).encode()
        sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n'%len(body)).encode()+body);sys.stdout.buffer.flush()
    if 'id' in msg:
        result={'capabilities':{}} if msg['method']=='initialize' else documents if msg['method']=='documents' else closed if msg['method']=='closed' else watched if msg['method']=='watched' else True
        body=json.dumps({'jsonrpc':'2.0','id':msg['id'],'result':result}).encode()
        sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n'%len(body)).encode()+body);sys.stdout.buffer.flush()
"#).unwrap();
    let config = json!({"lsp":{"fixture":{"command":["python3",script],"extensions":[".txt"]}},"sandbox":{"network":"off"}});
    fixture.set_config(config.clone());
    std::fs::write(paths.config.join("cyber.jsonc"), config.to_string()).unwrap();
    let launcher = Arc::new(
        LocalLauncher::new(
            location,
            LaunchOptions {
                checkout_claim: Some(fixture.host.lsp_checkout_claim()),
                paths,
                home: root.join("home"),
                environment: std::env::vars().collect(),
                profile: None,
                overrides: vec![],
                flags: json!({}),
                sandbox_policy: None,
                helper: cyber_sandbox::find_helper(),
                credential_env_names: vec![],
            },
        )
        .unwrap(),
    );
    launcher.pool().unwrap()
}

#[cfg(all(unix, any(target_os = "macos", target_os = "linux")))]
#[tokio::test]
async fn lsp_idle_waits_for_runtime_tool_activity_and_a_fresh_release_window() {
    use cyber_server::runtime::{CreateSession, NoSnapshots};
    use cyber_tools::lsp::{Locations, ServerState};
    assert!(
        cyber_sandbox::available(),
        "native sandbox prerequisites are required"
    );
    let (fixture, _, managed) = owned().await;
    let pool = managed_lsp_pool(&fixture, &managed.path);
    let locations = Locations::with_idle(
        Arc::new(move |_| Ok(pool.clone())),
        Duration::from_millis(250),
    );
    fixture
        .host
        .attach_lsp_locations(locations.clone())
        .unwrap();
    let bootstrap = locations.acquire(&managed.path).unwrap();
    let flow = support::flow::Flow::with(
        fixture,
        vec![
            support::flow::call(
                "busy",
                "bash",
                json!({"command":"printf busy > busy-marker; sleep 2; printf done","timeout":5000}),
            ),
            support::flow::text("finished"),
        ],
        false,
        Arc::new(NoSnapshots),
    );
    let info = flow
        .runtime
        .create_session(CreateSession {
            directory: managed.path.display().to_string(),
            model: "test/main".into(),
            mode: Some("bypass".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    locations
        .warm(
            &managed.path,
            managed.path.join("tracked.txt"),
            "snapshot".into(),
        )
        .unwrap();
    flow.prompt(&info.id, "run the waiting tool").await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while !managed.path.join("busy-marker").exists()
            || !locations
                .status(&managed.path)
                .unwrap()
                .first()
                .is_some_and(|row| row.status == ServerState::Connected)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(bootstrap);
    tokio::time::sleep(Duration::from_millis(650)).await;
    assert_eq!(
        locations.status(&managed.path).unwrap()[0].status,
        ServerState::Connected
    );
    flow.settle(&info.id).await;
    assert!(flow.output(&info.id, "busy").await.contains("done"));
    assert_eq!(
        locations.status(&managed.path).unwrap()[0].status,
        ServerState::Connected
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        while locations.status(&managed.path).unwrap()[0].status != ServerState::Broken {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(locations.close().await.unwrap()[0].acknowledged);
    flow.runtime.shutdown().await;
}

#[cfg(all(unix, any(target_os = "macos", target_os = "linux")))]
#[tokio::test]
async fn lsp_idle_retains_activity_through_native_location_commit_proof() {
    use cyber_server::runtime::{CreateSession, NoSnapshots, ToolHost};
    use cyber_tools::lsp::{Locations, ServerState};
    assert!(
        cyber_sandbox::available(),
        "native sandbox prerequisites are required"
    );
    let (fixture, _, managed) = owned().await;
    let pool = managed_lsp_pool(&fixture, &managed.path);
    let locations = Locations::with_idle(
        Arc::new(move |_| Ok(pool.clone())),
        Duration::from_millis(250),
    );
    fixture
        .host
        .attach_lsp_locations(locations.clone())
        .unwrap();
    let flow = support::flow::Flow::with(fixture, vec![], false, Arc::new(NoSnapshots));
    let info = flow
        .runtime
        .create_session(CreateSession {
            directory: managed.path.display().to_string(),
            model: "test/main".into(),
            mode: Some("bypass".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let lease = flow
        .f
        .host
        .claim_location(&info, false, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(lease.worktree_id.as_deref(), Some(managed.id.as_str()));
    let proof = lease.settle_retained().unwrap();
    locations
        .warm(
            &managed.path,
            managed.path.join("tracked.txt"),
            "snapshot".into(),
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !locations
            .status(&managed.path)
            .unwrap()
            .first()
            .is_some_and(|row| row.status == ServerState::Connected)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(650)).await;
    assert_eq!(
        locations.status(&managed.path).unwrap()[0].status,
        ServerState::Connected
    );
    drop(proof);
    assert_eq!(
        locations.status(&managed.path).unwrap()[0].status,
        ServerState::Connected
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        while locations.status(&managed.path).unwrap()[0].status != ServerState::Broken {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(locations.close().await.unwrap()[0].acknowledged);
    flow.runtime.shutdown().await;
}

#[derive(Default)]
struct Sink {
    output: Mutex<Vec<u8>>,
    ready: Notify,
    fail: bool,
}

#[cfg(all(unix, any(target_os = "macos", target_os = "linux")))]
#[tokio::test]
async fn lsp_document_below_existing_root_claims_and_retains_its_checkout() {
    use cyber_core::worktrees::CheckoutActivity;
    assert!(
        cyber_sandbox::available(),
        "native sandbox prerequisites are required"
    );
    let (fixture, repository, managed) = owned().await;
    let location = fixture.dir.path().canonicalize().unwrap();
    let nested = repository
        .create(
            &Git,
            &Settings::default(),
            &managed.path.join("nested-data"),
            "prj_nested",
            &Name::parse("nested").unwrap(),
        )
        .await
        .unwrap();
    assert!(Repository::managed_at(&location).unwrap().is_none());
    let outer_file = location.join("outer.txt");
    std::fs::write(&outer_file, "outer").unwrap();
    let pool = managed_lsp_pool(&fixture, &location);
    let handle = pool.ensure("fixture", &outer_file).unwrap();
    handle
        .request("ping", json!({}), Duration::from_secs(3))
        .await
        .unwrap();
    assert_eq!(handle.status().root, location);
    let file = nested.path.join("tracked.txt");
    assert_eq!(
        Repository::managed_locations_at(file.parent().unwrap())
            .unwrap()
            .len(),
        2
    );
    handle
        .open_document(&file, "nested-checkout".into())
        .await
        .unwrap();
    handle
        .open_document(&file, "nested-checkout".into())
        .await
        .unwrap();
    assert_eq!(
        handle
            .request("documents", json!({}), Duration::from_secs(3))
            .await
            .unwrap(),
        json!(["nested-checkout"])
    );
    for index in 0..128 {
        let other = location.join(format!("zz-{index:03}.txt"));
        std::fs::write(&other, "outside-checkouts").unwrap();
        handle
            .open_document(&other, "outside-checkouts".into())
            .await
            .unwrap();
    }
    assert_eq!(
        handle
            .request("closed", json!({}), Duration::from_secs(3))
            .await
            .unwrap(),
        json!([reqwest::Url::from_file_path(&file).unwrap().to_string()])
    );
    for checkout in [&nested, &managed] {
        let error = repository
            .remove(&Git, &CheckoutActivity, checkout, false)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("in use by lsp_"), "{error}");
    }
    let record = nested
        .common_dir
        .join("cyber-worktrees")
        .join("nested.json");
    let original = std::fs::read(&record).unwrap();
    let mut changed = nested.clone();
    changed.id = cyber_core::ids::new_id("wt");
    std::fs::write(&record, serde_json::to_vec(&changed).unwrap()).unwrap();
    let rejected = handle
        .open_document(&file, "replacement-snapshot".into())
        .await;
    std::fs::write(&record, original).unwrap();
    assert!(rejected.is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(20), pool.close())
            .await
            .is_err()
    );
    assert!(
        repository
            .remove(&Git, &CheckoutActivity, &managed, false)
            .await
            .is_err()
    );
    assert!(pool.close().await.unwrap()[0].acknowledged);
    repository
        .remove(&Git, &CheckoutActivity, &nested, false)
        .await
        .unwrap();
    repository
        .remove(&Git, &CheckoutActivity, &managed, false)
        .await
        .unwrap();
    assert!(!managed.path.exists());
}

#[tokio::test]
async fn lsp_background_snapshot_cannot_start_against_a_recreated_checkout() {
    assert_recreated_lsp_origin(false).await;
}

#[cfg(all(unix, any(target_os = "macos", target_os = "linux")))]
#[tokio::test]
async fn lsp_unopened_diagnostic_file_claims_all_enclosing_managed_scopes() {
    use cyber_core::worktrees::CheckoutActivity;
    assert!(
        cyber_sandbox::available(),
        "native sandbox prerequisites are required"
    );
    let (fixture, repository, managed) = owned().await;
    let nested = repository
        .create(
            &Git,
            &Settings::default(),
            &managed.path.join("nested-data"),
            "prj_nested",
            &Name::parse("nested").unwrap(),
        )
        .await
        .unwrap();
    let location = fixture.dir.path().canonicalize().unwrap();
    let outer = location.join("outer.txt");
    std::fs::write(&outer, "outer").unwrap();
    let pool = managed_lsp_pool(&fixture, &location);
    let handle = pool.ensure("fixture", &outer).unwrap();
    let file = nested.path.join("tracked.txt").canonicalize().unwrap();
    handle
        .request(
            "publish-diagnostics",
            json!({"uri":reqwest::Url::from_file_path(&file).unwrap().as_str(),"diagnostics":[]}),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    let snapshot = handle.diagnostics(&file).await.unwrap().unwrap();
    assert_eq!(snapshot.document_version, None);
    assert_eq!(
        handle
            .request("documents", json!({}), Duration::from_secs(3))
            .await
            .unwrap(),
        json!([])
    );
    for checkout in [&nested, &managed] {
        let error = repository
            .remove(&Git, &CheckoutActivity, checkout, false)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("in use by lsp_"), "{error}");
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(20), pool.close())
            .await
            .is_err()
    );
    assert!(
        repository
            .remove(&Git, &CheckoutActivity, &managed, false)
            .await
            .is_err()
    );
    assert!(pool.close().await.unwrap()[0].acknowledged);
    repository
        .remove(&Git, &CheckoutActivity, &nested, false)
        .await
        .unwrap();
    repository
        .remove(&Git, &CheckoutActivity, &managed, false)
        .await
        .unwrap();
}

#[tokio::test]
async fn lsp_background_document_cannot_start_against_a_recreated_nested_checkout() {
    assert_recreated_lsp_origin(true).await;
}

async fn assert_recreated_lsp_origin(nested: bool) {
    use cyber_core::intelligence::{DetectedServer, InstallMethod, ServerDefinition};
    use cyber_core::worktrees::CheckoutActivity;
    use cyber_tools::lsp::{LaunchError, Locations, LspError, Pool};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let (fixture, repository, managed) = owned().await;
    let file = managed.path.join("tracked.txt");
    let location = if nested {
        fixture.dir.path().canonicalize().unwrap()
    } else {
        managed.path.clone()
    };
    let (release, blocked) = std::sync::mpsc::channel();
    let blocked = Mutex::new(blocked);
    let started = Arc::new(AtomicBool::new(false));
    let completed = Arc::new(AtomicBool::new(false));
    let launches = Arc::new(AtomicUsize::new(0));
    let observed_started = started.clone();
    let observed_completed = completed.clone();
    let observed_launches = launches.clone();
    let locations = Locations::new(Arc::new(move |directory| {
        observed_started.store(true, Ordering::Release);
        blocked
            .lock()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let launches = observed_launches.clone();
        let pool = Pool::new(
            directory,
            vec![DetectedServer {
                definition: ServerDefinition {
                    id: "fixture".into(),
                    command: vec!["fixture".into()],
                    extensions: vec![".txt".into()],
                    root_markers: vec![],
                    env: Default::default(),
                    initialization_options: None,
                    install: InstallMethod::Custom,
                },
                enabled: true,
                installed: true,
                executable: Some("/fixture".into()),
            }],
            Arc::new(move |_, _| {
                let launches = launches.clone();
                Box::pin(async move {
                    launches.fetch_add(1, Ordering::SeqCst);
                    Err(LaunchError {
                        error: LspError::Protocol("unexpected stale launch"),
                        acknowledged: true,
                    })
                })
            }),
        );
        observed_completed.store(true, Ordering::Release);
        pool
    }));
    locations
        .warm(&location, file, "private-original-read".into())
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !started.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    repository
        .remove(&Git, &CheckoutActivity, &managed, false)
        .await
        .unwrap();
    let replacement = repository
        .create(
            &Git,
            &Settings::default(),
            fixture.dir.path(),
            "prj_test",
            &Name::parse("setup").unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(managed.path, replacement.path);
    assert_ne!(managed.id, replacement.id);
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !completed.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(locations.status(&location).unwrap().is_empty());
    assert_eq!(launches.load(Ordering::SeqCst), 0);
    locations.close().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn original_setup_authority_refuses_a_second_command_after_generation_fence() {
    use cyber_server::runtime::{CreateSession, NoSnapshots};
    use cyber_store::{Expected, NewEvent};
    use std::sync::atomic::{AtomicBool, Ordering};
    struct FenceSink {
        store: Arc<cyber_store::Store>,
        session: String,
        output: Mutex<Vec<u8>>,
        fenced: AtomicBool,
    }
    impl SetupSink for FenceSink {
        fn emit(&self, event: SetupEvent<'_>) -> io::Result<()> {
            if let SetupEvent::Output { bytes, .. } = event {
                let mut output = self.output.lock().unwrap();
                output.extend_from_slice(bytes);
                if output.windows(9).any(|bytes| bytes == b"fence-now")
                    && !self.fenced.swap(true, Ordering::SeqCst)
                {
                    self.store
                        .append(
                            &self.session,
                            Expected::Any,
                            vec![NewEvent::new("session.admission.fenced.1", json!({}))],
                        )
                        .map_err(io::Error::other)?;
                }
            }
            Ok(())
        }
    }
    let (fixture, repository, managed) = owned().await;
    let commands = vec![
        "printf fence-now".to_string(),
        "printf forbidden > second.txt".to_string(),
    ];
    fixture.set_config(json!({"permissions":{"worktree":"allow"}, "worktrees":{"setup":commands}}));
    let flow = support::flow::Flow::with(fixture, vec![], false, Arc::new(NoSnapshots));
    let info = flow
        .runtime
        .create_session(CreateSession {
            directory: managed.path.display().to_string(),
            model: "test/main".into(),
            mode: Some("bypass".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut inv = flow.f.invocation("bypass", "worktree", json!({}));
    inv.session_id = info.id.clone();
    inv.directory = info.directory;
    let sink = FenceSink {
        store: flow.f.store.clone(),
        session: info.id.clone(),
        output: Mutex::default(),
        fenced: AtomicBool::new(false),
    };
    let error = flow
        .f
        .host
        .setup_worktree(&inv, CancellationToken::new(), &repository, &managed, &sink)
        .await
        .unwrap_err();
    assert!(sink.fenced.load(Ordering::SeqCst));
    assert!(error.to_string().contains("fenced"), "{error}");
    assert!(!managed.path.join("second.txt").exists());
    let journal = cyber_server::worktrees::SetupJournal::new(
        flow.f.store.clone(),
        &managed,
        &commands,
        &info.id,
    )
    .unwrap();
    assert!(matches!(
        journal.start(1).unwrap(),
        cyber_server::worktrees::CommandDecision::Recorded(
            cyber_server::worktrees::CommandResult::NotDispatched { .. }
        )
    ));
    flow.runtime.shutdown().await;
}
impl SetupSink for Sink {
    fn emit(&self, event: SetupEvent<'_>) -> io::Result<()> {
        if let SetupEvent::Output { bytes, .. } = event {
            let mut output = self.output.lock().unwrap();
            output.extend_from_slice(bytes);
            if output.windows(5).any(|part| part == b"ready") {
                self.ready.notify_one();
            }
            if self.fail {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "Session output unavailable",
                ));
            }
        }
        Ok(())
    }
}

#[tokio::test]
async fn setup_requires_session_location_and_worktree_permission() {
    let (fixture, repository, managed) = owned().await;
    fixture.set_config(
        json!({"permissions": {"worktree": "deny"}, "worktrees": {"setup": ["must not run"]}}),
    );
    let mut inv = fixture.invocation("bypass", "worktree", json!({}));
    let sink = Sink::default();
    let error = fixture
        .host
        .setup_worktree(&inv, CancellationToken::new(), &repository, &managed, &sink)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not located"));
    inv.directory = managed.path.display().to_string();
    let error = fixture
        .host
        .setup_worktree(&inv, CancellationToken::new(), &repository, &managed, &sink)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Permission denied"));
    assert!(sink.output.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn setup_runs_in_sandbox_streams_and_preserves_failed_worktree() {
    let (fixture, repository, managed) = owned().await;
    let outside = fixture.dir.path().join("outside");
    std::fs::write(&outside, "protected").unwrap();
    // The output explicitly distinguishes an enforced write denial from shell failure.
    let source = format!(
        "printf ready; printf diagnostic >&2; printf changed >> '{}'; if printf forbidden > '{}' 2>/dev/null; then exit 9; fi; printf '%s' \"${{HOME-unset}}\"; exit 7",
        managed.path.join("result").display(),
        outside.display()
    );
    fixture.set_config(json!({"permissions": {"worktree": "allow"}, "providers": {"local": {"env": ["HOME"]}}, "worktrees": {"setup": [source, "printf wrong > unexpected"]}}));
    let mut inv = fixture.invocation("bypass", "worktree", json!({}));
    inv.directory = managed.path.display().to_string();
    let sink = Sink::default();
    let outcome = fixture
        .host
        .setup_worktree(&inv, CancellationToken::new(), &repository, &managed, &sink)
        .await
        .unwrap();
    assert_eq!(
        outcome,
        SetupOutcome::Failed {
            index: 0,
            code: Some(7)
        }
    );
    let output = String::from_utf8_lossy(&sink.output.lock().unwrap()).into_owned();
    assert!(
        output.contains("ready") && output.contains("diagnostic") && output.contains("unset"),
        "{output}"
    );
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "protected");
    assert_eq!(
        std::fs::read_to_string(managed.path.join("result")).unwrap(),
        "changed"
    );
    assert!(!managed.path.join("unexpected").exists());
    assert!(managed.path.join("tracked.txt").exists());
    inv.session_id = "ses_repeat".into();
    let repeated = Sink::default();
    assert_eq!(
        fixture
            .host
            .setup_worktree(
                &inv,
                CancellationToken::new(),
                &repository,
                &managed,
                &repeated
            )
            .await
            .unwrap(),
        outcome
    );
    assert!(repeated.output.lock().unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(managed.path.join("result")).unwrap(),
        "changed"
    );
}

#[cfg(unix)]
const TREE: &str = "(while :; do printf tick >> heartbeat; sleep 0.05; done) & while [ ! -s heartbeat ]; do sleep 0.01; done; printf ready; ";

#[cfg(unix)]
async fn assert_stopped(managed: &Managed, repository: &Repository) {
    assert!(
        RepositoryLock::try_acquire(&repository.common_dir)
            .unwrap()
            .is_some()
    );
    let path = managed.path.join("heartbeat");
    let before = std::fs::read(&path).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        std::fs::read(path).unwrap(),
        before,
        "descendant continued after settlement"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn setup_cancellation_and_future_disposal_stop_live_descendants() {
    for abort in [false, true] {
        let (fixture, repository, managed) = owned().await;
        fixture.set_config(json!({"permissions": {"worktree": "allow"}, "worktrees": {"setup": [format!("{TREE}sleep 30")]}}));
        let mut inv = fixture.invocation("bypass", "worktree", json!({}));
        inv.directory = managed.path.display().to_string();
        let sink = Arc::new(Sink::default());
        let host = Arc::clone(&fixture.host);
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let owned_repository = repository.clone();
        let owned_managed = managed.clone();
        let output = Arc::clone(&sink);
        let job = tokio::spawn(async move {
            host.setup_worktree(&inv, token, &owned_repository, &owned_managed, &*output)
                .await
        });
        tokio::time::timeout(Duration::from_secs(10), sink.ready.notified())
            .await
            .unwrap();
        if abort {
            job.abort();
            assert!(job.await.unwrap_err().is_cancelled());
        } else {
            cancel.cancel();
            let error = tokio::time::timeout(Duration::from_secs(2), job)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        }
        assert_stopped(&managed, &repository).await;
        let mut repeated = fixture.invocation("bypass", "worktree", json!({}));
        repeated.directory = managed.path.display().to_string();
        let replay = fixture
            .host
            .setup_worktree(
                &repeated,
                CancellationToken::new(),
                &repository,
                &managed,
                &Sink::default(),
            )
            .await
            .unwrap_err();
        assert!(replay.to_string().contains(if abort {
            "outcome unknown"
        } else {
            "Setup cancelled"
        }));
        assert_stopped(&managed, &repository).await;
        fixture
            .set_config(json!({"permissions": {"worktree": "allow"}, "worktrees": {"setup": []}}));
        assert!(
            fixture
                .host
                .setup_worktree(
                    &repeated,
                    CancellationToken::new(),
                    &repository,
                    &managed,
                    &Sink::default()
                )
                .await
                .unwrap_err()
                .to_string()
                .contains("configuration or ownership changed")
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn normal_setup_exit_stops_pipe_holding_descendants_and_preserves_exit_code() {
    let (fixture, repository, managed) = owned().await;
    fixture.set_config(json!({"permissions": {"worktree": "allow"}, "worktrees": {"setup": [format!("{TREE}exit 7")]}}));
    let mut inv = fixture.invocation("bypass", "worktree", json!({}));
    inv.directory = managed.path.display().to_string();
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        fixture.host.setup_worktree(
            &inv,
            CancellationToken::new(),
            &repository,
            &managed,
            &Sink::default(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        outcome,
        SetupOutcome::Failed {
            index: 0,
            code: Some(7)
        }
    );
    assert_stopped(&managed, &repository).await;
}

#[cfg(unix)]
#[tokio::test]
async fn output_delivery_failure_stops_the_setup_tree() {
    let (fixture, repository, managed) = owned().await;
    fixture.set_config(json!({"permissions": {"worktree": "allow"}, "worktrees": {"setup": [format!("{TREE}sleep 30")]}}));
    let mut inv = fixture.invocation("bypass", "worktree", json!({}));
    inv.directory = managed.path.display().to_string();
    let sink = Sink {
        fail: true,
        ..Sink::default()
    };
    let error = tokio::time::timeout(
        Duration::from_secs(5),
        fixture
            .host
            .setup_worktree(&inv, CancellationToken::new(), &repository, &managed, &sink),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert_stopped(&managed, &repository).await;
}

#[cfg(unix)]
#[tokio::test]
async fn attached_session_receives_live_output_and_shutdown_stops_setup() {
    use base64::Engine as _;
    use cyber_server::runtime::{CreateSession, LiveEvent, NoSnapshots, SetupChannel, SetupUpdate};
    for shutdown in [false, true] {
        let (fixture, repository, managed) = owned().await;
        let source = format!("{TREE}printf diagnostic >&2; sleep 30");
        fixture.set_config(
            json!({"permissions": {"worktree": "allow"}, "worktrees": {"setup": [source.clone()]}}),
        );
        let flow = support::flow::Flow::with(fixture, vec![], false, Arc::new(NoSnapshots));
        let info = flow
            .runtime
            .create_session(CreateSession {
                directory: managed.path.display().to_string(),
                model: "test/main".into(),
                mode: Some("bypass".into()),
                ..CreateSession::default()
            })
            .await
            .unwrap();
        let mut inv = flow.f.invocation("bypass", "worktree", json!({}));
        inv.directory = info.directory;
        inv.session_id = info.id.clone();
        let host = Arc::clone(&flow.f.host);
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let owned_repository = repository.clone();
        let owned_managed = managed.clone();
        let mut events = flow.runtime.subscribe();
        let job = tokio::spawn(async move {
            host.setup_worktree_session(&inv, token, &owned_repository, &owned_managed)
                .await
        });
        let received = async {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            while !stdout.windows(5).any(|part| part == b"ready")
                || !stderr.windows(10).any(|part| part == b"diagnostic")
            {
                if let LiveEvent::WorktreeSetup {
                    session_id,
                    worktree_id,
                    update: SetupUpdate::Output { stream, base64, .. },
                    ..
                } = events.recv().await.unwrap()
                {
                    assert_eq!(session_id, info.id);
                    assert_eq!(worktree_id, managed.id);
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(base64)
                        .unwrap();
                    match stream {
                        SetupChannel::Stdout => stdout.extend(bytes),
                        SetupChannel::Stderr => stderr.extend(bytes),
                    }
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(10), received)
            .await
            .unwrap();
        assert!(
            !job.is_finished(),
            "output arrived only after command completion"
        );
        if shutdown {
            flow.runtime.shutdown().await;
        } else {
            cancel.cancel();
        }
        let error = tokio::time::timeout(Duration::from_secs(2), job)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_stopped(&managed, &repository).await;
        repository
            .claim(&Git, &managed, &info.id)
            .await
            .unwrap()
            .settle()
            .unwrap();
        if shutdown {
            let journal = cyber_server::worktrees::SetupJournal::new(
                Arc::clone(&flow.f.store),
                &managed,
                &[source],
                &info.id,
            )
            .unwrap();
            assert!(matches!(
                journal.start(0).unwrap(),
                cyber_server::worktrees::CommandDecision::Recorded(
                    cyber_server::worktrees::CommandResult::Failed { .. }
                )
            ));
        } else {
            assert!(matches!(
                events.recv().await.unwrap(),
                LiveEvent::WorktreeSetup {
                    update: SetupUpdate::Failed { index: Some(0), .. },
                    ..
                }
            ));
        }
    }
}

fn creation_request(fixture: &support::Fixture, name: &str) -> cyber_tools::WorktreeSessionRequest {
    cyber_tools::WorktreeSessionRequest {
        session: cyber_server::runtime::CreateSession {
            model: "test/main".into(),
            mode: Some("bypass".into()),
            ..Default::default()
        },
        data: fixture.dir.path().canonicalize().unwrap(),
        project_id: "prj_test".into(),
        name: Name::parse(name).unwrap(),
    }
}

async fn creation_source(flow: &support::flow::Flow) -> cyber_server::runtime::Invocation {
    let source = flow
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            mode: Some("bypass".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut inv = flow.f.invocation("bypass", "worktree", json!({}));
    inv.session_id = source.id;
    inv
}

#[tokio::test]
async fn drain_lease_is_held_during_tools_and_settled_after_completion() {
    use cyber_core::worktrees::CheckoutActivity;
    use cyber_server::runtime::{CreateSession, NoSnapshots};
    use support::flow::{call, text};
    let (fixture, repository, managed) = owned().await;
    #[cfg(windows)]
    fixture.set_config(json!({"sandbox":{"policy":"full-access"}}));
    let nested = repository
        .create(
            &Git,
            &Settings {
                root: Some(managed.path.join("checkouts")),
                ..Default::default()
            },
            fixture.dir.path(),
            "prj_test",
            &Name::parse("nested").unwrap(),
        )
        .await
        .unwrap();
    let request = json!({"questions": [{"question": "Hold checkout", "header": "Lease", "options": [{"label": "done"}, {"label": "keep waiting"}]}]});
    let flow = support::flow::Flow::with(
        fixture,
        vec![call("lease_question", "question", request), text("done")],
        true,
        Arc::new(NoSnapshots),
    );
    let info = flow
        .runtime
        .create_session(CreateSession {
            directory: nested.path.display().to_string(),
            model: "test/main".into(),
            mode: Some("bypass".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(info.worktree_id.as_deref(), Some(nested.id.as_str()));
    flow.prompt(&info.id, "wait").await;
    let pending = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(request) = flow
                .runtime
                .pending_requests(Some(&info.id))
                .into_iter()
                .next()
            {
                break request;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let error = repository
        .remove(&Git, &CheckoutActivity, &managed, true)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains(&format!("in use by {}", info.id))
    );
    assert!(
        repository
            .remove(&Git, &CheckoutActivity, &nested, true)
            .await
            .unwrap_err()
            .to_string()
            .contains(&info.id)
    );
    flow.runtime
        .answer_question(
            &pending.id,
            cyber_server::runtime::QuestionReply::Answers {
                answers: vec![vec!["done".into()]],
            },
        )
        .await
        .unwrap();
    flow.settle(&info.id).await;
    assert_eq!(
        flow.runtime.state(&info.id).await.unwrap().info.worktree_id,
        info.worktree_id
    );
    repository
        .remove(&Git, &CheckoutActivity, &nested, false)
        .await
        .unwrap();
    repository
        .remove(&Git, &CheckoutActivity, &managed, false)
        .await
        .unwrap();
}

#[tokio::test]
async fn old_session_refuses_recreated_checkout_at_the_same_path_before_model_access() {
    use cyber_core::worktrees::CheckoutActivity;
    use cyber_server::runtime::{CreateSession, LiveEvent, NoSnapshots};
    let (fixture, repository, managed) = owned().await;
    #[cfg(windows)]
    fixture.set_config(json!({"sandbox":{"policy":"full-access"}}));
    let flow = support::flow::Flow::with(
        fixture,
        vec![support::flow::text("must not run")],
        false,
        Arc::new(NoSnapshots),
    );
    let info = flow
        .runtime
        .create_session(CreateSession {
            directory: managed.path.display().to_string(),
            model: "test/main".into(),
            mode: Some("bypass".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut historical = info.clone();
    historical.worktree_id = None;
    use cyber_server::runtime::ToolHost;
    assert!(
        flow.f
            .host
            .claim_location(&historical, false, CancellationToken::new())
            .await
            .is_err()
    );
    repository
        .remove(&Git, &CheckoutActivity, &managed, false)
        .await
        .unwrap();
    let replacement = repository
        .create(
            &Git,
            &Settings::default(),
            flow.f.dir.path(),
            "prj_test",
            &Name::parse("setup").unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(replacement.id, managed.id);
    let mut events = flow.runtime.subscribe();
    flow.prompt(&info.id, "run").await;
    flow.settle(&info.id).await;
    assert!(flow.main.requests().is_empty());
    let message = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let LiveEvent::Error { message, .. } = events.recv().await.unwrap() {
                break message;
            }
        }
    })
    .await
    .unwrap();
    assert!(message.contains("creation identity changed"));
    assert!(replacement.path.join("tracked.txt").exists());
}

#[cfg(windows)]
#[tokio::test]
async fn managed_session_refuses_unavailable_windows_enforcement_without_explicit_opt_out() {
    use cyber_server::runtime::{CreateSession, NoSnapshots};
    if cyber_sandbox::available() {
        return;
    }
    let (fixture, _, managed) = owned().await;
    let flow = support::flow::Flow::with(fixture, vec![], false, Arc::new(NoSnapshots));
    let error = flow
        .runtime
        .create_session(CreateSession {
            directory: managed.path.display().to_string(),
            model: "test/main".into(),
            mode: Some("bypass".into()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("SandboxUnavailableError"));
    assert!(
        flow.runtime
            .list(&Default::default())
            .unwrap()
            .sessions
            .is_empty()
    );
    assert!(flow.main.requests().is_empty());
    assert!(managed.path.join("tracked.txt").exists());
    assert!(!managed.common_dir.join("cyber-worktree-activity").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn idle_user_shell_fences_checkout_and_acknowledges_shutdown_after_descendant_cleanup() {
    use cyber_core::worktrees::CheckoutActivity;
    use cyber_server::runtime::{CreateSession, NoSnapshots};
    let (fixture, repository, managed) = owned().await;
    let flow = support::flow::Flow::with(fixture, vec![], false, Arc::new(NoSnapshots));
    let info = flow
        .runtime
        .create_session(CreateSession {
            directory: managed.path.display().to_string(),
            model: "test/main".into(),
            mode: Some("bypass".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let runtime = flow.runtime.clone();
    let id = info.id.clone();
    let job = tokio::spawn(async move { runtime.shell(&id, &format!("{TREE}sleep 30")).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !std::fs::metadata(managed.path.join("heartbeat"))
            .is_ok_and(|metadata| metadata.len() > 0)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let error = repository
        .remove(&Git, &CheckoutActivity, &managed, true)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains(&format!("in use by {}", info.id))
    );
    tokio::time::timeout(Duration::from_secs(5), flow.runtime.shutdown())
        .await
        .unwrap();
    assert!(job.await.unwrap().is_err());
    assert_stopped(&managed, &repository).await;
    repository
        .remove(&Git, &CheckoutActivity, &managed, true)
        .await
        .unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn disposed_session_setup_retains_activity_after_stopping_descendants() {
    use cyber_core::worktrees::CheckoutActivity;
    use cyber_server::runtime::{CreateSession, NoSnapshots};
    let (fixture, repository, managed) = owned().await;
    fixture.set_config(json!({"permissions":{"worktree":"allow"},"worktrees":{"setup":[format!("{TREE}sleep 30")]}}));
    let flow = support::flow::Flow::with(fixture, vec![], false, Arc::new(NoSnapshots));
    let info = flow
        .runtime
        .create_session(CreateSession {
            directory: managed.path.display().to_string(),
            model: "test/main".into(),
            mode: Some("bypass".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut inv = flow.f.invocation("bypass", "worktree", json!({}));
    inv.session_id = info.id.clone();
    inv.directory = info.directory.clone();
    let host = Arc::clone(&flow.f.host);
    let owned_repository = repository.clone();
    let owned_managed = managed.clone();
    let job = tokio::spawn(async move {
        host.setup_worktree_session(
            &inv,
            CancellationToken::new(),
            &owned_repository,
            &owned_managed,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !std::fs::metadata(managed.path.join("heartbeat"))
            .is_ok_and(|metadata| metadata.len() > 0)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    job.abort();
    assert!(job.await.unwrap_err().is_cancelled());
    assert_stopped(&managed, &repository).await;
    let error = repository
        .remove(&Git, &CheckoutActivity, &managed, true)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains(&format!("outcome unknown for {}", info.id))
    );
    assert!(managed.path.join("tracked.txt").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn creation_automatically_attaches_setup_and_preserves_failed_session() {
    use cyber_server::runtime::{LiveEvent, NoSnapshots, SetupUpdate};
    for fail in [false, true] {
        let (fixture, _, _) = owned().await;
        fixture.write(".worktreeinclude", ".env\n");
        fixture.write(".env", "included");
        fixture.write("cyber.json", "{}");
        assert!(
            invoke(&fixture.repo, &["add".into(), "cyber.json".into()])
                .unwrap()
                .status
                .success()
        );
        assert!(
            invoke(
                &fixture.repo,
                &[
                    "commit".into(),
                    "--quiet".into(),
                    "-m".into(),
                    "config fixture".into()
                ]
            )
            .unwrap()
            .status
            .success()
        );
        let source = if fail {
            "printf ready; printf once >> setup-result; exit 7"
        } else {
            "printf ready; printf once >> setup-result"
        };
        fixture.set_config(
            json!({"permissions": {"worktree": "allow"}, "worktrees": {"setup": [source]}}),
        );
        let flow = support::flow::Flow::with(fixture, vec![], false, Arc::new(NoSnapshots));
        let inv = creation_source(&flow).await;
        let mut events = flow.runtime.subscribe();
        let result = flow
            .f
            .host
            .create_worktree_session(
                &inv,
                CancellationToken::new(),
                creation_request(&flow.f, "automatic"),
            )
            .await
            .unwrap();
        assert_eq!(
            result.setup.unwrap(),
            if fail {
                SetupOutcome::Failed {
                    index: 0,
                    code: Some(7),
                }
            } else {
                SetupOutcome::Completed
            }
        );
        assert_eq!(
            result.session.directory,
            result.managed.path.display().to_string()
        );
        assert_eq!(
            flow.runtime
                .state(&result.session.id)
                .await
                .unwrap()
                .info
                .directory,
            result.session.directory
        );
        assert_eq!(
            flow.runtime
                .state(&inv.session_id)
                .await
                .unwrap()
                .info
                .directory,
            inv.directory
        );
        assert_eq!(
            std::fs::read_to_string(result.managed.path.join(".env")).unwrap(),
            "included"
        );
        assert_eq!(
            std::fs::read_to_string(result.managed.path.join("cyber.json")).unwrap(),
            "{}"
        );
        assert_eq!(
            std::fs::read_to_string(result.managed.path.join("setup-result")).unwrap(),
            "once"
        );
        let mut finished = false;
        while let Ok(event) = events.try_recv() {
            if let LiveEvent::WorktreeSetup {
                session_id,
                worktree_id,
                update: SetupUpdate::Finished { code, .. },
                ..
            } = event
            {
                assert_eq!(session_id, result.session.id);
                assert_eq!(worktree_id, result.managed.id);
                assert_eq!(code, Some(if fail { 7 } else { 0 }));
                finished = true;
            }
        }
        assert!(finished);
        std::fs::write(result.managed.path.join("tracked.txt"), "user edit").unwrap();
        let reused = flow
            .f
            .host
            .create_worktree_session(
                &inv,
                CancellationToken::new(),
                creation_request(&flow.f, "automatic"),
            )
            .await
            .unwrap();
        assert_eq!(reused.managed.id, result.managed.id);
        assert_ne!(reused.session.id, result.session.id);
        assert_eq!(
            std::fs::read_to_string(result.managed.path.join("tracked.txt")).unwrap(),
            "user edit"
        );
        assert_eq!(
            std::fs::read_to_string(result.managed.path.join("setup-result")).unwrap(),
            "once"
        );
    }
}

#[tokio::test]
async fn creation_refuses_denied_read_only_and_cancelled_before_ownership() {
    use cyber_server::runtime::NoSnapshots;
    for case in [
        "deny",
        "read-only",
        "cancel",
        "wrong-location",
        "existing-id",
        "source-plan",
    ] {
        let (fixture, repository, _) = owned().await;
        fixture.set_config(match case {
            "deny" => json!({"permissions": {"worktree": "deny"}}),
            "read-only" => {
                json!({"permissions": {"worktree": "allow"}, "sandbox": {"policy": "read-only"}})
            }
            _ => json!({"permissions": {"worktree": "allow"}}),
        });
        let flow = support::flow::Flow::with(fixture, vec![], false, Arc::new(NoSnapshots));
        let mut inv = creation_source(&flow).await;
        if case == "source-plan" {
            flow.runtime
                .switch_mode(&inv.session_id, "plan")
                .await
                .unwrap();
        }
        let cancel = CancellationToken::new();
        if case == "cancel" {
            cancel.cancel();
        }
        if case == "wrong-location" {
            inv.directory = flow.f.dir.path().display().to_string();
        }
        let mut request = creation_request(&flow.f, "refused");
        if case == "existing-id" {
            request.session.id = Some(inv.session_id.clone());
        }
        assert!(
            flow.f
                .host
                .create_worktree_session(&inv, cancel, request)
                .await
                .is_err(),
            "{case}"
        );
        assert!(
            !repository
                .common_dir
                .join("cyber-worktrees/refused.json")
                .exists(),
            "{case}"
        );
        assert!(
            !flow
                .f
                .dir
                .path()
                .join("worktrees/prj_test/refused")
                .exists(),
            "{case}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn creation_git_failure_preserves_pending_target_and_refuses_blind_retry() {
    use cyber_server::runtime::NoSnapshots;
    let (fixture, repository, _) = owned().await;
    assert!(
        invoke(&fixture.repo, &["branch".into(), "cyber/collision".into()])
            .unwrap()
            .status
            .success()
    );
    fixture.set_config(json!({"permissions": {"worktree": "allow"}}));
    let flow = support::flow::Flow::with(fixture, vec![], false, Arc::new(NoSnapshots));
    let inv = creation_source(&flow).await;
    let error = flow
        .f
        .host
        .create_worktree_session(
            &inv,
            CancellationToken::new(),
            creation_request(&flow.f, "collision"),
        )
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("already exists"), "{error}");
    let record = repository.common_dir.join("cyber-worktrees/collision.json");
    let pending: Managed = serde_json::from_slice(&std::fs::read(record).unwrap()).unwrap();
    assert!(!pending.ready);
    assert!(pending.path.is_dir());
    std::fs::write(pending.path.join("user-file"), "preserve").unwrap();
    let repeated = flow
        .f
        .host
        .create_worktree_session(
            &inv,
            CancellationToken::new(),
            creation_request(&flow.f, "collision"),
        )
        .await
        .err()
        .unwrap();
    assert!(repeated.to_string().contains("recovery"), "{repeated}");
    assert_eq!(
        std::fs::read_to_string(pending.path.join("user-file")).unwrap(),
        "preserve"
    );
    assert!(
        RepositoryLock::try_acquire(&repository.common_dir)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn explicit_start_preserves_denied_rules_and_read_only_admission() {
    use cyber_server::runtime::NoSnapshots;
    for case in ["deny", "read-only", "session-deny", "plan", "cancel"] {
        let (fixture, repository, _) = owned().await;
        fixture.set_config(match case {
            "deny" => json!({"permissions": {"worktree": "deny"}}),
            "read-only" => json!({"sandbox": {"policy": "read-only"}}),
            _ => json!({}),
        });
        let flow = support::flow::Flow::with(fixture, vec![], false, Arc::new(NoSnapshots));
        let mut request = creation_request(&flow.f, "explicit-refused");
        request.session.mode = Some(if case == "plan" { "plan" } else { "dont-ask" }.into());
        if case == "session-deny" {
            request.session.rules = Some(json!({"worktree": "deny"}));
        }
        let cancel = CancellationToken::new();
        if case == "cancel" {
            cancel.cancel();
        }
        assert!(
            flow.f
                .host
                .start_worktree_session(&flow.f.repo, "call_explicit".into(), cancel, request)
                .await
                .is_err(),
            "{case}"
        );
        assert!(
            !repository
                .common_dir
                .join("cyber-worktrees/explicit-refused.json")
                .exists(),
            "{case}"
        );
        assert!(
            !flow
                .f
                .dir
                .path()
                .join("worktrees/prj_test/explicit-refused")
                .exists(),
            "{case}"
        );
    }
}

#[tokio::test]
async fn reviewed_retry_after_preparation_failure_preserves_success_and_launches_once() {
    use cyber_server::worktrees::SetupJournal;
    struct ObstructPreparation(std::path::PathBuf);
    impl SetupSink for ObstructPreparation {
        fn emit(&self, event: SetupEvent<'_>) -> io::Result<()> {
            if matches!(event, SetupEvent::Started { index: 1, .. }) {
                // Git verification already prepared this directory. Block only the
                // second setup command's preparation, before its launch boundary.
                std::fs::remove_dir(&self.0)?;
                std::fs::write(&self.0, "temporary obstruction")?;
            }
            Ok(())
        }
    }
    let (mut fixture, repository, managed) = owned().await;
    fixture.renew_host(Some("full-access".into()));
    let commands = if cfg!(windows) {
        vec![
            "Add-Content -Path prefix.txt -Value once -NoNewline -Encoding ascii".to_string(),
            "Add-Content -Path target.txt -Value target -NoNewline -Encoding ascii".to_string(),
        ]
    } else {
        vec![
            "printf once >> prefix.txt".to_string(),
            "printf target >> target.txt".to_string(),
        ]
    };
    fixture.set_config(json!({"permissions":{"worktree":"allow"},"worktrees":{"setup":commands}}));
    let mut inv = fixture.invocation("bypass", "worktree", json!({}));
    inv.directory = managed.path.display().to_string();
    let obstruction = fixture.dir.path().join("tmp").join(&inv.session_id);
    let error = fixture
        .host
        .setup_worktree(
            &inv,
            CancellationToken::new(),
            &repository,
            &managed,
            &ObstructPreparation(obstruction.clone()),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Could not create"), "{error}");
    assert_eq!(
        std::fs::read_to_string(managed.path.join("prefix.txt")).unwrap(),
        "once"
    );
    assert!(!managed.path.join("target.txt").exists());
    let journal = SetupJournal::new(
        Arc::clone(&fixture.store),
        &managed,
        &commands,
        &inv.session_id,
    )
    .unwrap();
    let snapshot = journal.snapshot().unwrap();
    let result = serde_json::to_value(&snapshot.commands[1]).unwrap();
    assert_eq!(result["result"]["status"], "not_dispatched");
    // Repairing preparation alone does not grant redispatch.
    std::fs::remove_file(obstruction).unwrap();
    assert!(
        fixture
            .host
            .setup_worktree(
                &inv,
                CancellationToken::new(),
                &repository,
                &managed,
                &Sink::default(),
            )
            .await
            .is_err()
    );
    assert_eq!(journal.snapshot().unwrap(), snapshot);
    journal
        .retry_failed(
            snapshot.revision,
            &snapshot.digest,
            1,
            "Private temporary directory repaired",
        )
        .unwrap();
    assert_eq!(
        fixture
            .host
            .setup_worktree(
                &inv,
                CancellationToken::new(),
                &repository,
                &managed,
                &Sink::default(),
            )
            .await
            .unwrap(),
        cyber_core::worktrees::SetupOutcome::Completed
    );
    assert_eq!(
        std::fs::read_to_string(managed.path.join("prefix.txt")).unwrap(),
        "once"
    );
    assert_eq!(
        std::fs::read_to_string(managed.path.join("target.txt")).unwrap(),
        "target"
    );
    fixture
        .host
        .setup_worktree(
            &inv,
            CancellationToken::new(),
            &repository,
            &managed,
            &Sink::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(managed.path.join("target.txt")).unwrap(),
        "target"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn process_launch_failure_remains_uncertain_and_cannot_authorize_retry() {
    use cyber_server::worktrees::{CommandResult, CommandStatus, SetupJournal};
    let (owner, repository, managed) = owned().await;
    let shell = owner.dir.path().join("bash");
    // An existing recognized shell is selected, but the non-executable file
    // makes the actual process launch fail after preparation has succeeded.
    std::fs::write(&shell, "not executable").unwrap();
    let fixture = support::Fixture::with_policy(
        shell.to_str().unwrap(),
        cyber_sandbox::find_helper(),
        Some("full-access".into()),
    );
    let commands = vec!["printf must-not-run > target.txt".to_string()];
    fixture.set_config(json!({"permissions":{"worktree":"allow"},"worktrees":{"setup":commands}}));
    let mut inv = fixture.invocation("bypass", "worktree", json!({}));
    inv.directory = managed.path.display().to_string();
    assert!(
        fixture
            .host
            .setup_worktree(
                &inv,
                CancellationToken::new(),
                &repository,
                &managed,
                &Sink::default(),
            )
            .await
            .is_err()
    );
    let journal = SetupJournal::new(
        Arc::clone(&fixture.store),
        &managed,
        &commands,
        &inv.session_id,
    )
    .unwrap();
    let snapshot = journal.snapshot().unwrap();
    assert!(matches!(
        snapshot.commands[0],
        CommandStatus::Finished {
            result: CommandResult::Failed { .. },
        }
    ));
    assert!(
        journal
            .retry_failed(
                snapshot.revision,
                &snapshot.digest,
                0,
                "Launch failure is not proof of nondispatch"
            )
            .is_err()
    );
    assert_eq!(journal.snapshot().unwrap(), snapshot);
    assert!(!managed.path.join("target.txt").exists());
}

#[tokio::test]
async fn retained_native_location_proof_prevents_checkout_removal_until_disposal() {
    use cyber_core::worktrees::CheckoutActivity;
    use cyber_server::runtime::{CreateSession, NoSnapshots, ToolHost};
    let (fixture, repository, managed) = owned().await;
    #[cfg(windows)]
    fixture.set_config(json!({"sandbox":{"policy":"full-access"}}));
    let flow = support::flow::Flow::with(fixture, vec![], false, Arc::new(NoSnapshots));
    let info = flow
        .runtime
        .create_session(CreateSession {
            directory: managed.path.display().to_string(),
            model: "test/main".into(),
            mode: Some("bypass".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let lease = flow
        .f
        .host
        .claim_location(&info, false, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(lease.worktree_id.as_deref(), Some(managed.id.as_str()));
    let proof = lease.settle_retained().unwrap();
    let error = repository
        .remove(&Git, &CheckoutActivity, &managed, false)
        .await
        .unwrap_err();
    assert!(error.to_string().contains(&info.id), "{error}");
    assert!(managed.path.exists());
    drop(proof);
    repository
        .remove(&Git, &CheckoutActivity, &managed, false)
        .await
        .unwrap();
    assert!(!managed.path.exists());
}

#[tokio::test]
async fn acknowledged_native_worktree_scope_reopens_with_original_checkout_identity() {
    use cyber_core::worktrees::CheckoutActivity;
    use cyber_server::runtime::{CreateSession, NoSnapshots, SubtreeStopStatus};
    let (fixture, repository, managed) = owned().await;
    #[cfg(windows)]
    fixture.set_config(json!({"sandbox":{"policy":"full-access"}}));
    let flow = support::flow::Flow::with(fixture, vec![], false, Arc::new(NoSnapshots));
    let info = flow
        .runtime
        .create_session(CreateSession {
            directory: managed.path.display().to_string(),
            model: "test/main".into(),
            mode: Some("bypass".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let old = flow.runtime.capture_child_admission(&info.id).unwrap();
    let stopped = flow.runtime.stop_subtree(&info.id).await.unwrap();
    assert_eq!(
        stopped.status,
        SubtreeStopStatus::Acknowledged,
        "{:?}",
        stopped.problems
    );
    flow.runtime
        .reopen_subtree(
            &info.id,
            &stopped.scope_id,
            stopped.receipt_id.as_deref().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        flow.runtime
            .state(&info.id)
            .await
            .unwrap()
            .info
            .worktree_id
            .as_deref(),
        Some(managed.id.as_str())
    );
    assert!(flow.runtime.capture_child_admission(&info.id).is_ok());
    assert!(old.verify(&flow.runtime, &info.id).is_err());
    repository
        .remove(&Git, &CheckoutActivity, &managed, false)
        .await
        .unwrap();
}

#[cfg(all(unix, any(target_os = "macos", target_os = "linux")))]
#[tokio::test]
async fn lsp_unopened_deletion_retains_all_managed_claims_through_cancelled_close() {
    use cyber_core::worktrees::CheckoutActivity;
    use cyber_server::runtime::ToolHost;
    use cyber_tools::lsp::Locations;
    assert!(
        cyber_sandbox::available(),
        "native sandbox prerequisites are required"
    );
    let (fixture, repository, managed) = owned().await;
    let nested = repository
        .create(
            &Git,
            &Settings::default(),
            &managed.path.join("nested-data"),
            "prj_nested",
            &Name::parse("nested").unwrap(),
        )
        .await
        .unwrap();
    let location = fixture.dir.path().canonicalize().unwrap();
    let outer = location.join("outer.txt");
    std::fs::write(&outer, "outer").unwrap();
    let pool = managed_lsp_pool(&fixture, &location);
    let handle = pool.ensure("fixture", &outer).unwrap();
    handle
        .request("ping", json!({}), Duration::from_secs(3))
        .await
        .unwrap();
    let retained = pool.clone();
    let locations = Locations::new(Arc::new(move |_| Ok(retained.clone())));
    fixture
        .host
        .attach_lsp_locations(locations.clone())
        .unwrap();
    let file = nested.path.join("tracked.txt").canonicalize().unwrap();
    let mut invocation = fixture.invocation("accept-edits", "apply_patch", json!({"patch":format!("*** Begin Patch\n*** Delete File: {}\n*** End Patch",file.display())}));
    invocation.directory = location.display().to_string();
    support::ok(
        fixture
            .host
            .execute(invocation, CancellationToken::new())
            .await,
    );
    assert!(!file.exists());
    assert_eq!(
        handle
            .request("documents", json!({}), Duration::from_secs(3))
            .await
            .unwrap(),
        json!([])
    );
    assert_eq!(
        handle
            .request("closed", json!({}), Duration::from_secs(3))
            .await
            .unwrap(),
        json!([])
    );
    assert_eq!(
        handle
            .request("watched", json!({}), Duration::from_secs(3))
            .await
            .unwrap(),
        json!([{"uri":reqwest::Url::from_file_path(&file).unwrap().as_str(),"type":3}])
    );
    for checkout in [&nested, &managed] {
        let error = repository
            .remove(&Git, &CheckoutActivity, checkout, true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("in use by lsp_"), "{error}");
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(20), locations.close())
            .await
            .is_err()
    );
    assert!(
        repository
            .remove(&Git, &CheckoutActivity, &managed, true)
            .await
            .is_err()
    );
    assert!(locations.close().await.unwrap()[0].acknowledged);
    repository
        .remove(&Git, &CheckoutActivity, &nested, true)
        .await
        .unwrap();
    repository
        .remove(&Git, &CheckoutActivity, &managed, true)
        .await
        .unwrap();
}

/// WorktreeCreate runs after worktree permission and before any git effect; its
/// deny refuses the creation.
#[cfg(unix)]
#[tokio::test]
async fn worktree_create_hook_deny_refuses_creation_before_git_effects() {
    use cyber_server::runtime::NoSnapshots;
    let (fixture, repository, _) = owned().await;
    fixture.set_config(json!({"permissions": {"worktree": "allow"}}));
    let flow = support::flow::Flow::with(fixture, vec![], false, Arc::new(NoSnapshots));
    let deny =
        r#"cat > worktree-event.json; printf '%s' '{"decision":"deny","reason":"no worktrees"}'"#;
    attach_global_hooks(
        &flow,
        json!({"WorktreeCreate":[{"matcher":"refused","hooks":[{"type":"command","id":"guard","command":deny}]}]}),
    );
    let inv = creation_source(&flow).await;
    let error = flow
        .f
        .host
        .create_worktree_session(
            &inv,
            CancellationToken::new(),
            creation_request(&flow.f, "refused"),
        )
        .await
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("blocked by hook guard: no worktrees"),
        "{error}"
    );
    assert!(
        !repository
            .common_dir
            .join("cyber-worktrees/refused.json")
            .exists()
    );
    let event: serde_json::Value =
        serde_json::from_slice(&std::fs::read(flow.f.repo.join("worktree-event.json")).unwrap())
            .unwrap();
    assert_eq!(event["event"], "WorktreeCreate");
    assert_eq!(event["name"], "refused");
    assert_eq!(event["session_id"], inv.session_id);
}

/// Attach a live hook resolver whose global configuration holds `hooks`.
#[cfg(any(unix, windows))]
fn attach_global_hooks(flow: &support::flow::Flow, hooks: serde_json::Value) {
    let env = std::collections::HashMap::from([(
        "CYBER_HOME".into(),
        flow.f.dir.path().join("hook-config").display().to_string(),
    )]);
    let paths = cyber_core::paths::Paths::resolve(&env, flow.f.dir.path());
    paths.ensure().unwrap();
    std::fs::write(
        paths.config.join("cyber.jsonc"),
        json!({"hooks":hooks}).to_string(),
    )
    .unwrap();
    let home = flow.f.dir.path().to_path_buf();
    let trust = cyber_core::trust::TrustStore::new(paths.trust_file());
    flow.f
        .host
        .attach_hook_config(
            Arc::new(move |location| {
                cyber_core::config::load(&cyber_core::config::LoadRequest {
                    location,
                    paths: &paths,
                    env: &env,
                    home: &home,
                    profile: None,
                    overrides: &[],
                    flags: json!({}),
                })
                .map_err(|error| error.to_string())
            }),
            trust,
        )
        .unwrap();
}

/// WorktreeCreate runs before shutdown admission is held, so shutdown completes
/// while a handler is still held rather than waiting behind it: the handler stops
/// with the runtime (never by finishing), its receipt settles, and the creation is
/// refused before any git effect. A release-file barrier replaces wall-clock
/// timing. The pre-fix nested-admission deadlock needs shutdown's writer to queue
/// between a handler's cancellation check and its admission read, which no
/// external barrier can order (see `p1-status.md`).
#[cfg(unix)]
#[tokio::test]
async fn shutdown_during_a_held_worktree_create_hook_settles_without_git_effects() {
    use cyber_server::runtime::{HookExecutionStatus, NoSnapshots};
    let (fixture, repository, _) = owned().await;
    fixture.set_config(json!({"permissions": {"worktree": "allow"}}));
    let flow = support::flow::Flow::with(fixture, vec![], false, Arc::new(NoSnapshots));
    let held = r#"cat >/dev/null; touch hook-started; while [ ! -f release ]; do sleep 0.02; done; touch hook-finished; printf '%s' '{"decision":"allow"}'"#;
    attach_global_hooks(
        &flow,
        json!({"WorktreeCreate":[{"hooks":[{"type":"command","id":"held","command":held}]}]}),
    );
    let inv = creation_source(&flow).await;
    let repo = flow.f.repo.clone();
    let creation = flow.f.host.create_worktree_session(
        &inv,
        CancellationToken::new(),
        creation_request(&flow.f, "late"),
    );
    let shutdown = async {
        while !repo.join("hook-started").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        tokio::time::timeout(std::time::Duration::from_secs(20), flow.runtime.shutdown())
            .await
            .expect("shutdown does not wait behind the held hook");
        std::fs::write(repo.join("release"), "").unwrap();
    };
    let (created, ()) = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        tokio::join!(creation, shutdown)
    })
    .await
    .expect("creation and shutdown settle");
    assert!(
        !repo.join("hook-finished").exists(),
        "the handler stopped with the runtime while still held"
    );
    assert!(created.is_err(), "creation after shutdown must be refused");
    assert!(
        !repository
            .common_dir
            .join("cyber-worktrees/late.json")
            .exists()
    );
    let receipts = flow.runtime.hook_executions(&inv.session_id, 10).unwrap();
    assert_eq!(receipts.len(), 1, "{receipts:?}");
    assert_eq!(receipts[0].event, "WorktreeCreate");
    assert!(
        matches!(receipts[0].status, HookExecutionStatus::Completed),
        "{receipts:?}"
    );
}

/// A committed repository whose Sessions may use worktrees and edit. Full-access
/// selects process availability; these cases do not claim sandbox confinement.
fn worktree_flow(script: Vec<Vec<cyber_llm::adapters::ScriptStep>>) -> support::flow::Flow {
    let fixture = support::Fixture::with_policy(
        "bash",
        cyber_sandbox::find_helper(),
        Some("full-access".into()),
    );
    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.name", "Worktree test"],
        vec!["config", "user.email", "test@example.invalid"],
    ] {
        let args: Vec<OsString> = args.into_iter().map(OsString::from).collect();
        assert!(invoke(&fixture.repo, &args).unwrap().status.success());
    }
    fixture.write("tracked.txt", "base");
    for args in [
        vec!["add", "tracked.txt"],
        vec!["commit", "--quiet", "-m", "initial"],
    ] {
        let args: Vec<OsString> = args.into_iter().map(OsString::from).collect();
        assert!(invoke(&fixture.repo, &args).unwrap().status.success());
    }
    let flow = support::flow::Flow::with(
        fixture,
        script,
        false,
        Arc::new(cyber_server::runtime::NoSnapshots),
    );
    flow.f
        .set_config(json!({"permissions":{"worktree":"allow","edit":"allow","agent":"allow"}}));
    flow
}

fn offered(request: &cyber_llm::LlmRequest) -> Vec<String> {
    request
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .filter(|name| name.ends_with("_worktree"))
        .collect()
}

fn branch_exists(repo: &Path, branch: &str) -> bool {
    let args: Vec<OsString> = ["rev-parse", "--verify", "--quiet"]
        .into_iter()
        .map(OsString::from)
        .chain([OsString::from(format!("refs/heads/{branch}"))])
        .collect();
    invoke(repo, &args).unwrap().status.success()
}

#[tokio::test]
async fn model_enter_worktree_resolves_later_paths_in_spike_and_exit_returns() {
    use support::flow::{call, text};
    let flow = worktree_flow(vec![
        call("enter", "enter_worktree", json!({"name": "spike"})),
        call(
            "probe",
            "write",
            json!({"path": "probe.txt", "content": "in spike\n"}),
        ),
        call("leave", "exit_worktree", json!({})),
        call("again", "enter_worktree", json!({"name": "tidy"})),
        call("drop", "exit_worktree", json!({})),
        text("done"),
    ]);
    let id = flow.session("default").await;
    flow.prompt(&id, "work in a worktree").await;
    flow.settle_within(&id, std::time::Duration::from_secs(60))
        .await;
    let entered: serde_json::Value = serde_json::from_str(&flow.output(&id, "enter").await)
        .unwrap_or_else(|error| {
            panic!(
                "{error}: {}",
                futures::executor::block_on(flow.output(&id, "enter"))
            )
        });
    let mut stable = entered.clone();
    stable["path"] = json!("<worktree>");
    assert!(
        Path::new(entered["returned_to_on_exit"].as_str().unwrap())
            .canonicalize()
            .unwrap()
            == flow.f.repo.canonicalize().unwrap()
    );
    stable["returned_to_on_exit"] = json!("<repo>");
    support::golden(&flow.f, "enter_worktree", &stable.to_string());
    assert_eq!(entered["created"], true, "{entered}");
    assert_eq!(entered["branch"], "cyber/spike");
    let spike = std::path::PathBuf::from(entered["path"].as_str().unwrap());
    // The write after entering resolved its relative path in the worktree.
    assert_eq!(
        std::fs::read_to_string(spike.join("probe.txt")).unwrap(),
        "in spike\n"
    );
    assert!(!flow.f.repo.join("probe.txt").exists());
    // Exit returns; the changed worktree is kept with its branch.
    let left: serde_json::Value = serde_json::from_str(&flow.output(&id, "leave").await).unwrap();
    assert_eq!(left["kept"], true, "{left}");
    assert!(
        Path::new(left["returned_to"].as_str().unwrap())
            .canonicalize()
            .unwrap()
            == flow.f.repo.canonicalize().unwrap()
    );
    assert!(branch_exists(&flow.f.repo, "cyber/spike"));
    // A worktree left without changes is removed with its branch.
    let dropped: serde_json::Value = serde_json::from_str(&flow.output(&id, "drop").await).unwrap();
    assert_eq!(dropped["kept"], false, "{dropped}");
    assert!(!Path::new(dropped["path"].as_str().unwrap()).exists());
    assert!(!branch_exists(&flow.f.repo, "cyber/tidy"));
    let info = flow.runtime.state(&id).await.unwrap().info;
    assert!(
        Path::new(&info.directory).canonicalize().unwrap() == flow.f.repo.canonicalize().unwrap()
    );
    assert_eq!(info.worktree_id, None);
    assert_eq!(info.worktree_origin, None);
    // Only the tool that fits the Session's checkout is offered.
    let requests = flow.requests("test/main");
    assert_eq!(offered(&requests[0]), ["enter_worktree"]);
    assert_eq!(offered(&requests[1]), ["exit_worktree"]);
    assert_eq!(offered(&requests[2]), ["exit_worktree"]);
    assert_eq!(offered(&requests[3]), ["enter_worktree"]);
    // Both Drain leases on the worktree were settled: the user can remove it.
    flow.f
        .host
        .remove_worktree(
            &flow.f.repo,
            &Name::parse("spike").unwrap(),
            true,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!spike.exists());
}

/// Regression: `exit_worktree` removes only a checkout its Session created. Entering
/// another Session's existing worktree and exiting keeps the checkout and its branch.
#[tokio::test]
async fn exiting_an_entered_existing_worktree_keeps_it() {
    use support::flow::{call, text};
    let flow = worktree_flow(vec![
        text("owner done"),
        call("enter", "enter_worktree", json!({"name": "spike"})),
        call("leave", "exit_worktree", json!({})),
        text("done"),
    ]);
    let owner = flow
        .f
        .host
        .start_worktree_session(
            &flow.f.repo,
            "call_start".into(),
            CancellationToken::new(),
            creation_request(&flow.f, "spike"),
        )
        .await
        .unwrap();
    flow.prompt(&owner.session.id, "go").await;
    flow.settle(&owner.session.id).await;
    let spike = std::path::PathBuf::from(&owner.session.directory);
    let id = flow.session("default").await;
    flow.prompt(&id, "visit spike").await;
    flow.settle_within(&id, std::time::Duration::from_secs(60))
        .await;
    let entered: serde_json::Value =
        serde_json::from_str(&flow.output(&id, "enter").await).unwrap();
    assert_eq!(entered["created"], false, "{entered}");
    let left: serde_json::Value = serde_json::from_str(&flow.output(&id, "leave").await).unwrap();
    assert_eq!(left["kept"], true, "{left}");
    assert_eq!(left["cleanup_reason"], "entered an existing worktree");
    assert!(spike.exists());
    assert!(branch_exists(&flow.f.repo, "cyber/spike"));
    let info = flow.runtime.state(&id).await.unwrap().info;
    assert_eq!(info.worktree_origin, None);
}

#[tokio::test]
async fn exit_worktree_is_not_offered_to_isolated_children() {
    use cyber_server::runtime::ToolHost;
    use support::flow::text;
    let flow = worktree_flow(vec![text("child done"), text("isolated root done")]);
    let parent = flow.session("default").await;
    let mut inv = flow.f.invocation(
        "default",
        "agent",
        json!({"prompt": "look", "isolation": "worktree", "model": "test/main"}),
    );
    inv.session_id = parent.clone();
    let info = flow.runtime.state(&parent).await.unwrap().info;
    inv.agent = info.agent;
    inv.rules = info.rules;
    let outcome = flow.f.host.execute(inv, CancellationToken::new()).await;
    assert!(
        matches!(outcome, cyber_server::runtime::ToolOutcome::Ok(_)),
        "{outcome:?}"
    );
    // A top-level Session started in a managed checkout is isolated too.
    let started = flow
        .f
        .host
        .start_worktree_session(
            &flow.f.repo,
            "call_start".into(),
            CancellationToken::new(),
            creation_request(&flow.f, "started"),
        )
        .await
        .unwrap();
    flow.prompt(&started.session.id, "go").await;
    flow.settle(&started.session.id).await;
    let requests = flow.requests("test/main");
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(offered(request), Vec::<String>::new());
    }
    // A model that calls it anyway is refused before the Session moves.
    let mut call = flow.f.invocation("default", "exit_worktree", json!({}));
    call.session_id = started.session.id.clone();
    call.directory = started.session.directory.clone();
    let outcome = flow.f.host.execute(call, CancellationToken::new()).await;
    assert!(
        matches!(&outcome, cyber_server::runtime::ToolOutcome::Failed(error) if error.contains("did not enter")),
        "{outcome:?}"
    );
}

/// Real HTTP hook dispatch, with an optional held response for shutdown ordering.
#[cfg(any(unix, windows))]
struct CwdRecorder {
    seen: Arc<Mutex<Vec<serde_json::Value>>>,
    entered: Arc<Notify>,
    release: CancellationToken,
    url: String,
    task: tokio::task::JoinHandle<()>,
}

#[cfg(any(unix, windows))]
impl CwdRecorder {
    async fn start(held: bool) -> Self {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let entered = Arc::new(Notify::new());
        let release = CancellationToken::new();
        if !held {
            release.cancel();
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        let router = axum::Router::new().route(
            "/hook",
            axum::routing::post({
                let seen = Arc::clone(&seen);
                let entered = Arc::clone(&entered);
                let release = release.clone();
                move |axum::Json(event): axum::Json<serde_json::Value>| {
                    let seen = Arc::clone(&seen);
                    let entered = Arc::clone(&entered);
                    let release = release.clone();
                    async move {
                        seen.lock().unwrap().push(event);
                        entered.notify_one();
                        release.cancelled().await;
                        // An observational decision cannot veto the committed move.
                        axum::Json(json!({"decision":"deny", "reason":"observation only"}))
                    }
                }
            }),
        );
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            seen,
            entered,
            release,
            url,
            task,
        }
    }

    fn attach(&self, flow: &support::flow::Flow) {
        attach_global_hooks(
            flow,
            json!({"CwdChanged":[{"hooks":[{
                "type":"http", "id":"cwd-observer", "url":self.url, "fail_closed":true
            }]}]}),
        );
    }

    fn events(&self) -> Vec<serde_json::Value> {
        self.seen.lock().unwrap().clone()
    }
}

#[cfg(any(unix, windows))]
impl Drop for CwdRecorder {
    fn drop(&mut self) {
        self.release.cancel();
        self.task.abort();
    }
}

#[cfg(any(unix, windows))]
fn cwd_history(flow: &support::flow::Flow, id: &str) -> Vec<cyber_store::StoredEvent> {
    let mut events = Vec::new();
    let mut after = -1;
    loop {
        let page = flow
            .f
            .store
            .read_events(id, after, cyber_store::MAX_PAGE_LIMIT)
            .unwrap();
        if let Some(last) = page.events.last() {
            after = last.seq;
        }
        events.extend(page.events);
        if !page.has_more {
            return events;
        }
    }
}

#[cfg(any(unix, windows))]
fn assert_cwd_transition(flow: &support::flow::Flow, id: &str, hook: &serde_json::Value) {
    let events = cwd_history(flow, id);
    let transition = events
        .iter()
        .find(|event| hook["transition_id"] == event.id)
        .unwrap();
    assert_eq!(hook["event"], "CwdChanged");
    assert_eq!(hook["session_id"], id);
    assert_eq!(hook["transition_sequence"], transition.seq);
    assert_eq!(hook["transition"], transition.kind);
    assert_eq!(hook["previous_directory"], transition.data["from"]["path"]);
    assert_eq!(hook["directory"], transition.data["to"]["path"]);
    assert_eq!(hook["location"]["directory"], hook["directory"]);
    assert_ne!(hook["previous_directory"], hook["directory"]);
    assert_eq!(hook["agent"], "build");
    assert_eq!(hook["mode"], "default");
    assert!(hook["timestamp"].is_number());
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn cwd_changed_hooks_observe_committed_enter_and_exit_only() {
    use support::flow::{call, text};
    let flow = worktree_flow(vec![
        call("enter", "enter_worktree", json!({"name":"cwd"})),
        call("again", "enter_worktree", json!({"name":"refused"})),
        call("exit", "exit_worktree", json!({})),
        text("done"),
    ]);
    let recorder = CwdRecorder::start(false).await;
    recorder.attach(&flow);
    let id = flow.session("default").await;
    flow.prompt(&id, "visit a worktree").await;
    flow.settle_within(&id, Duration::from_secs(60)).await;
    let events = recorder.events();
    assert_eq!(events.len(), 2, "{events:?}");
    for event in &events {
        assert_cwd_transition(&flow, &id, event);
    }
    assert_eq!(events[0]["transition"], "session.worktree.entered.1");
    assert_eq!(events[1]["transition"], "session.worktree.exited.1");
    assert_ne!(events[0]["transition_id"], events[1]["transition_id"]);
    assert_eq!(events[0]["directory"], events[1]["previous_directory"]);
    assert_eq!(
        flow.runtime.state(&id).await.unwrap().info.directory,
        flow.f.repo.display().to_string()
    );
    let receipts = flow.runtime.hook_executions(&id, 20).unwrap();
    assert_eq!(
        receipts
            .iter()
            .filter(|receipt| receipt.event == "CwdChanged")
            .count(),
        2
    );
    assert!(
        receipts
            .iter()
            .all(|receipt| receipt.acknowledged == Some(true))
    );
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn cwd_changed_idle_exit_is_committed_once_and_replay_is_silent() {
    use support::flow::{call, text};
    let mut flow = worktree_flow(vec![
        call("enter", "enter_worktree", json!({"name":"idle-cwd"})),
        text("done"),
    ]);
    let recorder = CwdRecorder::start(false).await;
    recorder.attach(&flow);
    let id = flow.session("default").await;
    flow.prompt(&id, "enter and finish").await;
    flow.settle_within(&id, Duration::from_secs(60)).await;
    let unrelated = flow.session("default").await;
    // Forced removal owns shutdown admission while returning entered Sessions.
    // Dispatch must be deferred past that outer guard, even if removal fails.
    tokio::time::timeout(
        Duration::from_secs(30),
        flow.f.host.remove_worktree(
            &flow.f.repo,
            &Name::parse("idle-cwd").unwrap(),
            true,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("idle exit hook and removal settle")
    .unwrap();
    assert!(
        flow.runtime
            .exit_idle_worktree(&id)
            .await
            .unwrap()
            .is_none()
    );
    let events = recorder.events();
    assert_eq!(events.len(), 2, "{events:?}");
    for event in &events {
        assert_cwd_transition(&flow, &id, event);
    }
    assert!(events.iter().all(|event| event["session_id"] != unrelated));
    let before = cwd_history(&flow, &id);
    flow.restart_default_runtime().await;
    recorder.attach(&flow);
    let state = flow.runtime.state(&id).await.unwrap();
    assert_eq!(state.info.directory, flow.f.repo.display().to_string());
    assert!(
        flow.runtime
            .exit_idle_worktree(&id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        recorder.events(),
        events,
        "loading durable history must not replay handlers"
    );
    assert_eq!(cwd_history(&flow, &id), before);
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn cwd_changed_failed_idle_exit_commit_is_silent_and_retry_commits_once() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use support::flow::{call, text};

    let flow = worktree_flow(vec![
        call("enter", "enter_worktree", json!({"name":"failed-cwd-exit"})),
        text("done"),
    ]);
    let recorder = CwdRecorder::start(false).await;
    recorder.attach(&flow);
    let id = flow.session("default").await;
    flow.prompt(&id, "enter and finish").await;
    flow.settle_within(&id, Duration::from_secs(60)).await;
    let entered = recorder.events();
    assert_eq!(entered.len(), 1);
    assert_cwd_transition(&flow, &id, &entered[0]);
    let directory = flow.runtime.state(&id).await.unwrap().info.directory;
    assert_ne!(directory, flow.f.repo.display().to_string());

    let attempted = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&attempted);
    let session = id.clone();
    flow.f
        .store
        .set_append_fault(Some(Arc::new(move |owner, event| {
            let refuse = owner == session && event.kind == "session.worktree.exited.1";
            if refuse {
                observed.store(true, Ordering::SeqCst);
            }
            refuse
        })));
    let failed = tokio::time::timeout(
        Duration::from_secs(30),
        flow.runtime.exit_idle_worktree(&id),
    )
    .await;
    flow.f.store.set_append_fault(None);
    let error = failed.expect("failed commit settles").unwrap_err();
    assert!(attempted.load(Ordering::SeqCst));
    assert!(
        error.to_string().contains("injected append fault"),
        "{error}"
    );
    assert_eq!(
        flow.runtime.state(&id).await.unwrap().info.directory,
        directory
    );
    assert_eq!(
        recorder.events(),
        entered,
        "a failed commit emits no CwdChanged"
    );
    assert!(
        cwd_history(&flow, &id)
            .iter()
            .all(|event| event.kind != "session.worktree.exited.1")
    );

    tokio::time::timeout(
        Duration::from_secs(30),
        flow.runtime.exit_idle_worktree(&id),
    )
    .await
    .expect("retry settles")
    .unwrap()
    .expect("retry commits the idle exit");
    assert_eq!(
        flow.runtime.state(&id).await.unwrap().info.directory,
        flow.f.repo.display().to_string()
    );
    let events = recorder.events();
    assert_eq!(events.len(), 2, "{events:?}");
    assert_cwd_transition(&flow, &id, &events[1]);
    assert_eq!(events[1]["transition"], "session.worktree.exited.1");
    assert_eq!(events[1]["previous_directory"], directory);
    assert_eq!(
        cwd_history(&flow, &id)
            .iter()
            .filter(|event| event.kind == "session.worktree.exited.1")
            .count(),
        1
    );
    assert!(
        flow.runtime
            .exit_idle_worktree(&id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        recorder.events(),
        events,
        "an already exited Session is silent"
    );
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn cwd_changed_does_not_observe_permission_refusal() {
    use support::flow::{call, text};
    let flow = worktree_flow(vec![
        call("enter", "enter_worktree", json!({"name":"denied-cwd"})),
        text("done"),
    ]);
    flow.f
        .set_config(json!({"permissions":{"worktree":"deny"}}));
    let recorder = CwdRecorder::start(false).await;
    recorder.attach(&flow);
    let id = flow.session("default").await;
    flow.prompt(&id, "try to enter").await;
    flow.settle_within(&id, Duration::from_secs(30)).await;
    assert!(recorder.events().is_empty());
    assert_eq!(
        flow.runtime.state(&id).await.unwrap().info.directory,
        flow.f.repo.display().to_string()
    );
    assert!(!branch_exists(&flow.f.repo, "cyber/denied-cwd"));
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn cwd_changed_hook_shutdown_releases_outer_worktree_admission() {
    use support::flow::{call, text};
    let flow = worktree_flow(vec![
        call("enter", "enter_worktree", json!({"name":"held-cwd"})),
        text("done"),
    ]);
    let id = flow.session("default").await;
    flow.prompt(&id, "enter and finish").await;
    flow.settle_within(&id, Duration::from_secs(60)).await;
    let recorder = CwdRecorder::start(true).await;
    recorder.attach(&flow);
    let name = Name::parse("held-cwd").unwrap();
    let exiting = flow
        .f
        .host
        .remove_worktree(&flow.f.repo, &name, true, CancellationToken::new());
    let stopping = async {
        recorder.entered.notified().await;
        flow.runtime.shutdown().await;
        recorder.release.cancel();
    };
    tokio::time::timeout(Duration::from_secs(30), async {
        let (result, ()) = tokio::join!(exiting, stopping);
        result.unwrap();
    })
    .await
    .expect("shutdown and committed observation settle without nested admission deadlock");
    let events = recorder.events();
    assert_eq!(events.len(), 1);
    assert_cwd_transition(&flow, &id, &events[0]);
}
