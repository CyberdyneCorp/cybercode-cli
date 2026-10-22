//! Run results and handback (`workflows` → Run results; `hooks` → WorkflowRunStart and
//! WorkflowRunEnd).
//!
//! A return value whose JSON is at most 1 MiB is kept inline; a larger one is written to
//! `<state>/workflows/<run_id>.result.json` and the run keeps a preview. Once a run ends,
//! WorkflowRunEnd hooks observe it and a summary is admitted to the starting Session as a
//! queued `workflow` message with the idempotent ID `msg_workflow_<run_id>` (plus
//! `_<n>` after the run's `n`th resume). The admission is recorded
//! (`workflow.run.notified.1`) after it succeeds, and startup recovery admits any summary
//! a previous process left unrecorded, so the Session receives exactly one per ended
//! attempt: a resumed run that ends again sends a new one.

use std::path::Path;

use serde_json::{Value, json};

use super::model::Delivery;
use super::workflows::{RUN_NOTIFIED, RUN_RESUMED, WorkflowAgent, WorkflowRun};
use super::{Admission, Runtime, RuntimeError};

/// Return values larger than this (as JSON) are stored as a file with a preview.
pub const INLINE_RESULT_BYTES: usize = 1024 * 1024;
/// The preview a run keeps of a stored return value.
const PREVIEW_BYTES: usize = 16 * 1024;
/// The result preview a handback carries.
const HANDBACK_PREVIEW_BYTES: usize = 4 * 1024;

/// The message ID of a run's summary in its starting Session.
pub fn handback_message_id(run_id: &str) -> String {
    format!("msg_workflow_{run_id}")
}

/// The message ID of the summary of the attempt after `resumes` resumes.
pub fn attempt_handback_message_id(run_id: &str, resumes: u64) -> String {
    match resumes {
        0 => handback_message_id(run_id),
        n => format!("msg_workflow_{run_id}_{n}"),
    }
}

/// Keep `value` inline when its JSON is at most 1 MiB, else write it to
/// `<dir>/<run_id>.result.json` and keep a preview.
pub(super) fn store_result(
    dir: Option<&Path>,
    run: &mut WorkflowRun,
    value: Value,
) -> Result<(), String> {
    let text = value.to_string();
    if text.len() <= INLINE_RESULT_BYTES {
        run.result = Some(value);
        return Ok(());
    }
    let dir = dir.ok_or("no workflow state directory is configured for large results")?;
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let path = dir.join(format!("{}.result.json", run.id));
    std::fs::write(&path, &text).map_err(|error| error.to_string())?;
    run.result_file = Some(path.display().to_string());
    run.result_preview = Some(prefix(&text, PREVIEW_BYTES).to_owned());
    Ok(())
}

