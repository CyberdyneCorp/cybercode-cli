//! Command hooks at the built-in tool boundary; lifecycle_hooks reuses this dispatch.
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use futures::{FutureExt, StreamExt, stream};
use std::time::Instant;

use cyber_core::config::{HookKind, Resolved};
use cyber_core::hooks::{
    HookAction, HookCatalog, HookDecision, HookDefinition, HookEvent, HookIdentity, HookOutcome,
};
use cyber_core::trust::{HookInvocationTrust, TrustStore};
use cyber_server::runtime::{HookEffects, Invocation, ToolOutcome};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::BuiltinHost;
use crate::hook_commands::{HookCommandReport, HookCommandRunner};
use crate::tools::ToolError;

pub type HookConfigFn = dyn Fn(&Path) -> Result<Resolved, String> + Send + Sync;
/// Tool-event hook effects awaiting settlement, keyed by (Session, call).
pub(crate) type PendingEffects = Mutex<HashMap<(String, String), HookEffects>>;
pub(crate) struct HooksConfig {
    pub(crate) resolve: Arc<HookConfigFn>,
    pub(crate) trust: TrustStore,
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

    /// Merge connected clients' hook handlers after every configured handler
    /// (`client-sdk` → Hook callbacks).
    pub fn attach_client_hooks(
        &self,
        hooks: Arc<dyn cyber_server::runtime::ClientHooks>,
    ) -> Result<(), String> {
        self.client_hooks
            .set(hooks)
            .map_err(|_| "Client hooks are already attached".into())
    }

    /// Approve exactly these inspected project/local handler digests for this host's
    /// lifetime (`cyber exec --trust-project-hooks`). Nothing is persisted, and a
    /// changed definition has a different digest and stays unapproved.
    pub fn attach_invocation_hook_trust(&self, trust: HookInvocationTrust) -> Result<(), String> {
        self.hook_invocation_trust
            .set(trust)
            .map_err(|_| "Invocation hook approval is already attached".into())
    }

