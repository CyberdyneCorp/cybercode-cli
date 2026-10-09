//! Synthetic commands own receipts and native checkout activity without Sessions.
#![cfg(unix)]
mod support;

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use cyber_core::config::{self, LoadRequest, Resolved};
use cyber_core::hooks::{HookEvent, HookLocation, HookOutcome};
use cyber_core::paths::Paths;
use cyber_core::trust::TrustStore;
use cyber_core::worktrees::{
    CheckoutActivity, GitExecution, GitFuture, Managed, Name, Repository, Settings,
};
use cyber_server::runtime::{HookExecutionRecord, HookExecutionStatus};
use cyber_tools::hook_commands::{HookCommandReport, HookCommandRunner};
use serde_json::json;
use tokio_util::sync::CancellationToken;

const POINTER: &str = "/hooks/PreToolUse/0/hooks/0";

struct TestRun {
    fixture: support::Fixture,
    resolved: Resolved,
    trust: TrustStore,
    event: HookEvent,
    temp: PathBuf,
}

impl TestRun {
    fn new(command: &str, once: bool) -> Self {
        Self::at(support::Fixture::new(), None, command, once)
    }

    fn at(fixture: support::Fixture, location: Option<&Path>, command: &str, once: bool) -> Self {
        let location = location.unwrap_or(&fixture.repo);
        let env = HashMap::from([(
            "CYBER_HOME".into(),
            fixture.dir.path().join("cyber").display().to_string(),
        )]);
        let paths = Paths::resolve(&env, fixture.dir.path());
        paths.ensure().unwrap();
        std::fs::write(paths.config.join("cyber.jsonc"), json!({
            "sandbox":{"policy":"full-access","network":"off"},
            "hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":command,"once":once}]}]}
        }).to_string()).unwrap();
        let resolved = config::load(&LoadRequest {
            location,
            paths: &paths,
            env: &env,
            home: fixture.dir.path(),
            profile: None,
            overrides: &[],
            flags: json!({}),
        })
        .unwrap();
        fixture.set_config(resolved.value.clone());
        let event = HookEvent::synthetic(
            "PreToolUse",
            HookLocation {
                directory: location.into(),
                workspace: None,
            },
            "global".into(),
            "build".into(),
            "bypass".into(),
            1,
            json!({"tool_name":"bash","tool_input":{"command":"private synthetic request"}}),
        )
        .unwrap();
        let temp = fixture.dir.path().join("hook-scratch");
        Self {
            fixture,
            resolved,
            trust: TrustStore::new(paths.trust_file()),
            event,
            temp,
        }
    }

    fn runner(&self) -> HookCommandRunner<'_> {
        HookCommandRunner {
            resolved: &self.resolved,
            trust: &self.trust,
            invocation_trust: None,
            home: self.fixture.dir.path(),
            temp_dir: &self.temp,
            shell: "/bin/sh",
            helper: None,
            credential_env_names: &[],
        }
    }

    fn receipts(&self) -> Vec<HookExecutionRecord> {
        self.fixture
            .store
            .read(|conn| {
                let mut query =
                    conn.prepare("SELECT data FROM hook_test_execution ORDER BY started_ms,id")?;
                let rows = query.query_map([], |row| row.get::<_, String>(0))?;
                rows.map(|row| Ok(serde_json::from_str(&row?).unwrap()))
                    .collect()
            })
            .unwrap()
    }

    fn assert_no_sessions(&self) {
        let counts: (i64, i64) = self
            .fixture
            .store
            .read(|conn| {
                Ok((
                    conn.query_row("SELECT count(*) FROM session", [], |row| row.get(0))?,
                    conn.query_row("SELECT count(*) FROM hook_execution", [], |row| row.get(0))?,
                ))
            })
            .unwrap();
        assert_eq!(counts, (0, 0));
    }
}

