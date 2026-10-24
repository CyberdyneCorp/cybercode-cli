//! The background update check (`installation-upgrade` → Background update policy)
//! through the production server: `run_server_until` with a pinned version, channel and
//! environment, a loopback release source reached through the debug-only test hook, the
//! instance event stream over HTTP and `GET /api/v1/service/update`.

#[path = "../../cyber-install/tests/support/stub_release.rs"]
mod stub_release;

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cyber_app::update_check::UpdateCheckOptions;
use cyber_app::{App, AppOptions, ServeOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use cyber_core::version::{Channel, ReleaseSource};
use cyber_install::test_support::TestSigner;
use futures::StreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use stub_release::{Reply, StubRelease};
use tokio::sync::Notify;

const PASSWORD: &str = "update-check-password";
const CURRENT: &str = "1.4.0";

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    stub: StubRelease,
    env: HashMap<String, String>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let stub = StubRelease::start();
        // An empty PATH: no package manager is asked about the binary.
        std::fs::create_dir_all(root.join("empty-path")).unwrap();
        let env = HashMap::from([
            ("HOME".to_string(), root.join("home").display().to_string()),
            (
                "PATH".to_string(),
                root.join("empty-path").display().to_string(),
            ),
            (
                "CYBER_TEST_RELEASE_BASE_URL".to_string(),
                stub.base_url.clone(),
            ),
        ]);
        Self {
            _dir: dir,
            root,
            stub,
            env,
        }
    }

    fn paths(&self) -> Paths {
        Paths {
            data: self.root.join("data"),
            config: self.root.join("config"),
            state: self.root.join("state"),
            cache: self.root.join("cache"),
            tmp: self.root.join("tmp"),
        }
    }

    async fn app(&self, config: Value) -> App {
        let paths = self.paths();
        paths.ensure().unwrap();
        std::fs::write(paths.config.join("cyber.json"), config.to_string()).unwrap();
        App::build(AppOptions {
            paths,
            home: self.root.join("home"),
            database: DatabaseLocation::Memory,
            default_directory: self.root.clone(),
            sandbox_policy: None,
            snapshots: false,
            interactive: true,
            password: Some(PASSWORD.into()),
            mcp_credentials: None,
            remote_skill_fetcher: None,
            workflow_cores: None,
            isolation: None,
        })
        .await
        .unwrap()
    }

    /// The production server checking `first` after start, as `exe` of a stable 1.4.0.
    async fn serve(&self, config: Value, exe: PathBuf, first: Duration) -> Server {
        self.serve_every(config, exe, first, cyber_app::update_check::INTERVAL)
            .await
    }

    /// [`Fixture::serve`], checking again every `interval`.
    async fn serve_every(
        &self,
        config: Value,
        exe: PathBuf,
        first: Duration,
        interval: Duration,
    ) -> Server {
        let app = self.app(config).await;
        let stop = Arc::new(Notify::new());
        let (ready, address) = tokio::sync::oneshot::channel();
        let options = ServeOptions {
            port: Some(0),
            socket: cfg!(unix).then(|| self.root.join("s.sock")),
            update: UpdateCheckOptions {
                current: CURRENT.into(),
                channel: Channel::Stable,
                exe: Some(exe),
                env: Arc::new(self.env.clone()),
                first,
                interval,
                ..UpdateCheckOptions::default()
            },
            ..ServeOptions::default()
        };
        let task = tokio::spawn(cyber_app::run_server_until(
            app,
            options,
            move |url| {
                let _ = ready.send(url.to_string());
            },
            Arc::clone(&stop),
        ));
        let url = tokio::time::timeout(Duration::from_secs(20), address)
            .await
            .expect("the server started")
            .unwrap();
        let client = cyber_client::Client::http(&url, Some(PASSWORD.into()));
        Server { client, stop, task }
    }

    fn exe(&self, dir: &str) -> PathBuf {
        let dir = self.root.join(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(exe_name("cyber"));
        std::fs::write(&exe, format!("cyber {CURRENT}")).unwrap();
        exe
    }
}

