mod support;

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::Mutex;
#[cfg(unix)]
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

#[derive(Default)]
struct Sink {
    output: Mutex<Vec<u8>>,
    ready: Notify,
    fail: bool,
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