#[tokio::test]
async fn synthetic_command_receipt_is_private_and_once_is_per_invocation() {
    let f = TestRun::new(
        "cat >/dev/null; printf '{\"decision\":\"deny\",\"reason\":\"test policy\"}'",
        true,
    );
    let runner = f.runner();
    let result = runner
        .run_test(&f.fixture.host, POINTER, &f.event, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.outcome, HookOutcome::Blocked);
    let skipped = runner
        .run_test(&f.fixture.host, POINTER, &f.event, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(skipped.outcome, HookOutcome::Skipped);
    let records = f.receipts();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, HookExecutionStatus::Completed);
    assert_eq!(records[0].acknowledged, Some(true));
    assert!(records[0].synthetic && records[0].io.is_none());
    assert_eq!(
        records[0].decision.as_ref().unwrap().reason.as_deref(),
        Some("test policy")
    );
    let facts = f
        .fixture
        .store
        .read_events(&records[0].id, -1, 10)
        .unwrap()
        .events;
    assert_eq!(facts.len(), 2);
    assert!(
        !serde_json::to_string(&facts)
            .unwrap()
            .contains("private synthetic request")
    );
    f.assert_no_sessions();
}

#[tokio::test]
async fn cancelled_before_admission_does_not_record_or_launch() {
    let f = TestRun::new("printf effect > effect", false);
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        f.runner()
            .run_test(&f.fixture.host, POINTER, &f.event, cancel)
            .await
            .is_err()
    );
    assert!(f.receipts().is_empty());
    assert!(!f.fixture.repo.join("effect").exists());
    f.assert_no_sessions();
}

#[tokio::test]
async fn ordinary_event_cannot_enter_the_synthetic_effect_path() {
    let f = TestRun::new("printf effect > effect", false);
    assert!(
        f.runner()
            .run(POINTER, &f.event, CancellationToken::new())
            .await
            .is_err()
    );
    let ordinary = HookEvent::new(
        "PreToolUse",
        f.event.identity().clone(),
        1,
        Default::default(),
    )
    .unwrap();
    let error = f
        .runner()
        .run_test(
            &f.fixture.host,
            POINTER,
            &ordinary,
            CancellationToken::new(),
        )
        .await
        .err()
        .unwrap();
    assert!(error.contains("synthetic event"));
    assert!(f.receipts().is_empty());
    assert!(!f.fixture.repo.join("effect").exists());
    f.assert_no_sessions();
}

#[tokio::test]
async fn opted_in_synthetic_io_captures_event_and_command_streams() {
    let mut f = TestRun::new("cat; printf 'private hook stderr' >&2", false);
    f.resolved.value["telemetry"] = json!({"log_hook_io":true});
    let result = f
        .runner()
        .run_test(&f.fixture.host, POINTER, &f.event, CancellationToken::new())
        .await
        .unwrap();
    assert!(result.acknowledged);
    let records = f.receipts();
    let io = records[0].io.as_ref().unwrap();
    assert!(io.stdin.contains("private synthetic request"));
    assert!(io.stdout.contains("private synthetic request"));
    assert_eq!(io.stderr, "private hook stderr");
    assert!(!io.truncated);
    f.assert_no_sessions();
}

async fn wait_started<F: std::future::Future<Output = Result<HookCommandReport, String>>>(
    directory: &Path,
    running: &mut std::pin::Pin<Box<F>>,
) {
    let marker = directory.join("started");
    tokio::select! {
        result = running => panic!("hook ended before observation: {}", result.is_ok()),
        result = tokio::time::timeout(Duration::from_secs(5), async {
            while !marker.exists() { tokio::time::sleep(Duration::from_millis(10)).await; }
        }) => result.unwrap(),
    }
}

struct Git;
impl GitExecution for Git {
    fn run<'a>(&'a self, directory: &'a Path, args: &'a [OsString]) -> GitFuture<'a> {
        Box::pin(async move {
            Command::new("git")
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .current_dir(directory)
                .args(args)
                .output()
        })
    }
}

async fn managed() -> (support::Fixture, Repository, Managed) {
    let f = support::Fixture::new();
    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.name", "Hook test"],
        vec!["config", "user.email", "hook@example.invalid"],
    ] {
        assert!(
            Git.run(
                &f.repo,
                &args.into_iter().map(OsString::from).collect::<Vec<_>>()
            )
            .await
            .unwrap()
            .status
            .success()
        );
    }
    f.write("tracked.txt", "base");
    for args in [
        vec!["add", "tracked.txt"],
        vec!["commit", "--quiet", "-m", "initial"],
    ] {
        assert!(
            Git.run(
                &f.repo,
                &args.into_iter().map(OsString::from).collect::<Vec<_>>()
            )
            .await
            .unwrap()
            .status
            .success()
        );
    }
    let repo = Repository::discover(&Git, &f.repo).await.unwrap();
    let managed = repo
        .create(
            &Git,
            &Settings::default(),
            f.dir.path(),
            "prj_test",
            &Name::parse("hook").unwrap(),
        )
        .await
        .unwrap();
    (f, repo, managed)
}