struct Server {
    client: cyber_client::Client,
    stop: Arc<Notify>,
    task: tokio::task::JoinHandle<Result<(), String>>,
}

impl Server {
    async fn events(&self) -> futures::stream::BoxStream<'static, cyber_client::Event> {
        let mut events = self.client.events(false).await.unwrap().boxed();
        let first = events.next().await.unwrap();
        assert_eq!(first.kind, "server.connected");
        events
    }

    /// The next `installation.*` event.
    async fn installation(
        events: &mut futures::stream::BoxStream<'static, cyber_client::Event>,
    ) -> cyber_client::Event {
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let event = events.next().await.expect("the stream is open");
                if event.kind.starts_with("installation.") {
                    return event;
                }
            }
        })
        .await
        .expect("an installation event")
    }

    async fn status(&self) -> Value {
        self.client.get("/service/update").await.unwrap()
    }

    async fn stop(self) {
        self.stop.notify_one();
        tokio::time::timeout(Duration::from_secs(20), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

fn exe_name(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// The archive of `cyber` and its helper, in the running target's format.
fn archive(target: &str, cyber: &[u8], helper: &[u8]) -> Vec<u8> {
    let members = [
        (exe_name("cyber"), cyber),
        (exe_name("cyber-sandbox-exec"), helper),
    ];
    if target.contains("windows") {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, body) in members {
            zip.start_file(name, stored).unwrap();
            zip.write_all(body).unwrap();
        }
        return zip.finish().unwrap().into_inner();
    }
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::none());
    let mut tar = tar::Builder::new(gz);
    for (name, body) in members {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o755);
        tar.append_data(&mut header, name, body).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

/// Publish `version` on the stub: `SHA256SUMS` signed by `signer`, and the archive.
fn publish(stub: &StubRelease, signer: &TestSigner, version: &str, archive: &[u8]) {
    let name = ReleaseSource::archive_name(version, cyber_core::version::build_info().target);
    let digest: String = Sha256::digest(archive)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let sums = format!("{digest}  {name}\n").into_bytes();
    let route = |file: &str, body: Vec<u8>| {
        stub.route(&format!("/v{version}/{file}"), vec![Reply::Ok(body)]);
    };
    route(
        "SHA256SUMS.sigstore.json",
        signer.sign_release(version, &sums),
    );
    route("SHA256SUMS", sums);
    route(&name, archive.to_vec());
}

/// "Notify by default": a minor release is announced on the event stream and by the
/// endpoint, which answered `latest: null` before the check; nothing is downloaded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notify_by_default_publishes_update_available() {
    let f = Fixture::new();
    f.stub.manifest("stable", "1.5.0");
    let exe = f.exe("bin");
    let server = f
        .serve(json!({}), exe.clone(), Duration::from_secs(2))
        .await;
    let mut events = server.events().await;
    let before = server.status().await;
    assert_eq!(before["current"], CURRENT);
    assert_eq!(before["channel"], "stable");
    assert_eq!(before["latest"], Value::Null, "{before}");

    let event = Server::installation(&mut events).await;
    assert_eq!(event.kind, "installation.update_available.1");
    assert_eq!(
        event.data,
        json!({"current": CURRENT, "latest": "1.5.0", "kind": "minor",
            "notes_url": "https://example.test/notes/1.5.0"})
    );
    let status = server.status().await;
    assert_eq!(status["latest"], "1.5.0");
    assert_eq!(status["kind"], "minor");
    assert_eq!(status["method"], "unknown");
    assert_eq!(status["min_supported"], "0.0.1");
    assert_eq!(status["unsupported"], false);
    assert_eq!(status["updated"], false);
    assert!(status["checked_at_ms"].as_u64().is_some(), "{status}");
    let paths: Vec<_> = f.stub.requests().into_iter().map(|r| r.path).collect();
    assert_eq!(paths, ["/channel-stable/stable.json"]);
    assert_eq!(
        std::fs::read_to_string(&exe).unwrap(),
        format!("cyber {CURRENT}")
    );
    server.stop().await;
}

/// A `script` install in `<root>/install` trusting a throwaway signer, with `version`
/// published as the channel's latest.
fn script_install(version: &str) -> (Fixture, TestSigner, PathBuf) {
    let mut f = Fixture::new();
    let signer = TestSigner::new(
        "https://example.test/release.yml@refs/tags/",
        "https://issuer.example.test",
    );
    let trust = f.root.join("trust");
    signer.write_trust_dir(&trust).unwrap();
    let exe = f.exe("install");
    f.env.insert(
        "CYBER_INSTALL_DIR".into(),
        f.root.join("install").display().to_string(),
    );
    f.env.insert(
        "CYBER_TEST_RELEASE_TRUST".into(),
        trust.display().to_string(),
    );
    publish_patch(&f, &signer, version);
    (f, signer, exe)
}

/// `version` on the stub and as the stable manifest, its archive holding
/// `cyber <version>` and `helper <version>`.
fn publish_patch(f: &Fixture, signer: &TestSigner, version: &str) {
    f.stub.manifest("stable", version);
    let target = cyber_core::version::build_info().target;
    let cyber = format!("cyber {version}");
    let helper = format!("helper {version}");
    let body = archive(target, cyber.as_bytes(), helper.as_bytes());
    publish(&f.stub, signer, version, &body);
}

/// `autoupdate: true` on a `script` install: a patch release is verified and installed
/// without asking, and announced as `installation.updated.1`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn silent_patch_upgrade_on_script_install_publishes_updated() {
    let (f, _signer, exe) = script_install("1.4.1");
    let server = f
        .serve(
            json!({"autoupdate": true}),
            exe.clone(),
            Duration::from_secs(2),
        )
        .await;
    let mut events = server.events().await;

    let event = Server::installation(&mut events).await;
    assert_eq!(event.kind, "installation.updated.1", "{:?}", event.data);
    assert_eq!(event.data["latest"], "1.4.1");
    assert_eq!(event.data["kind"], "patch");
    assert_eq!(std::fs::read(&exe).unwrap(), b"cyber 1.4.1");
    let helper = f.root.join("install").join(exe_name("cyber-sandbox-exec"));
    assert_eq!(std::fs::read(helper).unwrap(), b"helper 1.4.1");
    let status = server.status().await;
    assert_eq!(status["method"], "script");
    assert_eq!(status["updated"], true);
    assert_eq!(status["latest"], "1.4.1");
    server.stop().await;
}