fn prefix(text: &str, bytes: usize) -> &str {
    let mut end = bytes.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The summary admitted to the starting Session: the return value preview, agent
/// counts, cost and duration. Built only from durable records, so a recovery admits the
/// same text and the admission stays an exact retry.
fn handback_text(run: &WorkflowRun, agents: &[WorkflowAgent]) -> String {
    let count = |status: &str| agents.iter().filter(|a| a.status == status).count();
    let result = match (&run.result, &run.result_preview) {
        (Some(value), _) => Some(value.to_string()),
        (None, Some(preview)) => Some(preview.clone()),
        (None, None) => None,
    };
    let preview = result
        .as_deref()
        .map(|text| prefix(text, HANDBACK_PREVIEW_BYTES));
    let summary = json!({
        "run_id": run.id,
        "name": run.name,
        "status": run.status,
        "reason": run.reason,
        "error": run.error.as_ref().map(ToString::to_string),
        "result_preview": preview,
        "result_truncated": result.as_ref().is_some_and(|text| text.len() > HANDBACK_PREVIEW_BYTES) || run.result_file.is_some(),
        "result_file": run.result_file,
        "agents": {
            "total": agents.len(),
            "completed": count("completed"),
            "failed": count("failed"),
            "cancelled": count("cancelled"),
            "interrupted": count("interrupted"),
        },
        // A fold from +0.0: an empty f64 `sum` is -0.0, which would print as `-0.0`.
        "cost_usd": agents.iter().fold(0.0, |sum, a| sum + a.cost),
        "tokens": agents.iter().map(|a| a.tokens).sum::<u64>(),
        "duration_ms": run.ended_ms.unwrap_or(run.started_ms).saturating_sub(run.started_ms),
    });
    format!("Workflow run handback: {summary}")
}

impl Runtime {
    /// An observational WorkflowRunStart/WorkflowRunEnd event on the owner Session; its
    /// envelope carries `workflow_run_id`.
    pub(super) async fn workflow_hook(&self, run: &WorkflowRun, kind: &str) {
        if self.inner.closed.is_cancelled() {
            return;
        }
        let Ok(handle) = self.inner.handle(&run.session_id).await else {
            return;
        };
        let fields = json!({"workflow": {
            "name": run.name,
            "status": run.status,
            "revision": run.revision,
            "phases": run.phases,
            "budget": run.budget,
            "reason": run.reason,
            "error": run.error,
        }});
        let mut hook = self
            .inner
            .lifecycle_hook(&handle, kind, run.name.clone(), fields)
            .await;
        hook.workflow_run_id = Some(run.id.clone());
        self.inner.observe_lifecycle(hook).await;
    }

    /// After a run's terminal record: WorkflowRunEnd hooks, then the summary.
    pub(super) async fn workflow_run_ended(&self, run: WorkflowRun) {
        self.workflow_hook(&run, "WorkflowRunEnd").await;
        if let Err(error) = self.hand_back_workflow_run(&run).await {
            cyber_core::log::warn(
                "workflow",
                "run summary not admitted",
                json!({"run_id": run.id, "error": error.to_string()}),
            );
        }
    }

    /// Admit the summary once, then record that it was admitted. An existing message
    /// with the summary's ID (an exact retry, or one already consumed) counts as admitted.
    pub(super) async fn hand_back_workflow_run(
        &self,
        run: &WorkflowRun,
    ) -> Result<(), RuntimeError> {
        if run.notified {
            return Ok(());
        }
        let agents = self.workflow_agents(&run.id)?;
        let message_id = attempt_handback_message_id(&run.id, self.workflow_run_resumes(&run.id)?);
        let mut admission = Admission::text(handback_text(run, &agents), Delivery::Queue);
        admission.message_id = Some(message_id.clone());
        admission.source = "workflow".into();
        admission.resume = run.continue_owner;
        match self.admit(&run.session_id, admission).await {
            Ok(_) | Err(RuntimeError::PromptConflict(_)) => {}
            Err(error) => return Err(error),
        }
        self.run_log()
            .append(&run.id, RUN_NOTIFIED, json!({"message_id": message_id}))
            .map_err(RuntimeError::Invalid)
    }

    /// How often the run was resumed: its attempt number less one.
    fn workflow_run_resumes(&self, run_id: &str) -> Result<u64, RuntimeError> {
        let run = run_id.to_owned();
        let count = self.inner.store.read(move |db| {
            Ok(db.query_row(
                "SELECT COUNT(*) FROM event WHERE aggregate_id = ?1 AND type = ?2",
                rusqlite::params![run, RUN_RESUMED],
                |row| row.get::<_, i64>(0),
            )?)
        })?;
        Ok(u64::try_from(count).unwrap_or_default())
    }

    /// Startup: admit the summary of every ended run a previous process did not record
    /// as admitted.
    pub(super) async fn recover_workflow_handbacks(&self) {
        let Ok(runs) = self.workflow_runs(None) else {
            return;
        };
        for run in runs
            .into_iter()
            .filter(|run| run.status.is_terminal() && !run.notified)
        {
            if let Err(error) = self.hand_back_workflow_run(&run).await {
                cyber_core::log::warn(
                    "workflow",
                    "run summary not admitted during recovery",
                    json!({"run_id": run.id, "error": error.to_string()}),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::WorkflowRunStatus;

    fn run() -> WorkflowRun {
        serde_json::from_value(json!({
            "id": "run_1", "name": "n", "session_id": "ses_1", "directory": "/", "mode": "default",
            "origin": "inline", "digest": "sha256:0", "args": {}, "status": "completed",
            "phases": [], "started_ms": 10, "ended_ms": 25
        }))
        .unwrap()
    }

    #[test]
    fn results_up_to_one_mebibyte_stay_inline() {
        let dir = tempfile::tempdir().unwrap();
        let mut small = run();
        store_result(Some(dir.path()), &mut small, json!({"ok": true})).unwrap();
        assert_eq!(small.result, Some(json!({"ok": true})));
        assert!(small.result_file.is_none());
        let mut large = run();
        let text = "é".repeat(INLINE_RESULT_BYTES / 2 + 1);
        store_result(Some(dir.path()), &mut large, json!(text)).unwrap();
        assert!(large.result.is_none());
        let file = large.result_file.clone().unwrap();
        assert_eq!(Path::new(&file), dir.path().join("run_1.result.json"));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            json!(text).to_string()
        );
        let preview = large.result_preview.unwrap();
        assert!(preview.len() <= PREVIEW_BYTES && preview.starts_with("\"éé"));
        let mut homeless = run();
        assert!(store_result(None, &mut homeless, json!(text)).is_err());
    }

    #[test]
    fn handback_reports_preview_counts_cost_and_duration() {
        let mut run = run();
        run.status = WorkflowRunStatus::Completed;
        run.result = Some(json!("x".repeat(HANDBACK_PREVIEW_BYTES * 2)));
        let agent = |status: &str, cost: f64| WorkflowAgent {
            id: "agt_1".into(),
            run_id: "run_1".into(),
            call_key: "k:1".into(),
            session_id: None,
            phase: None,
            label: None,
            model: None,
            tier: None,
            branch: None,
            status: status.into(),
            result: None,
            error: None,
            cost,
            tokens: 5,
            started_ms: None,
            ended_ms: None,
            compactions: 0,
        };
        let text = handback_text(&run, &[agent("completed", 0.5), agent("failed", 0.25)]);
        let summary: Value =
            serde_json::from_str(text.strip_prefix("Workflow run handback: ").unwrap()).unwrap();
        assert_eq!(summary["agents"]["completed"], 1);
        assert_eq!(summary["agents"]["failed"], 1);
        assert_eq!(summary["cost_usd"], 0.75);
        assert_eq!(summary["tokens"], 10);
        assert_eq!(summary["duration_ms"], 15);
        assert_eq!(summary["result_truncated"], true);
        assert_eq!(
            summary["result_preview"].as_str().unwrap().len(),
            HANDBACK_PREVIEW_BYTES
        );
    }

    /// Regression: a run stopped while queued has no agents, and its summary said
    /// `"cost_usd":-0.0` (the empty f64 sum).
    #[test]
    fn handback_of_a_run_without_agents_costs_zero_not_negative_zero() {
        let mut run = run();
        run.status = WorkflowRunStatus::Stopped;
        let text = handback_text(&run, &[]);
        assert!(text.contains(r#""cost_usd":0.0"#), "{text}");
        assert!(!text.contains("-0.0"), "{text}");
    }
}