#[tokio::test]
async fn managed_test_blocks_removal_until_cancel_acknowledges_native_tree() {
    let (fixture, repository, managed) = managed().await;
    let f = TestRun::at(
        fixture,
        Some(&managed.path),
        "printf started > started; sleep 30",
        false,
    );
    let runner = f.runner();
    let cancel = CancellationToken::new();
    let mut running = Box::pin(runner.run_test(&f.fixture.host, POINTER, &f.event, cancel.clone()));
    wait_started(&managed.path, &mut running).await;
    let mut remove = Box::pin(repository.remove(&Git, &CheckoutActivity, &managed, true));
    tokio::select! {
        result = &mut running => panic!("hook ended while checkout was owned: {}", result.is_ok()),
        result = &mut remove => assert!(result.unwrap_err().to_string().contains("in use")),
    }
    drop(remove);
    cancel.cancel();
    let report = running.await.unwrap();
    assert!(report.acknowledged && report.must_stop);
    assert_eq!(f.receipts()[0].status, HookExecutionStatus::Completed);
    repository
        .remove(&Git, &CheckoutActivity, &managed, true)
        .await
        .unwrap();
    f.assert_no_sessions();
}

#[tokio::test]
async fn disposed_managed_test_keeps_unknown_receipt_and_removal_fence() {
    let (fixture, repository, managed) = managed().await;
    let f = TestRun::at(
        fixture,
        Some(&managed.path),
        "printf started > started; sleep 30",
        false,
    );
    let runner = f.runner();
    let mut running =
        Box::pin(runner.run_test(&f.fixture.host, POINTER, &f.event, CancellationToken::new()));
    wait_started(&managed.path, &mut running).await;
    drop(running);
    let record = f.receipts().remove(0);
    assert_eq!(record.status, HookExecutionStatus::Unknown);
    assert_eq!(record.acknowledged, Some(false));
    assert!(record.must_stop);
    let error = repository
        .remove(&Git, &CheckoutActivity, &managed, true)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("unknown"), "{error}");
    assert!(managed.path.join("started").exists());
    f.assert_no_sessions();
}

#[tokio::test]
async fn concurrent_handlers_share_test_identity_but_retain_distinct_checkout_leases() {
    let (fixture, repository, managed) = managed().await;
    let f = TestRun::at(
        fixture,
        Some(&managed.path),
        "cat >/dev/null; printf 'running\n' >> starts; while [ $(wc -l < starts) -lt 2 ]; do sleep 0.01; done",
        false,
    );
    let runner = f.runner();
    let (one, two) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            runner.run_test(&f.fixture.host, POINTER, &f.event, CancellationToken::new()),
            runner.run_test(&f.fixture.host, POINTER, &f.event, CancellationToken::new()),
        )
    })
    .await
    .unwrap();
    assert_eq!(one.unwrap().outcome, HookOutcome::Ok);
    assert_eq!(two.unwrap().outcome, HookOutcome::Ok);
    let records = f.receipts();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].session_id, records[1].session_id);
    repository
        .remove(&Git, &CheckoutActivity, &managed, true)
        .await
        .unwrap();
    f.assert_no_sessions();
}

#[tokio::test]
async fn synthetic_timeout_includes_waiting_for_checkout_admission() {
    let (fixture, repository, managed) = managed().await;
    let mut f = TestRun::at(fixture, Some(&managed.path), "touch must-not-run", false);
    f.resolved.value["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"] = json!(1);
    let lock = cyber_core::worktrees::RepositoryLock::try_acquire(&managed.common_dir)
        .unwrap()
        .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        f.runner()
            .run_test(&f.fixture.host, POINTER, &f.event, CancellationToken::new()),
    )
    .await
    .unwrap();
    assert!(result.unwrap_err().contains("checkout admission timed out"));
    assert!(!managed.path.join("must-not-run").exists());
    assert_eq!(f.receipts()[0].status, HookExecutionStatus::Unknown);
    drop(lock);
    repository
        .remove(&Git, &CheckoutActivity, &managed, true)
        .await
        .unwrap();
    f.assert_no_sessions();
}
