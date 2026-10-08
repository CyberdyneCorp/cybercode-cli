//! Real loaded configuration, fresh trust checks and command sandbox boundaries.
use std::collections::HashMap;
use std::path::PathBuf;

use cyber_core::config::{self, LoadRequest, Resolved};
#[cfg(unix)]
use cyber_core::hooks::HookAction;
use cyber_core::hooks::{HookCatalog, HookEvent, HookIdentity, HookLocation};
use cyber_core::paths::Paths;
use cyber_core::trust::{HookInvocationTrust, TrustStore};
use cyber_tools::hook_commands::HookCommandRunner;
#[cfg(unix)]
use cyber_tools::hook_commands::HookOutcome;
#[cfg(unix)]
use serde_json::Value;
use serde_json::json;
use tokio_util::sync::CancellationToken;

const POINTER: &str = "/hooks/PreToolUse/0/hooks/0";

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    location: PathBuf,
    home: PathBuf,
    temp: PathBuf,
    paths: Paths,
    env: HashMap<String, String>,
    trust: TrustStore,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        let root = base.join("repo");
        let location = root.join("nested");
        let home = base.join("home");
        let temp = base.join("scratch");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(&location).unwrap();
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        let env = HashMap::from([(
            "CYBER_HOME".into(),
            base.join("cyber").display().to_string(),
        )]);
        let paths = Paths::resolve(&env, &home);
        paths.ensure().unwrap();
        let trust = TrustStore::new(paths.trust_file());
        Self {
            _dir: dir,
            root,
            location,
            home,
            temp,
            paths,
            env,
            trust,
        }
    }
    fn write(&self, project: bool, command: &str, sandbox_all: bool) {
        let file = if project {
            self.root.join("cyber.jsonc")
        } else {
            self.paths.config.join("cyber.jsonc")
        };
        std::fs::write(file, json!({
            "sandbox":{"policy":"full-access","network":"off"},
            "hooks":{"sandbox_all":sandbox_all,"PreToolUse":[{"hooks":[{"type":"command","command":command,"id":"guard"}]}]}
        }).to_string()).unwrap();
    }
    fn load(&self) -> Resolved {
        config::load(&LoadRequest {
            location: &self.location,
            paths: &self.paths,
            env: &self.env,
            home: &self.home,
            profile: None,
            overrides: &[],
            flags: json!({}),
        })
        .unwrap()
    }
    fn approve_workspace(&self) -> Resolved {
        let resolved = self.load();
        self.trust
            .approve(
                &resolved.trust.checkout_root,
                resolved.trust.digest.as_ref().unwrap(),
            )
            .unwrap();
        self.load()
    }
    fn event(&self) -> HookEvent {
        HookEvent::new(
            "PreToolUse",
            HookIdentity {
                session_id: "ses_launch".into(),
                location: HookLocation {
                    directory: self.location.clone(),
                    workspace: None,
                },
                project_id: "global".into(),
                agent: "coder".into(),
                mode: "bypass".into(),
            },
            1,
            json!({"tool_name":"bash","tool_input":{"command":"reviewed"}})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap()
    }
    fn runner<'a>(&'a self, resolved: &'a Resolved) -> HookCommandRunner<'a> {
        HookCommandRunner {
            resolved,
            trust: &self.trust,
            invocation_trust: None,
            home: &self.home,
            temp_dir: &self.temp,
            shell: "/bin/sh",
            helper: None,
            credential_env_names: &[],
        }
    }
    fn digest(&self, resolved: &Resolved) -> String {
        HookCatalog::from_config(resolved).unwrap().definitions[0]
            .digest
            .clone()
    }
}

