//! Command hooks at the built-in tool boundary. Other lifecycle owners follow separately.
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};

use futures::{FutureExt, StreamExt, stream};
use std::time::Instant;

use cyber_core::config::Resolved;
use cyber_core::hooks::{
    HookAction, HookCatalog, HookDecision, HookDefinition, HookEvent, HookIdentity, HookLocation,
    HookOutcome,
};
use cyber_core::trust::TrustStore;
use cyber_server::runtime::{HookExecutionResult, Invocation, ToolOutcome};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::BuiltinHost;
use crate::hook_commands::{HookCommandReport, HookCommandRunner};
use crate::tools::ToolError;

pub type HookConfigFn = dyn Fn(&Path) -> Result<Resolved, String> + Send + Sync;
pub(crate) struct HooksConfig {
    resolve: Arc<HookConfigFn>,
    trust: TrustStore,
}

impl BuiltinHost {
    /// Bind a live full resolver once; each event reloads config and exact trust.
    pub fn attach_hook_config(
        &self,
        resolve: Arc<HookConfigFn>,
        trust: TrustStore,
    ) -> Result<(), String> {
        self.hook_config
            .set(HooksConfig { resolve, trust })
            .map_err(|_| "Hook configuration resolver is already attached".into())
    }

    /// Reload definitions and exact approvals without dispatching handlers.
    pub fn review_hooks(&self, location: &Path) -> Result<cyber_core::hooks::HookReview, String> {
        let config = self
            .hook_config
            .get()
            .ok_or("Hook configuration resolver is unavailable")?;
        cyber_core::hooks::HookReview::from_config(&(config.resolve)(location)?, &config.trust)
    }

    /// Approve only an exact, currently resolved checkout-scoped definition.
    pub fn trust_hook(&self, location: &Path, digest: &str) -> Result<(), String> {
        let config = self
            .hook_config
            .get()
            .ok_or("Hook configuration resolver is unavailable")?;
        let resolved = (config.resolve)(location)?;
        if !resolved.trust.trusted {
            return Err("Trust the current checkout configuration before approving hooks".into());
        }
        let catalog = HookCatalog::from_config(&resolved)?;
        if !catalog.definitions.iter().any(|definition| {
            definition.digest == digest && definition.scope.requires_handler_trust()
        }) {
            return Err("Digest does not identify a currently resolved project/local hook".into());
        }
        config
            .trust
            .approve_hook(&resolved.trust.checkout_root, digest)
            .map_err(|error| error.to_string())
    }

    /// Revoke obsolete approvals without loading potentially malformed configuration.
    pub fn untrust_hook(&self, location: &Path, digest: &str) -> Result<bool, String> {
        let config = self
            .hook_config
            .get()
            .ok_or("Hook configuration resolver is unavailable")?;
        let root = cyber_core::config::project_root(location);
        config
            .trust
            .revoke_hook(&root, digest)
            .map_err(|error| error.to_string())
    }

    pub(crate) async fn pre_tool_hooks(
        &self,
        inv: &mut Invocation,
        schema: &Value,
        cancel: CancellationToken,
    ) -> Result<Option<HookAction>, ToolError> {
        if self.hook_config.get().is_none() {
            return Ok(None);
        }
        let event = event("PreToolUse", inv, Value::Null)?;
        let decision = self.dispatch_tool_hooks(event, &inv.name, cancel).await?;
        if matches!(
            decision.decision,
            Some(HookAction::Deny | HookAction::Block)
        ) {
            return Err(ToolError::Failed(format!(
                "blocked by hook {}",
                decision.reason.as_deref().unwrap_or("policy")
            )));
        }
        if let Some(input) = decision.updated_input {
            crate::schema::validate(schema, &input)
                .map_err(|_| ToolError::Failed("hook produced invalid tool input".into()))?;
            inv.asker
                .validate_auto_override(&inv.name, &input)
                .map_err(|error| ToolError::Failed(error.to_string()))?;
            inv.input = input;
        }
        Ok(decision.decision)
    }

