//! Dedicated local MCP connections owned by synchronous hook receipts.
use crate::hook_reports::{failure, log_io, logged_io, skipped};
use crate::mcp::{LocalLauncher, LocalServer, authorize_server};
use crate::{
    BuiltinHost,
    hook_commands::{HookCommandReport, HookCommandRunner},
};
use cyber_core::config::{HookKind, Resolved};
use cyber_core::hooks::{HookAction, HookDefinition, HookEvent, HookOutcome};
use cyber_server::runtime::{HookExecution, HookExecutionIo, HookExecutionResult, Runtime};
use std::path::PathBuf;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub struct HookMcpRunner<'a, 'config> {
    pub settings: &'a HookCommandRunner<'config>,
}
struct Capture {
    report: HookCommandReport,
    io: Option<HookExecutionIo>,
    server: Option<LocalServer>,
    scratch: Option<PathBuf>,
}
impl Capture {
    fn settlement(&self) -> HookExecutionResult {
        HookExecutionResult {
            outcome: self.report.outcome,
            decision: self.report.decision.clone(),
            acknowledged: self.report.acknowledged,
            must_stop: self.report.must_stop,
            io: self.io.clone(),
        }
    }
    fn cleanup(&mut self) -> Result<(), String> {
        if !self.report.acknowledged {
            return Ok(());
        }
        if let Some(server) = &mut self.server {
            server.settle_after_shutdown(|| Ok(()))?;
        }
        if let Some(path) = self.scratch.take() {
            std::fs::remove_dir_all(path)
                .map_err(|_| "MCP hook scratch cleanup failed".to_string())?;
        }
        Ok(())
    }
}
impl HookMcpRunner<'_, '_> {
    fn authorize(
        &self,
        host: &BuiltinHost,
        pointer: &str,
        event: &HookEvent,
    ) -> Result<(Resolved, HookDefinition, bool), String> {
        let (expected, _) = crate::hook_authority::authorize(
            self.settings.resolved,
            self.settings.trust,
            self.settings.invocation_trust,
            pointer,
            event,
            HookKind::McpTool,
        )?;
        let resolver = host
            .hook_config
            .get()
            .ok_or("MCP hook live resolver is unavailable")?;
        let resolved = (resolver.resolve)(&event.identity().location.directory)?;
        let (definition, sandbox_all) = crate::hook_authority::authorize(
            &resolved,
            self.settings.trust,
            self.settings.invocation_trust,
            pointer,
            event,
            HookKind::McpTool,
        )?;
        if definition.digest != expected.digest || definition.scope != expected.scope {
            return Err("MCP hook definition changed during dispatch".into());
        }
        Ok((resolved, definition, sandbox_all))
    }

    pub async fn run_test(
        &self,
        host: &BuiltinHost,
        pointer: &str,
        event: &HookEvent,
        cancel: CancellationToken,
    ) -> Result<HookCommandReport, String> {
        if !event.is_synthetic() {
            return Err("MCP hook test requires synthetic identity".into());
        }
        let (resolved, definition, _) = self.authorize(host, pointer, event)?;
        if cancel.is_cancelled() {
            return Err("MCP hook cancelled before admission".into());
        }
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(definition.handler.timeout.into());
        let Some(owner) = cyber_server::runtime::try_start_synthetic_hook_execution(
            host.opts.store.clone(),
            event,
            &definition,
            log_io(&resolved),
        )
        .map_err(|e| e.to_string())?
        else {
            return Ok(skipped(false));
        };
        let lease = tokio::time::timeout_at(
            deadline,
            host.claim_hook_test_location(event, &owner.record().id, cancel.clone()),
        )
        .await
        .map_err(|_| "MCP hook checkout admission timed out".to_string())??;
        let mut capture = self
            .execute(host, pointer, event, deadline, cancel, None)
            .await;
        let retained = if capture.report.acknowledged {
            Some(lease.settle_retained()?)
        } else {
            drop(lease);
            None
        };
        owner
            .finish(capture.settlement())
            .map_err(|e| e.to_string())?;
        capture.cleanup()?;
        drop(retained);
        Ok(capture.report)
    }