#[tokio::test]
async fn loaded_command_launch_rechecks_checkout_and_handler_approvals() {
    let f = Fixture::new();
    f.write(true, "printf unsafe > effect", false);
    let withheld = f.load();
    assert!(
        f.runner(&withheld)
            .run(POINTER, &f.event(), CancellationToken::new())
            .await
            .err()
            .unwrap()
            .contains("absent")
    );
    let resolved = f.approve_workspace();
    let digest = f.digest(&resolved);
    assert!(
        f.runner(&resolved)
            .run(POINTER, &f.event(), CancellationToken::new())
            .await
            .err()
            .unwrap()
            .contains("digest is untrusted")
    );
    f.trust.approve_hook(&f.root, &digest).unwrap();
    f.trust.revoke_hook(&f.root, &digest).unwrap();
    assert!(
        f.runner(&resolved)
            .run(POINTER, &f.event(), CancellationToken::new())
            .await
            .err()
            .unwrap()
            .contains("digest is untrusted")
    );
    f.trust.approve_hook(&f.root, &digest).unwrap();
    f.trust.revoke(&f.root).unwrap();
    assert!(
        f.runner(&resolved)
            .run(POINTER, &f.event(), CancellationToken::new())
            .await
            .err()
            .unwrap()
            .contains("configuration is untrusted")
    );
    assert!(!f.location.join("effect").exists());
    assert!(!f.temp.exists());
}

#[tokio::test]
async fn changed_definitions_and_foreign_event_locations_refuse_before_effects() {
    let f = Fixture::new();
    f.write(true, "printf reviewed > effect", false);
    let resolved = f.approve_workspace();
    f.trust.approve_hook(&f.root, &f.digest(&resolved)).unwrap();
    f.write(true, "printf changed > effect", false);
    let changed = f.approve_workspace();
    assert!(
        f.runner(&changed)
            .run(POINTER, &f.event(), CancellationToken::new())
            .await
            .err()
            .unwrap()
            .contains("digest is untrusted")
    );
    let other = tempfile::tempdir().unwrap();
    let foreign = HookEvent::new(
        "PreToolUse",
        HookIdentity {
            session_id: "ses_foreign".into(),
            location: HookLocation {
                directory: std::fs::canonicalize(other.path()).unwrap(),
                workspace: None,
            },
            project_id: "global".into(),
            agent: "coder".into(),
            mode: "default".into(),
        },
        1,
        serde_json::Map::new(),
    )
    .unwrap();
    assert!(
        f.runner(&changed)
            .run(POINTER, &foreign, CancellationToken::new())
            .await
            .err()
            .unwrap()
            .contains("Location does not match")
    );
    assert!(!f.location.join("effect").exists());
}