    pub(crate) async fn permission_request_hooks(
        &self,
        inv: &Invocation,
        ask: &cyber_server::runtime::PermissionAsk,
        cancel: CancellationToken,
    ) -> Result<Option<HookAction>, ToolError> {
        if self.hook_config.get().is_none() {
            return Ok(None);
        }
        let event = event("PermissionRequest", inv, json!({"permission":ask}))?;
        let decision = self.dispatch_tool_hooks(event, &inv.name, cancel).await?;
        if matches!(
            decision.decision,
            Some(HookAction::Deny | HookAction::Block)
        ) {
            return Err(ToolError::Failed(format!(
                "blocked by hook {}",
                decision.reason.as_deref().unwrap_or("policy")
            )));
        }
        Ok(decision.decision)
    }

    pub(crate) async fn post_tool_hooks(
        &self,
        inv: &Invocation,
        outcome: &ToolOutcome,
        started: Instant,
        cancel: CancellationToken,
    ) -> Result<(), ToolError> {
        let (kind, output) = match outcome {
            ToolOutcome::Ok(output) | ToolOutcome::Structured { output, .. } => {
                ("PostToolUse", output)
            }
            ToolOutcome::Failed(error) | ToolOutcome::Crashed(error) => {
                ("PostToolUseFailure", error)
            }
            ToolOutcome::Aborted => return Ok(()),
        };
        let event = event(
            kind,
            inv,
            json!({"tool_output":output,"duration_ms":u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)}),
        )?;
        self.dispatch_tool_hooks(event, &inv.name, cancel)
            .await
            .map(|_| ())
    }

    async fn dispatch_tool_hooks(
        &self,
        mut event: HookEvent,
        subject: &str,
        cancel: CancellationToken,
    ) -> Result<HookDecision, ToolError> {
        let Some(config) = self.hook_config.get() else {
            return Ok(HookDecision::default());
        };
        let resolved =
            (config.resolve)(&event.identity().location.directory).map_err(ToolError::Failed)?;
        let catalog = HookCatalog::from_config(&resolved).map_err(ToolError::Failed)?;
        let Some(runtime) = self.runtime() else {
            if catalog
                .definitions
                .iter()
                .any(|definition| definition.event == event.event())
            {
                return Err(ToolError::Failed(
                    "Hook execution requires runtime ownership".into(),
                ));
            }
            return Ok(HookDecision::default());
        };
        let credentials = self
            .opts
            .models
            .as_ref()
            .map(|models| models.credential_env_names())
            .unwrap_or_default();
        let runner = HookCommandRunner {
            resolved: &resolved,
            trust: &config.trust,
            invocation_trust: None,
            home: &self.opts.home,
            temp_dir: &self.opts.temp_dir,
            shell: &self.opts.shell,
            helper: self.opts.sandbox_helper.as_deref(),
            credential_env_names: &credentials,
        };
        if !matches!(event.event(), "PreToolUse" | "PermissionDenied") {
            return Box::pin(
                self.dispatch_fixed_hooks(&runner, &runtime, &catalog, &event, subject, cancel),
            )
            .await;
        }
        let mut decision = HookDecision::default();
        let commands = Mutex::new(HashSet::new());
        for definition in catalog.definitions {
            if cancel.is_cancelled() {
                return Err(ToolError::Aborted);
            }
            let paths = target_paths(&event, &self.opts.home);
            if !definition.matches_event(&event, subject, &paths) {
                continue;
            }
            let Some(report) = self
                .run_matching_hook(
                    &runner,
                    &runtime,
                    &definition,
                    &event,
                    &commands,
                    cancel.clone(),
                )
                .await?
            else {
                continue;
            };
            self.observe_hook_report(&event, &definition, &report)?;
            merge_decision(&definition, report, &mut decision);
            if let Some(input) = decision.updated_input.clone() {
                event = event.with_tool_input(input).map_err(ToolError::Failed)?;
            }
        }
        Ok(decision)
    }
    async fn dispatch_fixed_hooks(
        &self,
        runner: &HookCommandRunner<'_>,
        runtime: &cyber_server::runtime::Runtime,
        catalog: &HookCatalog,
        event: &HookEvent,
        subject: &str,
        cancel: CancellationToken,
    ) -> Result<HookDecision, ToolError> {
        let stop = cancel.child_token();
        let commands = Mutex::new(HashSet::new());
        let failure = Mutex::new(None);
        let paths = target_paths(event, &self.opts.home);
        let definitions = catalog
            .definitions
            .iter()
            .filter(|definition| definition.matches_event(event, subject, &paths));
        let mut executions = Vec::new();
        for (index, definition) in definitions.enumerate() {
            let stop = &stop;
            let commands = &commands;
            let failure = &failure;
            executions.push(
                async move {
                    let result = self
                        .run_matching_hook(
                            runner,
                            runtime,
                            definition,
                            event,
                            commands,
                            stop.clone(),
                        )
                        .await
                        .and_then(|report| {
                            if let Some(report) = &report {
                                self.observe_hook_report(event, definition, report)?;
                            }
                            Ok(report)
                        });
                    if let Err(error) = &result {
                        let mut first = failure.lock().unwrap_or_else(|poison| poison.into_inner());
                        if first.is_none() {
                            *first = Some(error.clone());
                            stop.cancel();
                        }
                    }
                    (index, definition, result)
                }
                .boxed(),
            );
        }
        let mut reports = stream::iter(executions)
            .buffer_unordered(catalog.settings.concurrency)
            .collect::<Vec<_>>()
            .await;
        // Drain all launched siblings before returning; never drop their native owners
        // merely because an earlier ordered result refused admission.
        if let Some(error) = failure
            .into_inner()
            .unwrap_or_else(|poison| poison.into_inner())
        {
            return Err(error);
        }
        reports.sort_by_key(|(index, _, _)| *index);
        let mut decision = HookDecision::default();
        for (_, definition, report) in reports {
            if let Some(report) = report? {
                merge_decision(definition, report, &mut decision);
            }
        }
        Ok(decision)
    }

    async fn run_matching_hook(
        &self,
        runner: &HookCommandRunner<'_>,
        runtime: &cyber_server::runtime::Runtime,
        definition: &HookDefinition,
        event: &HookEvent,
        commands: &Mutex<HashSet<String>>,
        cancel: CancellationToken,
    ) -> Result<Option<HookCommandReport>, ToolError> {
        if cancel.is_cancelled() {
            return Err(ToolError::Aborted);
        }
        let trusted = definition
            .is_trusted(&runner.resolved.trust.checkout_root, runner.trust, None)
            .map_err(|error| ToolError::Failed(error.to_string()))?;
        if !trusted {
            self.hook_notice(event, "untrusted hook skipped", &definition.digest);
            return Ok(None);
        }
        let result = if definition.kind() != cyber_core::config::HookKind::Command
            || definition.handler.asynchronous
        {
            let owner = runtime
                .start_hook_execution(event, definition, false)
                .await
                .map_err(|error| ToolError::Failed(error.to_string()))?;
            owner
                .finish(HookExecutionResult {
                    outcome: HookOutcome::Error,
                    decision: HookDecision::default(),
                    acknowledged: true,
                    must_stop: false,
                    io: None,
                })
                .map_err(|error| ToolError::Failed(error.to_string()))?;
            Err("Hook handler type or async scheduling is not implemented".to_string())
        } else {
            if let Some(command) = &definition.handler.command
                && !commands
                    .lock()
                    .map_err(|_| ToolError::Failed("Hook command scheduling lock poisoned".into()))?
                    .insert(command.clone())
            {
                return Ok(None);
            }
            runner
                .run_recorded(runtime, &definition.pointer, event, cancel)
                .await
        };
        match result {
            Ok(report) => Ok(Some(report)),
            Err(error) => {
                self.hook_notice(event, &error, &definition.digest);
                if definition.handler.fail_closed {
                    return Err(ToolError::Failed(format!(
                        "hook {} unavailable",
                        definition
                            .handler
                            .id
                            .as_deref()
                            .unwrap_or(&definition.digest)
                    )));
                }
                Ok(None)
            }
        }
    }

    fn observe_hook_report(
        &self,
        event: &HookEvent,
        definition: &HookDefinition,
        report: &HookCommandReport,
    ) -> Result<(), ToolError> {
        if report.must_stop {
            return Err(ToolError::Aborted);
        }
        if let Some(diagnostic) = &report.diagnostic {
            self.hook_notice(event, diagnostic, &definition.digest);
        }
        if report.decision.additional_context.is_some()
            || report.decision.continuation == Some(false)
        {
            self.hook_notice(
                event,
                "hook context/continuation admission remains unavailable",
                &definition.digest,
            );
            if definition.handler.fail_closed {
                return Err(ToolError::Failed(
                    "hook context/continuation admission unavailable".into(),
                ));
            }
        }
        if report.outcome == HookOutcome::Blocked {
            self.hook_notice(
                event,
                report.decision.reason.as_deref().unwrap_or("blocked"),
                definition
                    .handler
                    .id
                    .as_deref()
                    .unwrap_or(&definition.digest),
            );
        }
        if report.decision.updated_input.is_some() {
            self.hook_notice(
                event,
                "hook rewrote tool input",
                definition
                    .handler
                    .id
                    .as_deref()
                    .unwrap_or(&definition.digest),
            );
        }
        Ok(())
    }
}