    /// The effects tool-event hooks recorded for a call, taken once at settlement.
    pub(crate) fn take_tool_hook_effects(&self, session_id: &str, call_id: &str) -> HookEffects {
        self.hook_effects
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&(session_id.to_owned(), call_id.to_owned()))
            .unwrap_or_default()
    }

    /// Reload definitions and exact approvals without dispatching handlers.
    pub fn review_hooks(&self, location: &Path) -> Result<cyber_core::hooks::HookReview, String> {
        let config = self
            .hook_config
            .get()
            .ok_or("Hook configuration resolver is unavailable")?;
        cyber_core::hooks::HookReview::from_config(&(config.resolve)(location)?, &config.trust)
    }

    /// Every plugin the Location's scopes list, with its state (`plugins-marketplace` →
    /// Plugin CLI, `/plugins`), from the same resolution hooks and MCP use.
    pub fn plugins(&self, location: &Path) -> Result<cyber_core::plugins::PluginList, String> {
        let config = self
            .hook_config
            .get()
            .ok_or("Plugin configuration resolver is unavailable")?;
        let mut resolved = (config.resolve)(location)?;
        self.plugin_host_status(location, &mut resolved.plugins);
        Ok(cyber_core::plugins::PluginList {
            plugins: resolved.plugins,
            pure: resolved.pure,
        })
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
        let decision = self
            .dispatch_tool_event("PreToolUse", inv, Value::Null, cancel)
            .await?;
        self.record_hook_effects(inv, &decision);
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
            if inv.name == "computer" {
                return Err(ToolError::Failed("native computer input cannot be rewritten by a hook; request a fresh reviewed action".into()));
            }
            crate::schema::validate(schema, &input)
                .map_err(|_| ToolError::Failed("hook produced invalid tool input".into()))?;
            inv.asker
                .validate_auto_override(&inv.name, &input)
                .map_err(|error| ToolError::Failed(error.to_string()))?;
            inv.input = input;
            self.record_rewrite(inv);
        }
        Ok(decision.decision)
    }

    /// Whether `PreToolUse` hooks let a host-issued `bash` `command` run unchanged on
    /// the Session's behalf. Any block, ask, rewrite or hook failure refuses it (fail
    /// closed); the hook's effects are not recorded on the Session's own call.
    pub(crate) async fn hooks_admit_host_bash(
        &self,
        inv: &Invocation,
        command: &str,
        cancel: CancellationToken,
    ) -> bool {
        if self.hook_config.get().is_none() {
            return true;
        }
        let mut probe = inv.clone();
        probe.name = "bash".into();
        probe.input = json!({"command": command});
        match self
            .dispatch_tool_event("PreToolUse", &probe, Value::Null, cancel)
            .await
        {
            Ok(decision) => {
                decision.updated_input.is_none()
                    && matches!(decision.decision, None | Some(HookAction::Allow))
            }
            Err(_) => false,
        }
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
        let decision = self
            .dispatch_tool_event("PermissionRequest", inv, json!({"permission":ask}), cancel)
            .await?;
        self.record_hook_effects(inv, &decision);
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

    /// PermissionDenied at a deny-rule, Mode-ceiling, auto-block or user-reject
    /// boundary. A merged `retry` returns the input to re-issue (its `updated_input`
    /// or the denied input); the caller re-validates and re-evaluates it once.
    pub(crate) async fn permission_denied_hooks(
        &self,
        inv: &Invocation,
        denial: &PermissionDenial<'_>,
        cancel: CancellationToken,
    ) -> Result<Option<Value>, ToolError> {
        if self.hook_config.get().is_none() {
            return Ok(None);
        }
        let fields = json!({
            "reason": denial.reason,
            "denied_by": denial.source,
            "permission": {"action": denial.action, "resources": denial.resources},
        });
        let decision = self
            .dispatch_tool_event("PermissionDenied", inv, fields, cancel)
            .await?;
        self.record_hook_effects(inv, &decision);
        Ok((decision.decision == Some(HookAction::Retry))
            .then(|| decision.updated_input.unwrap_or_else(|| inv.input.clone())))
    }

    /// Notification hooks for a user-facing prompt about to be published. They are
    /// observational: decisions are ignored and failures never block the prompt.
    pub(crate) async fn notification_hooks(
        &self,
        inv: &Invocation,
        kind: &str,
        message: String,
        cancel: CancellationToken,
    ) {
        if self.hook_config.get().is_none() || !inv.asker.attended() {
            return;
        }
        let fields = json!({"notification_type":kind,"message":message,"tool_name":inv.name,"call_id":inv.call_id});
        let fields = fields.as_object().cloned().unwrap_or_default();
        if let Err(ToolError::Failed(error)) = self
            .dispatch_owned("Notification", &HookOwner::of(inv), fields, kind, cancel)
            .await
        {
            cyber_core::log::debug(
                "hooks",
                "Notification hook failed",
                json!({"error":error,"session_id":inv.session_id}),
            );
        }
    }

    pub(crate) async fn post_tool_hooks(
        &self,
        inv: &Invocation,
        outcome: &ToolOutcome,
        started: Instant,
        cancel: CancellationToken,
    ) -> Result<(), ToolError> {
        let (kind, output) = match outcome {
            ToolOutcome::Ok(output)
            | ToolOutcome::SkillLoaded { output, .. }
            | ToolOutcome::Structured { output, .. }
            | ToolOutcome::SkillSuggestions {
                failed: false,
                output,
                ..
            } => ("PostToolUse", output),
            ToolOutcome::Failed(error)
            | ToolOutcome::Crashed(error)
            | ToolOutcome::SkillSuggestions {
                failed: true,
                output: error,
                ..
            } => ("PostToolUseFailure", error),
            ToolOutcome::Aborted => return Ok(()),
        };
        let duration = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let decision = self
            .dispatch_tool_event(
                kind,
                inv,
                json!({"tool_output":output,"duration_ms":duration}),
                cancel,
            )
            .await?;
        self.record_hook_effects(inv, &decision);
        Ok(())
    }

    /// Keep a tool event's context and stop request for the call's settlement.
    fn record_hook_effects(&self, inv: &Invocation, decision: &HookDecision) {
        if decision.additional_context.is_none() && decision.continuation != Some(false) {
            return;
        }
        self.hook_effects
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry((inv.session_id.clone(), inv.call_id.clone()))
            .or_default()
            .absorb(decision);
    }

    /// Keep an accepted PreToolUse rewrite for the call's settlement, so the
    /// transcript records the input that actually ran.
    fn record_rewrite(&self, inv: &Invocation) {
        self.hook_effects
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry((inv.session_id.clone(), inv.call_id.clone()))
            .or_default()
            .record_rewrite(rewrite_notice(&inv.name, &inv.input));
    }

    /// Reload configuration and definitions for the owner's Location, with the hook
    /// handlers of the clients that own its Session.
    async fn hook_catalog_at(
        &self,
        owner: &HookOwner,
    ) -> Result<Option<(Resolved, HookCatalog)>, ToolError> {
        let Some(config) = self.hook_config.get() else {
            return Ok(None);
        };
        let resolved = (config.resolve)(Path::new(&owner.directory)).map_err(ToolError::Failed)?;
        let catalog = HookCatalog::from_config(&resolved).map_err(ToolError::Failed)?;
        let client = self.client_definitions(&owner.session_id).await;
        Ok(Some((resolved, catalog.with_client(client))))
    }

    /// Definitions of the live client registrations that apply to `session_id`: those of
    /// the clients owning it or an ancestor, in registration order.
    pub(crate) async fn client_definitions(&self, session_id: &str) -> Vec<HookDefinition> {
        let (Some(hooks), Some(runtime)) = (self.client_hooks.get(), self.runtime()) else {
            return Vec::new();
        };
        let Ok(lineage) = runtime.lineage(session_id).await else {
            return Vec::new();
        };
        hooks
            .client_hooks(&lineage)
            .into_iter()
            .filter_map(|hook| {
                HookDefinition::client(&hook.event, &hook.id, hook.matcher, hook.timeout).ok()
            })
            .collect()
    }

    /// Dispatch an owner's event. Configuration reloads for its Location, and only
    /// when a definition exists for `kind` does the runtime resolve the Session's
    /// envelope identity and the handlers run. None means nothing was configured;
    /// otherwise the merged decision and the catalog's Stop continuation cap.
    /// The future is boxed with a declared `Send` type: handlers (MCP elicitation,
    /// background runs) can reach dispatch again, so auto traits must not be
    /// inferred through this recursion.
    pub(crate) fn dispatch_owned<'a>(
        &'a self,
        kind: &'a str,
        owner: &'a HookOwner,
        fields: serde_json::Map<String, Value>,
        subject: &'a str,
        cancel: CancellationToken,
    ) -> futures::future::BoxFuture<'a, Result<Option<(HookDecision, usize)>, ToolError>> {
        Box::pin(async move {
            let Some((resolved, catalog)) = self.hook_catalog_at(owner).await? else {
                return Ok(None);
            };
            if !catalog
                .definitions
                .iter()
                .chain(catalog.skipped_for(kind))
                .any(|definition| definition.event == kind)
            {
                return Ok(None);
            }
            let identity = self.hook_identity(owner).await?;
            let event = lifecycle_event(kind, identity, fields)?;
            let cap = catalog.settings.max_stop_continuations;
            let decision = self
                .dispatch_catalog(event, subject, &resolved, catalog, cancel)
                .await?;
            Ok(Some((decision, cap)))
        })
    }

    pub(crate) async fn dispatch_catalog(
        &self,
        mut event: HookEvent,
        subject: &str,
        resolved: &Resolved,
        catalog: HookCatalog,
        cancel: CancellationToken,
    ) -> Result<HookDecision, ToolError> {
        // Nothing to dispatch: skip the runner, credentials and receipt path entirely.
        if !crate::lifecycle_hooks::configured(&catalog, &event) {
            return Ok(HookDecision::default());
        }
        let config = self.hook_config.get().ok_or_else(|| {
            ToolError::Failed("Hook configuration resolver is unavailable".into())
        })?;
        let Some(runtime) = self.runtime() else {
            return Err(ToolError::Failed(
                "Hook execution requires runtime ownership".into(),
            ));
        };
        let credentials = self
            .opts
            .models
            .as_ref()
            .map(|models| models.credential_env_names())
            .unwrap_or_default();
        let runner = HookCommandRunner {
            resolved,
            trust: &config.trust,
            invocation_trust: self.hook_invocation_trust.get(),
            home: &self.opts.home,
            temp_dir: &self.opts.temp_dir,
            shell: &self.opts.shell,
            helper: self.opts.sandbox_helper.as_deref(),
            credential_env_names: &credentials,
        };
        self.report_policy_skips(&runtime, &catalog, &event, subject)
            .await;
        if !catalog
            .definitions
            .iter()
            .any(|definition| definition.event == event.event())
        {
            return Ok(HookDecision::default());
        }
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
            let report = crate::lifecycle_hooks::mergeable_report(&event, &definition, report);
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
                let report = crate::lifecycle_hooks::mergeable_report(event, definition, report);
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
            .is_trusted(
                &runner.resolved.trust.checkout_root,
                runner.trust,
                runner.invocation_trust,
            )
            .map_err(|error| ToolError::Failed(error.to_string()))?;
        if !trusted {
            self.untrusted_hook_skipped(runner, runtime, definition, event)
                .await;
            return Ok(None);
        }
        if definition.handler.asynchronous {
            // The event never waits for, or fails closed on, a background handler.
            if let Err(error) = self
                .start_background_hook(runner, runtime, definition, event)
                .await
            {
                self.hook_notice(event, &error, &definition.digest);
            }
            return Ok(None);
        }
        if let Some(command) = &definition.handler.command
            && !commands
                .lock()
                .map_err(|_| ToolError::Failed("Hook command scheduling lock poisoned".into()))?
                .insert(command.clone())
        {
            return Ok(None);
        }
        let result = self
            .run_handler(runner, runtime, definition, event, cancel)
            .await;
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

    /// Run one synchronous handler through its kind's recorded runner.
    async fn run_handler(
        &self,
        runner: &HookCommandRunner<'_>,
        runtime: &cyber_server::runtime::Runtime,
        definition: &HookDefinition,
        event: &HookEvent,
        cancel: CancellationToken,
    ) -> Result<HookCommandReport, String> {
        let pointer = &definition.pointer;
        match definition.kind() {
            HookKind::McpTool => {
                crate::hook_mcp::HookMcpRunner { settings: runner }
                    .run_recorded(self, runtime, pointer, event, cancel)
                    .await
            }
            HookKind::Prompt => {
                crate::hook_prompt::HookPromptRunner {
                    resolved: runner.resolved,
                    trust: runner.trust,
                    invocation_trust: runner.invocation_trust,
                    home: runner.home,
                }
                .run_recorded(runtime, pointer, event, cancel)
                .await
            }
            HookKind::Http => {
                crate::hook_http::HookHttpRunner {
                    resolved: runner.resolved,
                    trust: runner.trust,
                    invocation_trust: runner.invocation_trust,
                    home: runner.home,
                }
                .run_recorded(runtime, pointer, event, cancel)
                .await
            }
            HookKind::Command => runner.run_recorded(runtime, pointer, event, cancel).await,
            HookKind::Client => {
                let hooks = self
                    .client_hooks
                    .get()
                    .ok_or("client hooks are unavailable")?;
                crate::hook_client::run_recorded(hooks.as_ref(), runtime, definition, event, cancel)
                    .await
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
        let id = definition
            .handler
            .id
            .as_deref()
            .unwrap_or(&definition.digest);
        if !report.ignored_fields.is_empty() {
            cyber_core::log::debug(
                "hooks",
                "hook decision fields ignored for this event",
                json!({"hook_id":id,"event":event.event(),"fields":report.ignored_fields}),
            );
        }
        if let Some(diagnostic) = &report.diagnostic {
            self.hook_notice(event, diagnostic, &definition.digest);
        }
        if crate::lifecycle_hooks::observed_unavailable(event, definition, report) {
            return Err(ToolError::Failed(format!("hook {id} unavailable")));
        }
        if crate::lifecycle_hooks::unapplied_effect(event, &report.decision) {
            self.hook_notice(
                event,
                "hook context/continuation admission remains unavailable for this event",
                &definition.digest,
            );
            if definition.handler.fail_closed {
                return Err(ToolError::Failed(
                    "hook context/continuation admission unavailable".into(),
                ));
            }
        }
        // suppress_output hides the hook's own output; receipts and the model-facing
        // deny reason are never suppressed.
        if report.suppresses_output() {
            return Ok(());
        }
        if report.outcome == HookOutcome::Blocked {
            self.hook_notice(
                event,
                report.decision.reason.as_deref().unwrap_or("blocked"),
                id,
            );
        }
        if report.decision.updated_input.is_some() {
            self.hook_notice(event, "hook rewrote tool input", id);
        }
        Ok(())
    }
}

/// Longest rewritten input a transcript notice repeats; the call record keeps all of it.
const REWRITE_NOTICE_LIMIT: usize = 2048;

/// The model-visible rewrite notice: credentials a hook injected (a common reason to
/// rewrite) are redacted before the input is shown or truncated.
fn rewrite_notice(tool: &str, input: &Value) -> String {
    let mut shown = cyber_core::memory::redact_credentials(&input.to_string());
    if shown.len() > REWRITE_NOTICE_LIMIT {
        let mut end = REWRITE_NOTICE_LIMIT;
        while !shown.is_char_boundary(end) {
            end -= 1;
        }
        shown.truncate(end);
        shown.push('…');
    }
    format!("A PreToolUse hook rewrote the {tool} call input to: {shown}")
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

/// One permission denial a PermissionDenied hook observes.
pub(crate) struct PermissionDenial<'a> {
    /// `rule` (deny rules and Mode ceilings), `auto`, `user` or `unattended`.
    pub(crate) source: &'a str,
    pub(crate) reason: &'a str,
    pub(crate) action: &'a str,
    pub(crate) resources: &'a [String],
}

/// The invocation fields a hook envelope captures; the runtime supplies the
/// Session-derived project identity at dispatch.
#[derive(Debug, Clone)]
pub(crate) struct HookOwner {
    pub(crate) session_id: String,
    pub(crate) directory: String,
    pub(crate) agent: String,
    pub(crate) mode: String,
    /// A Workflow Run event's run; a workflow agent's Session supplies its own.
    pub(crate) workflow_run_id: Option<String>,
}

impl HookOwner {
    pub(crate) fn of(inv: &Invocation) -> Self {
        Self {
            session_id: inv.session_id.clone(),
            directory: inv.directory.clone(),
            agent: inv.agent.clone(),
            mode: inv.mode.clone(),
            workflow_run_id: None,
        }
    }
}

impl BuiltinHost {
    /// Captured envelope identity of the Session that owns an invocation, with the
    /// Session-derived project, workspace and parent that hook admission requires.
    pub(crate) async fn hook_identity(&self, owner: &HookOwner) -> Result<HookIdentity, ToolError> {
        let runtime = self
            .runtime()
            .ok_or_else(|| ToolError::Failed("Hook execution requires runtime ownership".into()))?;
        let envelope = runtime
            .session_envelope(&owner.session_id)
            .await
            .map_err(|error| ToolError::Failed(error.to_string()))?;
        let mut identity = envelope.identity(
            &owner.session_id,
            &owner.directory,
            &owner.agent,
            &owner.mode,
        );
        if owner.workflow_run_id.is_some() {
            identity.workflow_run_id = owner.workflow_run_id.clone();
        }
        Ok(identity)
    }

    /// Dispatch a tool event for an invocation with its tool fields and `extra`.
    async fn dispatch_tool_event(
        &self,
        kind: &str,
        inv: &Invocation,
        extra: Value,
        cancel: CancellationToken,
    ) -> Result<HookDecision, ToolError> {
        let hook_input = if inv.name == "computer" {
            cyber_computer::action::redacted_input(Some(&inv.input))
        } else {
            inv.input.clone()
        };
        let mut fields =
            json!({"tool_name":inv.name,"tool_input":hook_input,"call_id":inv.call_id})
                .as_object()
                .expect("object")
                .clone();
        if let Some(extra) = extra.as_object() {
            fields.extend(extra.clone());
        }
        if let Some(annotations) = self.annotations_of(inv) {
            fields.insert("tool_annotations".into(), annotations);
        }
        let dispatched = self
            .dispatch_owned(kind, &HookOwner::of(inv), fields, &inv.name, cancel)
            .await?;
        Ok(dispatched.map(|(decision, _)| decision).unwrap_or_default())
    }
}

/// Build any hook event from a captured identity and event-specific fields.
pub(crate) fn lifecycle_event(
    kind: &str,
    identity: HookIdentity,
    fields: serde_json::Map<String, Value>,
) -> Result<HookEvent, ToolError> {
    HookEvent::new(
        kind,
        identity,
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

/// Notice owners for aggregated skips, shown as `hook <id>: <message>`.
const TRUST_NOTICE: &str = "trust";
const POLICY_NOTICE: &str = "managed-policy";

fn hooks_noun(count: usize) -> &'static str {
    if count == 1 { "hook" } else { "hooks" }
}

impl BuiltinHost {
    /// Report and record the matching handlers a managed policy refuses: one notice
    /// per event per Session, and a `skipped` receipt for each handler.
    async fn report_policy_skips(
        &self,
        runtime: &cyber_server::runtime::Runtime,
        catalog: &HookCatalog,
        event: &HookEvent,
        subject: &str,
    ) {
        let paths = target_paths(event, &self.opts.home);
        let skipped: Vec<_> = catalog
            .skipped_for(event.event())
            .filter(|definition| definition.matches_event(event, subject, &paths))
            .collect();
        if skipped.is_empty() {
            return;
        }
        let reason = catalog.policy.skip_reason();
        for definition in &skipped {
            self.record_skip(runtime, event, definition, &reason).await;
        }
        let session = &event.identity().session_id;
        if self.first_notice(format!("{POLICY_NOTICE}\0{session}\0{}", event.event())) {
            let count = skipped.len();
            let message = format!("{count} {} skipped by {reason}", hooks_noun(count));
            self.hook_notice(event, &message, POLICY_NOTICE);
        }
    }

    /// Record an untrusted handler's skip; the Session's first one also reports how
    /// many resolved handlers await review.
    async fn untrusted_hook_skipped(
        &self,
        runner: &HookCommandRunner<'_>,
        runtime: &cyber_server::runtime::Runtime,
        definition: &HookDefinition,
        event: &HookEvent,
    ) {
        self.record_skip(runtime, event, definition, "untrusted")
            .await;
        let session = &event.identity().session_id;
        if !self.first_notice(format!("{TRUST_NOTICE}\0{session}")) {
            return;
        }
        let root = &runner.resolved.trust.checkout_root;
        let count = HookCatalog::from_config(runner.resolved)
            .map(|catalog| {
                catalog
                    .definitions
                    .iter()
                    .filter(|definition| {
                        !definition
                            .is_trusted(root, runner.trust, runner.invocation_trust)
                            .unwrap_or(false)
                    })
                    .count()
            })
            .unwrap_or(1)
            .max(1);
        let message = format!(
            "{count} untrusted {} changed \u{2014} review with /hooks",
            hooks_noun(count)
        );
        self.hook_notice(event, &message, TRUST_NOTICE);
    }

    async fn record_skip(
        &self,
        runtime: &cyber_server::runtime::Runtime,
        event: &HookEvent,
        definition: &HookDefinition,
        reason: &str,
    ) {
        if let Err(error) = runtime.record_skipped_hook(event, definition, reason).await {
            cyber_core::log::debug(
                "hooks",
                "skipped hook receipt not recorded",
                json!({"error":error.to_string(),"digest":definition.digest}),
            );
        }
    }

    /// Whether `key` is noticed for the first time in this host.
    fn first_notice(&self, key: String) -> bool {
        self.hook_notices
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key)
    }

    fn hook_notice(&self, event: &HookEvent, message: &str, digest: &str) {
        if let Some(runtime) = self.runtime() {
            runtime.hook_notice(&event.identity().session_id, digest, message);
        }
    }
}