#[tokio::test]
async fn pre_cancelled_and_mismatched_event_commands_never_launch() {
    let f = Fixture::new();
    f.write(false, "printf unsafe > effect", false);
    let resolved = f.load();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        f.runner(&resolved)
            .run(POINTER, &f.event(), cancel)
            .await
            .err()
            .unwrap()
            .contains("cancelled before launch")
    );
    let wrong = HookEvent::new(
        "Stop",
        f.event().identity().clone(),
        1,
        serde_json::Map::new(),
    )
    .unwrap();
    assert!(
        f.runner(&resolved)
            .run(POINTER, &wrong, CancellationToken::new())
            .await
            .err()
            .unwrap()
            .contains("event does not match")
    );
    assert!(!f.location.join("effect").exists());
    assert!(!f.temp.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn global_command_receives_event_stdin_location_and_captured_environment() {
    let f = Fixture::new();
    f.write(false,r#"IFS= read -r event; printf '%s' "$event" > event.json; printf '{"decision":"deny","additional_context":"%s|%s|%s|%s|%s"}' "$CYBER_PROJECT_DIR" "$CYBER_SESSION_ID" "$CYBER_HOOK_EVENT" "$CYBER_AGENT" "$PWD""#,false);
    let resolved = f.load();
    let report = f
        .runner(&resolved)
        .run(POINTER, &f.event(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(report.outcome, HookOutcome::Blocked);
    assert_eq!(report.decision.decision, Some(HookAction::Deny));
    assert_eq!(
        report.decision.additional_context.as_deref(),
        Some(
            format!(
                "{}|ses_launch|PreToolUse|coder|{}",
                f.root.display(),
                f.location.display()
            )
            .as_str()
        )
    );
    let input: Value =
        serde_json::from_slice(&std::fs::read(f.location.join("event.json")).unwrap()).unwrap();
    assert_eq!(input, *f.event().as_json());
    assert_eq!(std::fs::read_dir(&f.temp).unwrap().count(), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn mandatory_hook_sandbox_masks_credentials_and_denies_home_writes() {
    let f = Fixture::new();
    let target = f.home.join(".ssh/config");
    std::fs::write(&target, "original").unwrap();
    let quoted = format!("'{}'", target.display().to_string().replace('\'', "'\\''"));
    f.write(true, &format!("printf stolen > {quoted}"), false);
    let resolved = f.approve_workspace();
    f.trust.approve_hook(&f.root, &f.digest(&resolved)).unwrap();
    let result = f
        .runner(&resolved)
        .run(POINTER, &f.event(), CancellationToken::new())
        .await;
    if cyber_sandbox::available() {
        let report = result.unwrap();
        // Shells may return exit 2 for a refused redirection; the hook contract
        // treats that as blocked, while other nonzero exits are errors.
        assert!(
            matches!(report.outcome, HookOutcome::Error | HookOutcome::Blocked),
            "{:?}",
            report.outcome
        );
    } else {
        assert!(result.is_err());
    }
    assert_eq!(std::fs::read_to_string(target).unwrap(), "original");
    let f = Fixture::new();
    f.write(
        false,
        "printf '{\"additional_context\":\"%s\"}' \"${HOME-masked}\"",
        true,
    );
    let resolved = f.load();
    let credentials = ["HOME".into()];
    let mut runner = f.runner(&resolved);
    runner.credential_env_names = &credentials;
    let result = runner
        .run(POINTER, &f.event(), CancellationToken::new())
        .await;
    if cyber_sandbox::available() {
        let report = result.unwrap();
        assert_eq!(report.outcome, HookOutcome::Ok);
        assert_eq!(
            report.decision.additional_context.as_deref(),
            Some("masked")
        );
    } else {
        assert!(result.is_err());
    }
}

#[cfg(windows)]
#[tokio::test]
async fn windows_missing_helper_refuses_before_process_creation() {
    let f = Fixture::new();
    f.write(false, "echo unsafe > effect", false);
    let resolved = f.load();
    assert!(
        f.runner(&resolved)
            .run(POINTER, &f.event(), CancellationToken::new())
            .await
            .err()
            .unwrap()
            .contains("trusted process helper")
    );
    assert!(!f.location.join("effect").exists());
    assert!(!f.temp.exists());
}

#[cfg(windows)]
#[tokio::test]
async fn windows_global_command_receives_event_input_and_required_sandbox_refuses() {
    let f = Fixture::new();
    let helper = cyber_sandbox::find_helper().expect("native test requires the trusted helper");
    f.write(false, "$e = [Console]::In.ReadToEnd() | ConvertFrom-Json; @{additional_context=($e.session_id + ':' + $env:CYBER_HOOK_EVENT)} | ConvertTo-Json -Compress", false);
    let resolved = f.load();
    let mut runner = f.runner(&resolved);
    runner.helper = Some(&helper);
    let report = runner
        .run(POINTER, &f.event(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(report.outcome, cyber_tools::hook_commands::HookOutcome::Ok);
    assert_eq!(
        report.decision.additional_context.as_deref(),
        Some("ses_launch:PreToolUse")
    );
    assert_eq!(std::fs::read_dir(&f.temp).unwrap().count(), 0);
    f.write(false, "Set-Content effect unsafe", true);
    let resolved = f.load();
    let mut runner = f.runner(&resolved);
    runner.helper = Some(&helper);
    assert!(
        runner
            .run(POINTER, &f.event(), CancellationToken::new())
            .await
            .is_err()
    );
    assert!(!f.location.join("effect").exists());
}

#[tokio::test]
async fn invocation_approval_never_substitutes_for_checkout_approval() {
    let f = Fixture::new();
    f.write(true, "printf unsafe > effect", false);
    let resolved = f.approve_workspace();
    let invocation = HookInvocationTrust::new(&f.root, &[f.digest(&resolved)]).unwrap();
    f.trust.revoke(&f.root).unwrap();
    let mut runner = f.runner(&resolved);
    runner.invocation_trust = Some(&invocation);
    assert!(
        runner
            .run(POINTER, &f.event(), CancellationToken::new())
            .await
            .err()
            .unwrap()
            .contains("configuration is untrusted")
    );
    assert!(!f.temp.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn approved_project_command_writes_workspace_with_invocation_only_handler_trust() {
    let f = Fixture::new();
    f.write(
        true,
        "printf settled > effect; printf '{\"decision\":\"allow\"}'",
        false,
    );
    let resolved = f.approve_workspace();
    let digest = f.digest(&resolved);
    let invocation = HookInvocationTrust::new(&f.root, std::slice::from_ref(&digest)).unwrap();
    let mut runner = f.runner(&resolved);
    runner.invocation_trust = Some(&invocation);
    let result = runner
        .run(POINTER, &f.event(), CancellationToken::new())
        .await;
    if cyber_sandbox::available() {
        let report = result.unwrap();
        assert_eq!(report.outcome, HookOutcome::Ok);
        assert_eq!(report.decision.decision, Some(HookAction::Allow));
        assert_eq!(
            std::fs::read_to_string(f.location.join("effect")).unwrap(),
            "settled"
        );
        assert_eq!(std::fs::read_dir(&f.temp).unwrap().count(), 0);
    } else {
        assert!(result.is_err());
        assert!(!f.location.join("effect").exists());
    }
    assert!(!f.trust.is_hook_approved(&f.root, &digest).unwrap());
}

#[cfg(unix)]
#[tokio::test]
async fn disposed_execution_retains_scratch_while_acknowledged_cancellation_cleans_it() {
    let f = Fixture::new();
    f.write(false, "printf running > started; sleep 30", false);
    let resolved = f.load();
    let runner = f.runner(&resolved);
    let event = f.event();
    let mut running = Box::pin(runner.run(POINTER, &event, CancellationToken::new()));
    tokio::select! {
        result = &mut running => panic!("command settled before disposal: {}", result.is_ok()),
        _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !f.location.join("started").exists() {tokio::time::sleep(std::time::Duration::from_millis(10)).await;}
        }) => assert!(f.location.join("started").exists()),
    }
    drop(running);
    assert_eq!(std::fs::read_dir(&f.temp).unwrap().count(), 1);
    std::fs::remove_file(f.location.join("started")).unwrap();
    let cancel = CancellationToken::new();
    let mut running = Box::pin(runner.run(POINTER, &event, cancel.clone()));
    tokio::select! {
        result = &mut running => panic!("command settled before cancellation: {}", result.is_ok()),
        _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !f.location.join("started").exists() {tokio::time::sleep(std::time::Duration::from_millis(10)).await;}
        }) => assert!(f.location.join("started").exists()),
    }
    cancel.cancel();
    let report = running.await.unwrap();
    assert_eq!(report.outcome, HookOutcome::Skipped);
    assert!(report.must_stop && report.acknowledged);
    // Only the unacknowledged disposal's scratch remains.
    assert_eq!(std::fs::read_dir(&f.temp).unwrap().count(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn once_command_requires_recorded_admission_before_any_effect() {
    let f = Fixture::new();
    f.write(false, "printf unsafe > effect", false);
    let file = f.paths.config.join("cyber.jsonc");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    value["hooks"]["PreToolUse"][0]["hooks"][0]["once"] = json!(true);
    std::fs::write(file, value.to_string()).unwrap();
    let resolved = f.load();
    let error = f
        .runner(&resolved)
        .run(POINTER, &f.event(), CancellationToken::new())
        .await
        .err()
        .unwrap();
    assert!(
        error.contains("Once hooks require durable runtime admission"),
        "{error}"
    );
    assert!(!f.location.join("effect").exists());
    assert!(!f.temp.exists());
}
