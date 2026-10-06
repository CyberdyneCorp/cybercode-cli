//! One trial: a disposable workspace, a Session driven to idle, and the hidden grader.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cyber_client::Client;
use cyber_core::eval::{Manifest, Task};
use futures::StreamExt;
use serde::Serialize;
use serde_json::{Value, json};

pub struct TrialEnv {
    pub client: Client,
    pub root: PathBuf,
    pub manifest: Manifest,
    pub model: String,
    pub output: PathBuf,
    pub keep: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrialResult {
    pub task: String,
    pub trial: u32,
    pub passed: bool,
    /// The run never reached grading for reasons outside the model (provider or harness).
    pub infrastructure_failure: bool,
    pub stop_reason: String,
    pub error: Option<String>,
    pub turns: u32,
    pub tool_calls: u32,
    pub cost_usd: f64,
    /// Some step had no price in the catalog, so `cost_usd` is a lower bound.
    pub unpriced: bool,
    pub tokens: Tokens,
    pub latency_seconds: f64,
    pub grader_exit: Option<i32>,
    pub grader_tail: String,
    pub session_id: String,
}

impl TrialResult {
    pub fn verdict(&self) -> String {
        let outcome = if self.passed {
            "PASS"
        } else if self.infrastructure_failure {
            "INFRA"
        } else {
            "FAIL"
        };
        format!(
            "{outcome} ({} turns, ${:.4}, {:.0}s, {})",
            self.turns, self.cost_usd, self.latency_seconds, self.stop_reason
        )
    }

