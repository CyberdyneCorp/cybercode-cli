//! Synthetic selection and scheduling; decisions stay outside every Session.
use std::collections::HashSet;

use cyber_core::config::HookKind;
use cyber_core::hooks::{HookCatalog, HookDecision, HookDefinition, HookEvent, HookScope};
use futures::{StreamExt, stream};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::{
    HookCommandError, HookCommandReport, HookCommandRunner, HookOutcome, interpret_hook_command,
};
use crate::BuiltinHost;

#[derive(Debug, Serialize)]
pub struct HookTestHandlerResult {
    pub hook_id: String,
    pub digest: String,
    pub pointer: String,
    pub scope: HookScope,
    pub kind: HookKind,
    #[serde(flatten)]
    pub report: HookCommandReport,
}

#[derive(Debug, Serialize)]
pub struct HookTestRun {
    pub event: String,
    pub invocation_id: String,
    pub synthetic: bool,
    pub complete: bool,
    pub withheld_definitions: Vec<String>,
    pub decision: HookDecision,
    pub results: Vec<HookTestHandlerResult>,
}

struct Observer<'a> {
    stop: &'a CancellationToken,
    notice: &'a (dyn Fn(&str, &str) + Sync),
}

impl BuiltinHost {
    /// Exercise matching handlers, report unsupported transports, and never apply
    /// decisions to a Session. The caller retains the future through cancellation.
    pub async fn test_hooks(
        &self,
        runner: &HookCommandRunner<'_>,
        mut event: HookEvent,
        cancel: CancellationToken,
        notice: &(dyn Fn(&str, &str) + Sync),
    ) -> Result<HookTestRun, String> {
        if !event.is_synthetic() {
            return Err("hook tests require synthetic identity".into());
        }
        let catalog = HookCatalog::from_config(runner.resolved)?;
        let stop = cancel.child_token();
        let observer = Observer {
            stop: &stop,
            notice,
        };
        let mut decision = HookDecision::default();
        let results = if matches!(event.event(), "PreToolUse" | "PermissionDenied") {
            self.test_rewriting_hooks(runner, &catalog, &mut event, &observer, &mut decision)
                .await?
        } else {
            self.test_fixed_hooks(runner, &catalog, &event, &observer, &mut decision)
                .await?
        };
        Ok(HookTestRun {
            event: event.event().into(),
            invocation_id: event.identity().session_id.clone(),
            synthetic: true,
            withheld_definitions: if runner.resolved.trust.trusted {
                Vec::new()
            } else {
                runner.resolved.trust.definitions.clone()
            },
            complete: !stop.is_cancelled()
                && results.iter().all(|result| {
                    result.report.acknowledged
                        && !result.report.must_stop
                        && !matches!(
                            result.report.outcome,
                            HookOutcome::Error | HookOutcome::Timeout
                        )
                }),
            decision,
            results,
        })
    }

    async fn test_rewriting_hooks(
        &self,
        runner: &HookCommandRunner<'_>,
        catalog: &HookCatalog,
        event: &mut HookEvent,
        observer: &Observer<'_>,
        decision: &mut HookDecision,
    ) -> Result<Vec<HookTestHandlerResult>, String> {
        let stop = observer.stop;
        let mut commands = HashSet::new();
        let mut results = Vec::new();
        for definition in &catalog.definitions {
            if stop.is_cancelled() {
                break;
            }
            let paths = crate::tool_hooks::target_paths(event, &self.opts.home);
            if !definition.matches_event(event, &subject(event), &paths) {
                continue;
            }
            let prepared = prepare(runner, definition, &mut commands);
            let result = self
                .test_handler(runner, definition, event, observer, prepared)
                .await;
            crate::tool_hooks::merge_decision(definition, result.report.clone(), decision);
            if let Some(input) = decision.updated_input.clone() {
                *event = event.with_tool_input(input)?;
            }
            results.push(result);
        }
        Ok(results)
    }

    async fn test_fixed_hooks(
        &self,
        runner: &HookCommandRunner<'_>,
        catalog: &HookCatalog,
        event: &HookEvent,
        observer: &Observer<'_>,
        decision: &mut HookDecision,
    ) -> Result<Vec<HookTestHandlerResult>, String> {
        let paths = crate::tool_hooks::target_paths(event, &self.opts.home);
        let subject = subject(event);
        let mut commands = HashSet::new();
        // Prepare in declared order so trust and deduplication do not race completion.
        let selected: Vec<_> = catalog
            .definitions
            .iter()
            .filter(|definition| definition.matches_event(event, &subject, &paths))
            .map(|definition| (definition, prepare(runner, definition, &mut commands)))
            .collect();
        let mut results = stream::iter(selected.into_iter().enumerate().map(
            |(index, (definition, prepared))| async move {
                let result = self
                    .test_handler(runner, definition, event, observer, prepared)
                    .await;
                (index, definition, result)
            },
        ))
        .buffer_unordered(catalog.settings.concurrency)
        .collect::<Vec<_>>()
        .await;
        // Even after cancellation, drain every launched native owner before returning.
        results.sort_by_key(|(index, _, _)| *index);
        Ok(results
            .into_iter()
            .map(|(_, definition, result)| {
                crate::tool_hooks::merge_decision(definition, result.report.clone(), decision);
                result
            })
            .collect())
    }