    pub async fn run_recorded(
        &self,
        host: &BuiltinHost,
        runtime: &Runtime,
        pointer: &str,
        event: &HookEvent,
        cancel: CancellationToken,
    ) -> Result<HookCommandReport, String> {
        let (resolved, definition, _) = self.authorize(host, pointer, event)?;
        if cancel.is_cancelled() {
            return Err("MCP hook cancelled before admission".into());
        }
        let Some(mut owner) = runtime
            .try_start_hook_execution(event, &definition, log_io(&resolved))
            .await
            .map_err(|e| e.to_string())?
        else {
            return Ok(skipped(false));
        };
        if let Some(message) = &definition.handler.status_message {
            runtime.hook_notice(
                &event.identity().session_id,
                &owner.record().hook_id,
                message,
            );
        }
        let stop = owner.cancellation();
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(definition.handler.timeout.into());
        let mut capture = {
            let execution = self.execute(
                host,
                pointer,
                event,
                deadline,
                stop.clone(),
                Some((&mut owner, runtime)),
            );
            tokio::pin!(execution);
            tokio::select! {biased; _ = cancel.cancelled() => {stop.cancel();execution.await}, capture = &mut execution => capture}
        };
        capture.report.must_stop |= owner.verify(runtime).is_err();
        let record = owner
            .finish(capture.settlement())
            .map_err(|e| e.to_string())?;
        capture.cleanup()?;
        if capture.report.acknowledged
            && let Some(message) = &definition.handler.system_message
        {
            runtime.hook_notice(&record.session_id, &record.hook_id, message);
        }
        Ok(capture.report)
    }

    async fn execute(
        &self,
        host: &BuiltinHost,
        pointer: &str,
        event: &HookEvent,
        deadline: tokio::time::Instant,
        cancel: CancellationToken,
        owner: Option<(&mut HookExecution, &Runtime)>,
    ) -> Capture {
        let (resolved, definition, sandbox_all) = match self.authorize(host, pointer, event) {
            Ok(authorized) => authorized,
            Err(error) => return failed(event, None, error, true),
        };
        let name = definition.handler.server.as_deref().unwrap_or_default();
        let selected = match authorize_server(
            &resolved,
            self.settings.trust,
            &event.identity().location.directory,
            name,
        ) {
            Ok(selected) => selected,
            Err(error) => return failed(event, Some(&definition), error, true),
        };
        let arguments =
            match crate::mcp::render_arguments(definition.handler.arguments.as_ref(), event) {
                Ok(arguments) => arguments,
                Err(error) => return failed(event, Some(&definition), error, true),
            };
        let launcher = LocalLauncher {
            resolved: &resolved,
            trust: self.settings.trust,
            location: &event.identity().location.directory,
            home: self.settings.home,
            temp_dir: self.settings.temp_dir,
            helper: self.settings.helper,
            credential_env_names: self.settings.credential_env_names,
        };
        let connected = launcher
            .connect_for_hook(
                name,
                || {
                    if let Some((owner, runtime)) = owner {
                        owner.mark_launch(runtime).map_err(|e| e.to_string())?;
                    }
                    self.verify_server(host, pointer, event, &selected.digest)?;
                    Ok(())
                },
                deadline,
                definition.scope.requires_sandbox(sandbox_all),
                &cancel,
            )
            .await;
        let mut server = match connected {
            Ok(server) => server,
            Err(error) => {
                let mut capture = failed(
                    event,
                    Some(&definition),
                    error.diagnostic,
                    error.acknowledged,
                );
                if cancel.is_cancelled() && error.acknowledged {
                    capture.report = skipped(true);
                } else if tokio::time::Instant::now() >= deadline {
                    capture.report.outcome = HookOutcome::Timeout;
                }
                capture.scratch = error.scratch;
                return capture;
            }
        };
        let result = self
            .call(
                host,
                pointer,
                event,
                &definition,
                &selected.digest,
                &mut server,
                arguments,
                deadline,
                &cancel,
            )
            .await;
        let (acknowledged, _) = server.shutdown().await;
        let result = result.and_then(|value| {
            self.verify_server(host, pointer, event, &selected.digest)?;
            Ok(value)
        });
        let mut capture = match result {
            Ok(value) => match crate::mcp::decision(event, &value) {
                Ok(parsed) => Capture {
                    report: HookCommandReport {
                        outcome: if matches!(
                            parsed.decision.decision,
                            Some(HookAction::Deny | HookAction::Block)
                        ) {
                            HookOutcome::Blocked
                        } else {
                            HookOutcome::Ok
                        },
                        decision: parsed.decision,
                        ignored_fields: parsed.ignored_fields,
                        diagnostic: None,
                        acknowledged,
                        must_stop: !acknowledged,
                    },
                    io: log_io(&resolved).then(|| logged_io(event, value.to_string().as_bytes())),
                    server: None,
                    scratch: None,
                },
                Err(error) => failed(event, Some(&definition), error, acknowledged),
            },
            Err(error) => failed(event, Some(&definition), error, acknowledged),
        };
        if !acknowledged {
            capture = failed(
                event,
                Some(&definition),
                "MCP hook native shutdown was not acknowledged".into(),
                false,
            );
        } else if cancel.is_cancelled() {
            capture.report = skipped(true);
            capture.io = None;
        } else if tokio::time::Instant::now() >= deadline {
            capture.report.outcome = HookOutcome::Timeout;
        }
        capture.server = Some(server);
        capture
    }

