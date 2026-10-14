//! Background (`async: true`) handlers. Admission is durable before the event goes
//! on; the handler then runs in a runtime-owned task the event never waits for.
//! Decisions are ignored except `additional_context`, which is queued for the
//! Session's next Safe Boundary, and failures only leave a notice.
use std::path::PathBuf;

use cyber_core::config::{HookKind, Resolved};
use cyber_core::hooks::{HookDecision, HookDefinition, HookEvent};
use cyber_core::trust::{HookInvocationTrust, TrustStore};
use cyber_server::runtime::Runtime;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::BuiltinHost;
use crate::hook_commands::{HookCommandReport, HookCommandRunner};
use crate::hook_reports::Admitted;

/// Owned launch settings, so the handler can outlive the dispatching event.
struct BackgroundHook {
    resolved: Resolved,
    trust: TrustStore,
    invocation: Option<HookInvocationTrust>,
    home: PathBuf,
    temp_dir: PathBuf,
    shell: String,
    helper: Option<PathBuf>,
    credentials: Vec<String>,
    kind: HookKind,
    pointer: String,
    event: HookEvent,
}

impl BackgroundHook {
    fn capture(
        runner: &HookCommandRunner<'_>,
        definition: &HookDefinition,
        event: &HookEvent,
    ) -> Self {
        Self {
            resolved: runner.resolved.clone(),
            trust: runner.trust.clone(),
            invocation: runner.invocation_trust.cloned(),
            home: runner.home.to_path_buf(),
            temp_dir: runner.temp_dir.to_path_buf(),
            shell: runner.shell.to_owned(),
            helper: runner.helper.map(PathBuf::from),
            credentials: runner.credential_env_names.to_vec(),
            kind: definition.kind(),
            pointer: definition.pointer.clone(),
            event: event.clone(),
        }
    }

    fn runner(&self) -> HookCommandRunner<'_> {
        HookCommandRunner {
            resolved: &self.resolved,
            trust: &self.trust,
            invocation_trust: self.invocation.as_ref(),
            home: &self.home,
            temp_dir: &self.temp_dir,
            shell: &self.shell,
            helper: self.helper.as_deref(),
            credential_env_names: &self.credentials,
        }
    }

    fn http(&self) -> crate::hook_http::HookHttpRunner<'_> {
        crate::hook_http::HookHttpRunner {
            resolved: &self.resolved,
            trust: &self.trust,
            invocation_trust: self.invocation.as_ref(),
            home: &self.home,
        }
    }

    fn prompt(&self) -> crate::hook_prompt::HookPromptRunner<'_> {
        crate::hook_prompt::HookPromptRunner {
            resolved: &self.resolved,
            trust: &self.trust,
            invocation_trust: self.invocation.as_ref(),
            home: &self.home,
        }
    }

    async fn admit(
        &self,
        host: &BuiltinHost,
        runtime: &Runtime,
    ) -> Result<Option<Admitted>, String> {
        let (pointer, event, live) = (&self.pointer, &self.event, &CancellationToken::new());
        match self.kind {
            HookKind::Command => self.runner().admit(runtime, pointer, event, live).await,
            HookKind::Http => self.http().admit(runtime, pointer, event, live).await,
            HookKind::Prompt => self.prompt().admit(runtime, pointer, event, live).await,
            HookKind::McpTool => {
                let settings = self.runner();
                crate::hook_mcp::HookMcpRunner {
                    settings: &settings,
                }
                .admit(host, runtime, pointer, event, live)
                .await
            }
        }
    }

    /// Run to settlement. Only the execution's own activity cancels it.
    async fn run(
        &self,
        host: &BuiltinHost,
        runtime: &Runtime,
        admitted: Admitted,
    ) -> Result<HookCommandReport, String> {
        let (pointer, event, live) = (&self.pointer, &self.event, CancellationToken::new());
        match self.kind {
            HookKind::Command => {
                self.runner()
                    .run_admitted(runtime, admitted, pointer, event, live)
                    .await
            }
            HookKind::Http => {
                self.http()
                    .run_admitted(runtime, admitted, pointer, event, live)
                    .await
            }
            HookKind::Prompt => {
                self.prompt()
                    .run_admitted(runtime, admitted, pointer, event, live)
                    .await
            }
            HookKind::McpTool => {
                let settings = self.runner();
                crate::hook_mcp::HookMcpRunner {
                    settings: &settings,
                }
                .run_admitted(host, runtime, admitted, pointer, event, live)
                .await
            }
        }
    }
}

impl BuiltinHost {
    /// Admit an async handler with its event, then hand it to a runtime-owned task.
    /// Errors are notices only: a background handler never blocks its event, even
    /// when `fail_closed`.
    pub(crate) async fn start_background_hook(
        &self,
        runner: &HookCommandRunner<'_>,
        runtime: &Runtime,
        definition: &HookDefinition,
        event: &HookEvent,
    ) -> Result<(), String> {
        let host = self.weak.upgrade().ok_or("Hook host unavailable")?;
        let hook = BackgroundHook::capture(runner, definition, event);
        let Some(Admitted { owner, definition }) = hook.admit(self, runtime).await? else {
            return Ok(());
        };
        let task_runtime = runtime.clone();
        runtime
            .spawn_hook_execution(owner, move |owner| {
                background(hook, host, task_runtime, Admitted { owner, definition })
            })
            .map_err(|error| error.to_string())
    }

    async fn settle_background_hook(
        &self,
        runtime: &Runtime,
        event: &HookEvent,
        definition: &HookDefinition,
        result: Result<HookCommandReport, String>,
    ) {
        let report = match result {
            Ok(report) => report,
            Err(error) => {
                runtime.hook_notice(&event.identity().session_id, &definition.digest, &error);
                return;
            }
        };
        let id = definition
            .handler
            .id
            .as_deref()
            .unwrap_or(&definition.digest);
        if let Some(diagnostic) = &report.diagnostic {
            runtime.hook_notice(&event.identity().session_id, &definition.digest, diagnostic);
        }
        let HookDecision {
            additional_context, ..
        } = report.decision.clone();
        if ignores_decision(&report.decision) {
            cyber_core::log::debug(
                "hooks",
                "async hook decision ignored",
                json!({"hook_id":id,"event":event.event()}),
            );
        }
        let Some(context) = additional_context else {
            return;
        };
        let session = &event.identity().session_id;
        if let Err(error) = runtime
            .queue_hook_context(session, format!("[hook {id}] {context}"))
            .await
        {
            runtime.hook_notice(session, &definition.digest, &error.to_string());
        }
    }
}

/// The background task, boxed with a declared type: handler futures can reach
/// dispatch again, so their auto traits must not be inferred through this one.
fn background(
    hook: BackgroundHook,
    host: std::sync::Arc<BuiltinHost>,
    runtime: Runtime,
    admitted: Admitted,
) -> futures::future::BoxFuture<'static, ()> {
    Box::pin(async move {
        let definition = admitted.definition.clone();
        let result = hook.run(&host, &runtime, admitted).await;
        host.settle_background_hook(&runtime, &hook.event, &definition, result)
            .await;
    })
}

/// Anything besides context, which is the only part of an async decision applied.
fn ignores_decision(decision: &HookDecision) -> bool {
    decision.decision.is_some()
        || decision.updated_input.is_some()
        || decision.continuation.is_some()
        || decision.stop_reason.is_some()
}