pub(crate) fn merge_decision(
    definition: &HookDefinition,
    mut report: HookCommandReport,
    decision: &mut HookDecision,
) {
    if matches!(
        report.decision.decision,
        Some(HookAction::Deny | HookAction::Block)
    ) {
        report.decision.reason = Some(format!(
            "{}: {}",
            definition
                .handler
                .id
                .as_deref()
                .unwrap_or(&definition.digest),
            report.decision.reason.as_deref().unwrap_or("policy")
        ));
    }
    decision.merge(report.decision);
}

fn event(kind: &str, inv: &Invocation, extra: Value) -> Result<HookEvent, ToolError> {
    let mut fields = json!({"tool_name":inv.name,"tool_input":inv.input,"call_id":inv.call_id})
        .as_object()
        .expect("object")
        .clone();
    if let Some(extra) = extra.as_object() {
        fields.extend(extra.clone());
    }
    HookEvent::new(
        kind,
        HookIdentity {
            session_id: inv.session_id.clone(),
            location: HookLocation {
                directory: inv.directory.clone().into(),
                workspace: None,
            },
            project_id: "global".into(),
            agent: inv.agent.clone(),
            mode: inv.mode.clone(),
        },
        i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|error| ToolError::Failed(error.to_string()))?
                .as_millis(),
        )
        .map_err(|error| ToolError::Failed(error.to_string()))?,
        fields,
    )
    .map_err(ToolError::Failed)
}

pub(crate) fn target_paths(event: &HookEvent, home: &Path) -> Vec<String> {
    let input = if event.event() == "FileChanged" {
        event.as_json()
    } else {
        &event.as_json()["tool_input"]
    };
    let location = crate::host::canonical(&event.identity().location.directory);
    ["file_path", "path"]
        .into_iter()
        .filter_map(|key| input[key].as_str())
        .filter_map(|path| {
            let target = crate::host::canonical(&crate::host::resolve_path(
                &event.identity().location.directory,
                home,
                path,
            ));
            target
                .strip_prefix(&location)
                .ok()
                .map(crate::permissions::slash)
        })
        .collect()
}

impl BuiltinHost {
    fn hook_notice(&self, event: &HookEvent, message: &str, digest: &str) {
        if let Some(runtime) = self.runtime() {
            runtime.hook_notice(&event.identity().session_id, digest, message);
        }
    }
}
