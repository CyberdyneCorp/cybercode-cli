//! UserPromptSubmit dispatch runs outside the runtime lifecycle guard. Every hook
//! runner re-enters that guard to admit its receipt, so a shutdown that queues its
//! lifecycle write while a prompt hook is in flight must not deadlock either side.
mod support;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyber_core::config::{self, LoadRequest};
use cyber_core::hooks::{
    HookCatalog, HookDecision, HookDefinition, HookEvent, HookIdentity, HookLocation, HookOutcome,
};
use cyber_core::paths::Paths;
use cyber_server::runtime::*;
use futures::future::BoxFuture;
use serde_json::json;
use support::{Harness, Setup};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const BOUND: Duration = Duration::from_secs(10);

/// Holds UserPromptSubmit dispatch until released, then admits its receipt through
/// `try_start_hook_execution`, the runtime entry every real hook runner uses. The
/// hold stands in for a runner that passed its cancellation check just before
/// shutdown cancelled admission.
struct Host {
    runtime: Mutex<Option<WeakRuntime>>,
    definition: HookDefinition,
    entered: CancellationToken,
    release: CancellationToken,
}

impl Host {
    async fn admit_receipt(&self, hook: LifecycleHook) -> Result<LifecycleVerdict, String> {
        self.entered.cancel();
        self.release.cancelled().await;
        let runtime = self
            .runtime
            .lock()
            .unwrap()
            .as_ref()
            .and_then(WeakRuntime::upgrade)
            .ok_or_else(|| "runtime dropped".to_string())?;
        let identity = HookIdentity {
            session_id: hook.session_id,
            location: HookLocation {
                directory: hook.directory.into(),
                workspace: None,
            },
            project_id: "global".into(),
            agent: hook.agent,
            mode: hook.mode,
        };
        let event = HookEvent::new(hook.kind, identity, 1, hook.fields)?;
        let owner = runtime
            .try_start_hook_execution(&event, &self.definition, false)
            .await
            .map_err(|error| error.to_string())?;
        if let Some(owner) = owner {
            owner
                .finish(HookExecutionResult {
                    outcome: HookOutcome::Ok,
                    decision: HookDecision::default(),
                    acknowledged: true,
                    must_stop: false,
                    io: None,
                })
                .map_err(|error| error.to_string())?;
        }
        Ok(LifecycleVerdict::default())
    }
}

impl ToolHost for Host {
    fn definitions(&self, _: &TurnContext) -> Vec<ToolDef> {
        vec![]
    }
    fn execute(&self, _: Invocation, _: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        Box::pin(async { panic!("No tools") })
    }
    fn durable_user_delegation(&self) -> bool {
        true
    }
    fn lifecycle_hooks(
        &self,
        hook: LifecycleHook,
        _: CancellationToken,
    ) -> BoxFuture<'_, Result<LifecycleVerdict, String>> {
        Box::pin(async move {
            if hook.kind != "UserPromptSubmit" {
                return Ok(LifecycleVerdict::default());
            }
            self.admit_receipt(hook).await
        })
    }
}

/// A real UserPromptSubmit definition, so the admission matches its digest.
fn definition(h: &Harness) -> HookDefinition {
    let env = std::collections::HashMap::from([(
        "CYBER_HOME".into(),
        h.dir.path().join("config").display().to_string(),
    )]);
    let paths = Paths::resolve(&env, h.dir.path());
    paths.ensure().unwrap();
    std::fs::write(
        paths.config.join("cyber.jsonc"),
        json!({"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"cat >/dev/null; sleep 30","id":"slow"}]}]}}).to_string(),
    )
    .unwrap();
    let resolved = config::load(&LoadRequest {
        location: &h.repo,
        paths: &paths,
        env: &env,
        home: h.dir.path(),
        profile: None,
        overrides: &[],
        flags: json!({}),
    })
    .unwrap();
    HookCatalog::from_config(&resolved)
        .unwrap()
        .definitions
        .remove(0)
}

async fn fixture() -> (Harness, Runtime, Arc<Host>, String) {
    let h = Harness::new(Setup::default());
    let host = Arc::new(Host {
        runtime: Mutex::new(None),
        definition: definition(&h),
        entered: CancellationToken::new(),
        release: CancellationToken::new(),
    });
    let runtime = Runtime::new(RuntimeOptions {
        store: h.store.clone(),
        resolver: h.models.clone(),
        tools: host.clone(),
        global_config_dir: h.dir.path().join("global"),
        shell: "zsh".into(),
        claude_compat: true,
        compaction: CompactionConfig::default(),
        retry: cyber_llm::RetryPolicy::default(),
        max_steps: None,
        today: Some("2026-10-12".into()),
        interactive: false,
        snapshots: Arc::new(NoSnapshots),
    });
    *host.runtime.lock().unwrap() = Some(runtime.downgrade());
    let session = runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    (h, runtime, host, session)
}

/// Queue shutdown while the prompt hook is held, then release it: both the
/// admission and shutdown must finish, and the admission must be refused.
async fn shutdown_during_prompt_hook<T: std::fmt::Debug + Send + 'static>(
    runtime: &Runtime,
    host: &Host,
    admission: JoinHandle<Result<T, RuntimeError>>,
) {
    tokio::time::timeout(BOUND, host.entered.cancelled())
        .await
        .expect("UserPromptSubmit was dispatched");
    let closing = tokio::spawn({
        let runtime = runtime.clone();
        async move { runtime.shutdown().await }
    });
    // Let shutdown cancel admission and queue its lifecycle write first.
    tokio::time::sleep(Duration::from_millis(100)).await;
    host.release.cancel();
    let admitted = tokio::time::timeout(BOUND, admission)
        .await
        .expect("admission deadlocked behind a queued shutdown")
        .unwrap();
    assert!(admitted.is_err(), "admitted during shutdown: {admitted:?}");
    tokio::time::timeout(BOUND, closing)
        .await
        .expect("shutdown deadlocked behind an in-flight prompt hook")
        .unwrap();
}

#[tokio::test]
async fn shutdown_during_user_prompt_submit_does_not_deadlock_prompt_admission() {
    let (_h, runtime, host, session) = fixture().await;
    let admission = tokio::spawn({
        let runtime = runtime.clone();
        let session = session.clone();
        async move {
            runtime
                .admit_user(&session, Admission::text("hello", Delivery::Queue))
                .await
        }
    });
    shutdown_during_prompt_hook(&runtime, &host, admission).await;
    let state = runtime.state(&session).await.unwrap();
    assert!(state.inbox.is_empty(), "{:?}", state.inbox);
}

#[tokio::test]
async fn shutdown_during_user_prompt_submit_does_not_deadlock_delegation() {
    let (_h, runtime, host, parent) = fixture().await;
    let admission = tokio::spawn({
        let runtime = runtime.clone();
        let parent = parent.clone();
        async move {
            runtime
                .start_delegation(
                    &parent,
                    "op_prompt_hook",
                    UserSubtask {
                        skill_command: None,
                        admission_id: None,
                        prompt: "inspect project".into(),
                        agent: None,
                        attachments: vec![],
                        max_steps: None,
                        hook_context: None,
                    },
                )
                .await
        }
    });
    shutdown_during_prompt_hook(&runtime, &host, admission).await;
    assert!(
        runtime
            .delegation(&parent, "op_prompt_hook")
            .unwrap()
            .is_none()
    );
}