/// Regression: a server that already upgraded itself silently runs a build that no
/// longer matches the disk, so the next patch release is announced, never installed
/// over (on Linux the running exe would by then resolve to `cyber.old`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_patch_release_after_a_silent_upgrade_is_only_announced() {
    let (f, signer, exe) = script_install("1.4.1");
    let server = f
        .serve_every(
            json!({"autoupdate": true}),
            exe.clone(),
            Duration::from_millis(500),
            Duration::from_secs(3),
        )
        .await;
    let mut events = server.events().await;
    let first = Server::installation(&mut events).await;
    assert_eq!(first.kind, "installation.updated.1", "{:?}", first.data);
    assert_eq!(std::fs::read(&exe).unwrap(), b"cyber 1.4.1");

    // The next patch, and no fresh cache hiding it from the next check.
    publish_patch(&f, &signer, "1.4.2");
    std::fs::remove_file(f.paths().cache.join("release-stable.json")).unwrap();
    let second = Server::installation(&mut events).await;
    assert_eq!(second.kind, "installation.update_available.1");
    assert_eq!(second.data["latest"], "1.4.2");
    assert_eq!(second.data["kind"], "patch");
    assert_eq!(std::fs::read(&exe).unwrap(), b"cyber 1.4.1");
    let helper = f.root.join("install").join(exe_name("cyber-sandbox-exec"));
    assert_eq!(std::fs::read(helper).unwrap(), b"helper 1.4.1");
    let status = server.status().await;
    assert_eq!(status["latest"], "1.4.2");
    assert_eq!(status["updated"], false);
    let requests = f.stub.requests();
    assert!(
        requests.iter().all(|r| !r.path.starts_with("/v1.4.2/")),
        "{requests:?}"
    );
    server.stop().await;
}