    #[allow(clippy::too_many_arguments)]
    async fn call(
        &self,
        host: &BuiltinHost,
        pointer: &str,
        event: &HookEvent,
        definition: &HookDefinition,
        server_digest: &str,
        server: &mut LocalServer,
        arguments: serde_json::Value,
        deadline: tokio::time::Instant,
        cancel: &CancellationToken,
    ) -> Result<serde_json::Value, String> {
        self.verify_server(host, pointer, event, server_digest)?;
        let tool = definition
            .handler
            .tool
            .as_deref()
            .ok_or("MCP hook tool is absent")?;
        let schema = server
            .tools()
            .iter()
            .find(|entry| entry.remote_name == tool)
            .ok_or("MCP hook tool is absent or filtered")?
            .spec()
            .input_schema;
        crate::schema::validate(&schema, &arguments)
            .map_err(|_| "MCP hook arguments do not match the discovered schema".to_string())?;
        let result = tokio::select! {
            biased;
            _=cancel.cancelled()=>Err("MCP hook cancelled".into()),
            _=tokio::time::sleep_until(deadline)=>Err("MCP hook timed out".into()),
            result=server.call_tool(tool,arguments,deadline.saturating_duration_since(tokio::time::Instant::now()))=>result.map_err(|e|e.to_string()),
        }?;
        self.verify_server(host, pointer, event, server_digest)?;
        Ok(result)
    }
    fn verify_server(
        &self,
        host: &BuiltinHost,
        pointer: &str,
        event: &HookEvent,
        digest: &str,
    ) -> Result<(), String> {
        let (resolved, definition, _) = self.authorize(host, pointer, event)?;
        let selected = authorize_server(
            &resolved,
            self.settings.trust,
            &event.identity().location.directory,
            definition.handler.server.as_deref().unwrap_or_default(),
        )?;
        if selected.digest != digest {
            return Err("MCP hook server definition changed during execution".into());
        }
        Ok(())
    }
}
fn failed(
    event: &HookEvent,
    definition: Option<&HookDefinition>,
    message: String,
    acknowledged: bool,
) -> Capture {
    Capture {
        report: failure(
            event,
            definition,
            definition.is_none_or(|d| d.handler.fail_closed),
            HookOutcome::Error,
            message,
            acknowledged,
        ),
        io: None,
        server: None,
        scratch: None,
    }
}