    async fn test_handler(
        &self,
        runner: &HookCommandRunner<'_>,
        definition: &HookDefinition,
        event: &HookEvent,
        observer: &Observer<'_>,
        prepared: Prepared,
    ) -> HookTestHandlerResult {
        let id = definition
            .handler
            .id
            .as_deref()
            .unwrap_or(&definition.digest);
        let stop = observer.stop;
        let report = if stop.is_cancelled() {
            skipped("cancelled before launch", true)
        } else {
            self.test_prepared_handler(runner, definition, event, observer, prepared)
                .await
        };
        if report.must_stop {
            stop.cancel();
        }
        HookTestHandlerResult {
            hook_id: id.into(),
            digest: definition.digest.clone(),
            pointer: definition.pointer.clone(),
            scope: definition.scope,
            kind: definition.kind(),
            report,
        }
    }

    async fn test_prepared_handler(
        &self,
        runner: &HookCommandRunner<'_>,
        definition: &HookDefinition,
        event: &HookEvent,
        observer: &Observer<'_>,
        prepared: Prepared,
    ) -> HookCommandReport {
        let id = definition
            .handler
            .id
            .as_deref()
            .unwrap_or(&definition.digest);
        let stop = observer.stop;
        let notice = observer.notice;
        match prepared {
            Prepared::Skip(reason) => skipped(reason, false),
            Prepared::Error(error) => unavailable(event, definition, error, true),
            Prepared::Run => {
                if let Some(message) = &definition.handler.status_message {
                    notice(id, message);
                }
                let execution = if definition.kind() == HookKind::Http {
                    crate::hook_http::HookHttpRunner {
                        resolved: runner.resolved,
                        trust: runner.trust,
                        invocation_trust: runner.invocation_trust,
                        home: runner.home,
                    }
                    .run_test(self, &definition.pointer, event, stop.clone())
                    .await
                } else {
                    runner
                        .run_test(self, &definition.pointer, event, stop.clone())
                        .await
                };
                let report = match execution {
                    Ok(report) => report,
                    Err(error) => unavailable(event, definition, error, false),
                };
                if report.acknowledged
                    && report.outcome != HookOutcome::Skipped
                    && let Some(message) = &definition.handler.system_message
                {
                    notice(id, message);
                }
                report
            }
        }
    }
}

enum Prepared {
    Run,
    Skip(&'static str),
    Error(String),
}

fn prepare(
    runner: &HookCommandRunner<'_>,
    definition: &HookDefinition,
    commands: &mut HashSet<String>,
) -> Prepared {
    match definition.is_trusted(
        &runner.resolved.trust.checkout_root,
        runner.trust,
        runner.invocation_trust,
    ) {
        Ok(false) => return Prepared::Skip("untrusted hook skipped"),
        Err(error) => return Prepared::Error(error.to_string()),
        Ok(true) => {}
    }
    if !matches!(definition.kind(), HookKind::Command | HookKind::Http)
        || definition.handler.asynchronous
    {
        return Prepared::Error(
            "Hook handler transport or async scheduling is not implemented".into(),
        );
    }
    if let Some(command) = &definition.handler.command
        && !commands.insert(command.clone())
    {
        return Prepared::Skip("duplicate command skipped");
    }
    Prepared::Run
}

fn unavailable(
    event: &HookEvent,
    definition: &HookDefinition,
    error: String,
    acknowledged: bool,
) -> HookCommandReport {
    interpret_hook_command(
        event,
        definition
            .handler
            .id
            .as_deref()
            .unwrap_or(&definition.digest),
        definition.handler.fail_closed,
        Err(HookCommandError {
            message: error,
            acknowledged,
        }),
    )
}

fn skipped(reason: &str, must_stop: bool) -> HookCommandReport {
    HookCommandReport {
        outcome: HookOutcome::Skipped,
        decision: Default::default(),
        ignored_fields: Vec::new(),
        diagnostic: Some(reason.into()),
        acknowledged: true,
        must_stop,
    }
}

fn subject(event: &HookEvent) -> String {
    let payload = event.as_json();
    let value = match event.event() {
        "Notification" => payload["notification_type"].as_str(),
        "FileChanged" => payload["file_path"]
            .as_str()
            .or_else(|| payload["path"].as_str()),
        _ => payload["tool_name"].as_str(),
    };
    value.unwrap_or_default().into()
}
