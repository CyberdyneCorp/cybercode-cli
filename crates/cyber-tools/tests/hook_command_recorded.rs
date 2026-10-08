//! Real command execution under durable runtime/native cancellation ownership.
#![cfg(unix)]
mod support;

use cyber_core::config::{self, LoadRequest, Resolved};
use cyber_core::hooks::{HookEvent, HookIdentity, HookLocation, HookOutcome};
use cyber_core::paths::Paths;
use cyber_core::trust::TrustStore;
use cyber_server::runtime::{HookExecutionRecord, HookExecutionStatus, SubtreeStopStatus};
use cyber_tools::hook_commands::{HookCommandReport, HookCommandRunner};
use serde_json::json;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use support::flow::Flow;
use tokio_util::sync::CancellationToken;

const POINTER: &str = "/hooks/PreToolUse/0/hooks/0";

struct Recorded {
    flow: Flow,
    session: String,
    resolved: Resolved,
    event: HookEvent,
    trust: TrustStore,
    temp: PathBuf,
}

impl Recorded {
    async fn new(command: &str) -> Self {
        let flow = Flow::new(Vec::new(), false);
        let session = flow.session("default").await;
        let env = std::collections::HashMap::from([(
            "CYBER_HOME".into(),
            flow.f.dir.path().join("config").display().to_string(),
        )]);
        let paths = Paths::resolve(&env, flow.f.dir.path());
        paths.ensure().unwrap();
        std::fs::write(paths.config.join("cyber.jsonc"),json!({"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":command,"id":"guard"}]}]}}).to_string()).unwrap();
        let resolved = config::load(&LoadRequest {
            location: &flow.f.repo,
            paths: &paths,
            env: &env,
            home: flow.f.dir.path(),
            profile: None,
            overrides: &[],
            flags: json!({}),
        })
        .unwrap();
        let state = flow.runtime.state(&session).await.unwrap();
        let event = HookEvent::new(
            "PreToolUse",
            HookIdentity {
                session_id: session.clone(),
                location: HookLocation {
                    directory: state.info.directory.into(),
                    workspace: None,
                },
                project_id: "global".into(),
                agent: state.info.agent,
                mode: state.info.mode,
            },
            1,
            json!({"tool_input":{"command":"private request"}})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap();
        let trust = TrustStore::new(paths.trust_file());
        let temp = flow.f.dir.path().join("hook-scratch");
        Self {
            flow,
            session,
            resolved,
            event,
            trust,
            temp,
        }
    }
    fn runner(&self) -> HookCommandRunner<'_> {
        HookCommandRunner {
            resolved: &self.resolved,
            trust: &self.trust,
            invocation_trust: None,
            home: self.flow.f.dir.path(),
            temp_dir: &self.temp,
            shell: "/bin/sh",
            helper: None,
            credential_env_names: &[],
        }
    }
    fn receipt(&self) -> HookExecutionRecord {
        self.flow
            .runtime
            .hook_executions(&self.session, 10)
            .unwrap()
            .remove(0)
    }
    async fn wait_started<F: Future<Output = Result<HookCommandReport, String>>>(
        &self,
        running: &mut Pin<Box<F>>,
    ) {
        let marker = self.flow.f.repo.join("started");
        tokio::select! {
            result=running => panic!("hook ended before stop: {}",result.is_ok()),
            _=tokio::time::timeout(std::time::Duration::from_secs(5),async {
                while !marker.exists() {tokio::time::sleep(std::time::Duration::from_millis(10)).await;}
            }) => assert!(marker.exists()),
        }
        assert_eq!(self.receipt().status, HookExecutionStatus::Running);
    }
}

#[tokio::test]
async fn actual_command_decision_has_a_durable_privacy_preserving_receipt() {
    let f = Recorded::new("cat >/dev/null; printf '{\"decision\":\"deny\",\"reason\":\"policy\"}'")
        .await;
    let report = f
        .runner()
        .run_recorded(&f.flow.runtime, POINTER, &f.event, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(report.outcome, HookOutcome::Blocked);
    let receipt = f.receipt();
    assert_eq!(receipt.status, HookExecutionStatus::Completed);
    assert_eq!(receipt.outcome, Some(HookOutcome::Blocked));
    assert!(receipt.io.is_none());
    assert_eq!(receipt.decision.unwrap().reason.as_deref(), Some("policy"));
    let events = f
        .flow
        .f
        .store
        .read_events(&receipt.id, -1, 10)
        .unwrap()
        .events;
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].kind, "hook.executed.1");
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("private request")
    );
    f.flow.runtime.shutdown().await;
}

#[tokio::test]
async fn subtree_stop_cancels_owned_hook_and_waits_for_native_acknowledgement() {
    let f = Recorded::new("printf running > started; sleep 30").await;
    let runner = f.runner();
    let mut running =
        Box::pin(runner.run_recorded(&f.flow.runtime, POINTER, &f.event, CancellationToken::new()));
    f.wait_started(&mut running).await;
    let (result, stopped) = tokio::join!(running, f.flow.runtime.stop_subtree(&f.session));
    let report = result.unwrap();
    assert_eq!(report.outcome, HookOutcome::Skipped);
    assert!(report.acknowledged && report.must_stop);
    assert_eq!(stopped.unwrap().status, SubtreeStopStatus::Acknowledged);
    let receipt = f.receipt();
    assert_eq!(receipt.status, HookExecutionStatus::Completed);
    assert_eq!(receipt.outcome, Some(HookOutcome::Skipped));
    assert!(receipt.must_stop);
    assert_eq!(std::fs::read_dir(&f.temp).unwrap().count(), 0);
    f.flow.runtime.shutdown().await;
}

#[tokio::test]
async fn disposed_running_hook_preserves_unknown_native_and_receipt_evidence() {
    let f = Recorded::new("printf running > started; sleep 30").await;
    let runner = f.runner();
    let mut running =
        Box::pin(runner.run_recorded(&f.flow.runtime, POINTER, &f.event, CancellationToken::new()));
    f.wait_started(&mut running).await;
    drop(running);
    let receipt = f.receipt();
    assert_eq!(receipt.status, HookExecutionStatus::Unknown);
    assert_eq!(receipt.acknowledged, Some(false));
    assert!(receipt.must_stop);
    assert_eq!(
        f.flow
            .runtime
            .stop_subtree(&f.session)
            .await
            .unwrap()
            .status,
        SubtreeStopStatus::Unknown
    );
    assert_eq!(std::fs::read_dir(&f.temp).unwrap().count(), 1);
    f.flow.runtime.shutdown().await;
}