    fn failed(task: &Task, trial: u32, error: String) -> Self {
        Self {
            task: task.id.clone(),
            trial,
            passed: false,
            infrastructure_failure: true,
            stop_reason: "error".into(),
            error: Some(error),
            turns: 0,
            tool_calls: 0,
            cost_usd: 0.0,
            unpriced: false,
            tokens: Tokens::default(),
            latency_seconds: 0.0,
            grader_exit: None,
            grader_tail: String::new(),
            session_id: String::new(),
        }
    }
}

/// Provider and harness failures, as opposed to the model's own outcomes.
fn infrastructure(kind: &str) -> bool {
    !matches!(kind, "context_overflow" | "content_policy")
}

pub async fn run(env: &TrialEnv, task: &Task, trial: u32) -> TrialResult {
    let workspace = match prepare(env, task, trial) {
        Ok(w) => w,
        Err(e) => return TrialResult::failed(task, trial, format!("workspace setup: {e}")),
    };
    let mut result = match drive(env, task, trial, &workspace).await {
        Ok(r) => r,
        Err(e) => TrialResult::failed(task, trial, e),
    };
    if !result.infrastructure_failure {
        grade(env, task, &workspace, &mut result).await;
    }
    let _ = std::fs::write(
        env.output
            .join("trials")
            .join(format!("{}-{trial}.json", task.id)),
        serde_json::to_string_pretty(&result).unwrap_or_default(),
    );
    if !env.keep {
        let _ = std::fs::remove_dir_all(&workspace);
    }
    result
}

/// Copy the fixture into a fresh git repository.
fn prepare(env: &TrialEnv, task: &Task, trial: u32) -> Result<PathBuf, String> {
    let fixture =
        cyber_core::eval::task_fixture(&env.manifest, task).ok_or("task has no fixture")?;
    let base = if env.keep {
        env.output.join("workspaces")
    } else {
        std::env::temp_dir().join("cyber-eval")
    };
    let dir = base.join(format!(
        "{}-{trial}-{}",
        task.id,
        cyber_core::ids::new_id("ws")
            .trim_start_matches("ws_")
            .to_lowercase()
    ));
    copy_dir(&env.root.join(&fixture.path), &dir).map_err(|e| e.to_string())?;
    let dir = std::fs::canonicalize(&dir).map_err(|e| e.to_string())?;
    for args in [
        &["init", "-q"][..],
        &["add", "-A"],
        &[
            "-c",
            "user.email=eval@cyber",
            "-c",
            "user.name=eval",
            // No background maintenance writing into .git after the commit returns.
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "commit",
            "-qm",
            "fixture",
        ],
    ] {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(args)
            .output()
            .is_ok_and(|o| o.status.success());
        if !ok {
            return Err(format!("git {}", args.join(" ")));
        }
    }
    Ok(dir)
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == "__pycache__" || name == ".DS_Store" {
            continue;
        }
        let target = to.join(&name);
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Run the task until the Session goes idle, enforcing cost and wall-time budgets.
async fn drive(
    env: &TrialEnv,
    task: &Task,
    trial: u32,
    workspace: &Path,
) -> Result<TrialResult, String> {
    let client = env.client.at(&workspace.display().to_string());
    let body = json!({
        "model": env.model,
        "mode": "bypass",
        "title": format!("eval {} #{trial}", task.id),
        "rules": { "question": "deny", "plan_enter": "deny", "plan_exit": "deny" },
        "max_steps": task.budget.max_turns,
    });
    let created = client
        .post("/sessions", body)
        .await
        .map_err(|e| e.to_string())?;
    let id = created["data"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let filter_id = id.clone();
    let mut events = Box::pin(
        client
            .events(true)
            .await
            .map_err(|e| e.to_string())?
            .filter(move |e| {
                futures::future::ready(e.session_id.as_deref() == Some(filter_id.as_str()))
            }),
    );
    let started = Instant::now();
    client
        .post(
            &format!("/sessions/{id}/prompt"),
            json!({ "parts": [{ "type": "text", "text": task.prompt }] }),
        )
        .await
        .map_err(|e| e.to_string())?;
    let wall = Duration::from_secs(
        task.budget
            .max_wall_seconds
            .unwrap_or(task.timeout_seconds)
            .min(task.timeout_seconds)
            .max(1),
    );
    let mut r = TrialResult::failed(task, trial, String::new());
    r.infrastructure_failure = false;
    r.error = None;
    r.stop_reason = "end_turn".into();
    r.session_id = id.clone();
    let deadline = tokio::time::sleep(wall);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            event = events.next() => match event {
                Some(e) if e.kind == "session.idle" => break,
                Some(e) => {
                    if apply(&mut r, &e.kind, &e.data) && over_budget(&r, task) {
                        stop(&client, &id, &mut r, "budget_exceeded").await;
                    }
                }
                None => {
                    r.error = Some("event stream closed".into());
                    r.infrastructure_failure = true;
                    break;
                }
            },
            _ = &mut deadline, if r.stop_reason != "timeout" => stop(&client, &id, &mut r, "timeout").await,
        }
    }
    if r.turns >= task.budget.max_turns.unwrap_or(u32::MAX) && r.stop_reason == "end_turn" {
        r.stop_reason = "max_turns".into();
    }
    r.latency_seconds = started.elapsed().as_secs_f64();
    Ok(r)
}

async fn stop(client: &Client, id: &str, r: &mut TrialResult, reason: &str) {
    r.stop_reason = reason.into();
    let _ = client
        .post(&format!("/sessions/{id}/interrupt"), json!({}))
        .await;
}

fn over_budget(r: &TrialResult, task: &Task) -> bool {
    let tokens = r.tokens.input
        + r.tokens.output
        + r.tokens.reasoning
        + r.tokens.cache_read
        + r.tokens.cache_write;
    task.budget.max_cost_usd.is_some_and(|m| r.cost_usd > m)
        || task.budget.max_tokens.is_some_and(|m| tokens > m)
}

/// Fold one event into the result; true after accounted model work (budget checkpoint).
fn apply(r: &mut TrialResult, kind: &str, data: &Value) -> bool {
    let base = kind.rsplit_once('.').map_or(kind, |(b, _)| b);
    match base {
        "session.step.ended" => {
            r.turns += 1;
            add_usage(r, data);
            return true;
        }
        "permission.auto_decided" if data["usage"].is_object() => {
            add_usage(r, data);
            return true;
        }
        "session.compaction.completed" => {
            add_usage(r, data);
            return true;
        }
        "session.tool.called" => r.tool_calls += 1,
        "session" if kind == "session.error" => {
            let k = data["kind"].as_str().unwrap_or("internal");
            r.error = Some(format!(
                "{k}: {}",
                data["message"].as_str().unwrap_or_default()
            ));
            r.infrastructure_failure |= infrastructure(k);
            if r.stop_reason == "end_turn" {
                r.stop_reason = "error".into();
            }
        }
        _ => {}
    }
    false
}

fn add_usage(r: &mut TrialResult, data: &Value) {
    let u = &data["usage"];
    r.tokens.input += u["input"].as_u64().unwrap_or(0);
    r.tokens.output += u["output"].as_u64().unwrap_or(0);
    r.tokens.reasoning += u["reasoning"].as_u64().unwrap_or(0);
    r.tokens.cache_read += u["cache_read"].as_u64().unwrap_or(0);
    r.tokens.cache_write += u["cache_write"].as_u64().unwrap_or(0);
    match data["cost"].as_f64() {
        Some(c) => r.cost_usd += c,
        None => r.unpriced = true,
    }
}

/// Run the hidden grader against the final workspace.
async fn grade(env: &TrialEnv, task: &Task, workspace: &Path, r: &mut TrialResult) {
    let grader_dir = env.root.join(&task.grading.grader_dir);
    let args: Vec<String> = task
        .grading
        .command
        .iter()
        .map(|a| {
            a.replace("{grader_dir}", &grader_dir.display().to_string())
                .replace("{workspace}", &workspace.display().to_string())
        })
        .collect();
    let Some((program, rest)) = args.split_first() else {
        return;
    };
    let child = tokio::process::Command::new(program)
        .args(rest)
        .current_dir(&env.root)
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(Duration::from_secs(300), child).await {
        Ok(Ok(out)) => {
            r.grader_exit = out.status.code();
            r.passed = out.status.success();
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            r.grader_tail = text
                .lines()
                .rev()
                .take(15)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
        }
        Ok(Err(e)) => r.grader_tail = format!("grader did not start: {e}"),
        Err(_) => r.grader_tail = "grader timed out after 300 s".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> Task {
        serde_json::from_value(json!({
            "id": "accounting", "prompt": "Fix the fixture", "timeout_seconds": 10,
            "budget": {"max_tokens": 100},
            "grading": {"grader_dir": "unused", "command": ["python3"]}
        }))
        .unwrap()
    }

    #[test]
    fn compaction_exhausts_budget_without_counting_as_a_turn() {
        let mut task = task();
        let mut r = TrialResult::failed(&task, 1, String::new());
        assert!(apply(
            &mut r,
            "session.step.ended.1",
            &json!({
                "usage": {"input": 10, "output": 2, "reasoning": 5, "cache_read": 3},
                "cost": 0.125
            })
        ));
        assert!(!over_budget(&r, &task));
        assert!(apply(
            &mut r,
            "session.compaction.completed.1",
            &json!({
                "usage": {"input": 60, "output": 10, "reasoning": 5, "cache_read": 4, "cache_write": 2},
                "cost": 0.25
            })
        ));
        assert!(over_budget(&r, &task), "all token classes total 101");
        assert_eq!(r.turns, 1);
        assert_eq!(r.cost_usd, 0.375);
        task.budget.max_tokens = None;
        task.budget.max_cost_usd = Some(0.25);
        assert!(
            over_budget(&r, &task),
            "compaction cost also exhausts the budget"
        );
    }

    #[test]
    fn auto_review_is_metered_without_counting_as_a_turn() {
        let task = task();
        let mut r = TrialResult::failed(&task, 1, String::new());
        assert!(apply(
            &mut r,
            "permission.auto_decided.1",
            &json!({
                "decision": "allow", "usage": {"input": 90, "output": 5, "reasoning": 6}, "cost": 0.5
            })
        ));
        assert!(over_budget(&r, &task));
        assert_eq!(r.turns, 0);
        assert_eq!(r.cost_usd, 0.5);
        assert!(!apply(
            &mut r,
            "permission.auto_decided.1",
            &json!({
                "decision": "fallback", "usage": null, "cost": null
            })
        ));
        assert!(
            !r.unpriced,
            "a fallback without inference is not unpriced work"
        );
    }

    #[test]
    fn reasoning_is_reported_and_counts_towards_token_budget() {
        let mut task = task();
        task.budget.max_tokens = Some(10);
        let mut r = TrialResult::failed(&task, 1, String::new());
        apply(
            &mut r,
            "session.step.ended.1",
            &json!({
                "usage": {"input": 4, "output": 3, "reasoning": 4}, "cost": 0.0
            }),
        );
        assert_eq!(serde_json::to_value(&r.tokens).unwrap()["reasoning"], 4);
        assert!(over_budget(&r, &task));
    }

    #[test]
    fn unpriced_compaction_remains_unpriced_after_a_priced_step() {
        let task = task();
        let mut r = TrialResult::failed(&task, 1, String::new());
        apply(
            &mut r,
            "session.compaction.completed.1",
            &json!({
                "usage": {"input": 10, "output": 5}, "cost": null
            }),
        );
        apply(
            &mut r,
            "session.step.ended.1",
            &json!({
                "usage": {"input": 1, "output": 1}, "cost": 0.125
            }),
        );
        assert!(r.unpriced);
        assert_eq!(r.cost_usd, 0.125);
    }
}