/// Regression: a package manager that lists a cyber package does not show that the
/// running binary came from it. With `autoupdate: true` and a patch release, a binary
/// outside every manager's path is only announced, and the manager is never run.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listing_only_method_is_announced_never_upgraded() {
    use std::os::unix::fs::PermissionsExt;
    let mut f = Fixture::new();
    let managers = f.root.join("managers");
    std::fs::create_dir_all(&managers).unwrap();
    let brew = managers.join("brew");
    std::fs::write(
        &brew,
        "#!/bin/sh\necho \"$@\" >> \"$0.calls\"\n[ \"$1\" = list ] && echo 'cyber 1.4.0'\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&brew, std::fs::Permissions::from_mode(0o755)).unwrap();
    f.env.insert("PATH".into(), managers.display().to_string());
    f.stub.manifest("stable", "1.4.1");
    let exe = f.exe("bin");
    let server = f
        .serve(
            json!({"autoupdate": true}),
            exe.clone(),
            Duration::from_secs(1),
        )
        .await;
    let mut events = server.events().await;

    let event = Server::installation(&mut events).await;
    assert_eq!(
        event.kind, "installation.update_available.1",
        "{:?}",
        event.data
    );
    assert_eq!(event.data["kind"], "patch");
    let status = server.status().await;
    assert_eq!(status["method"], "brew");
    assert_eq!(status["updated"], false);
    let calls = std::fs::read_to_string(managers.join("brew.calls")).unwrap();
    assert_eq!(calls, "list --versions cyber\n");
    assert_eq!(
        std::fs::read_to_string(&exe).unwrap(),
        format!("cyber {CURRENT}")
    );
    server.stop().await;
}

/// Regression: a config that does not load skips the check instead of falling back to
/// the defaults, which would drop an org policy disabling it and ask the source.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unloadable_config_skips_the_check() {
    let f = Fixture::new();
    f.stub.manifest("stable", "1.5.0");
    let server = f
        .serve(json!({}), f.exe("bin"), Duration::from_secs(1))
        .await;
    // Config is read at every check: break it before the first one.
    std::fs::write(f.paths().config.join("cyber.json"), "{ not json").unwrap();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(f.stub.requests().is_empty(), "{:?}", f.stub.requests());
    assert_eq!(server.status().await["latest"], Value::Null);
    server.stop().await;
}

/// "Disabled in CI": with `CI=true` the server never asks the release source.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ci_disables_the_check() {
    let mut f = Fixture::new();
    f.env.insert("CI".into(), "true".into());
    f.stub.manifest("stable", "1.5.0");
    let server = f.serve(json!({}), f.exe("bin"), Duration::ZERO).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(f.stub.requests().is_empty(), "{:?}", f.stub.requests());
    let status = server.status().await;
    assert_eq!(status["latest"], Value::Null, "{status}");
    server.stop().await;
}

/// The minimum supported version is known from the cached manifest before any check
/// (`installation-upgrade` → Minimum supported version), without a request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cached_min_supported_marks_the_build_unsupported_before_a_check() {
    let mut f = Fixture::new();
    f.env.insert("CYBER_OFFLINE".into(), "1".into());
    let cache = f.paths().cache;
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(
        cache.join("release-stable.json"),
        json!({"fetched_at_ms": 1, "manifest": {"version": "9.0.0", "min_supported": "2.0.0"}})
            .to_string(),
    )
    .unwrap();
    let server = f.serve(json!({}), f.exe("bin"), Duration::ZERO).await;
    let status = server.status().await;
    assert_eq!(status["min_supported"], "2.0.0", "{status}");
    assert_eq!(status["unsupported"], true);
    assert_eq!(status["latest"], Value::Null);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(f.stub.requests().is_empty());
    server.stop().await;
}
