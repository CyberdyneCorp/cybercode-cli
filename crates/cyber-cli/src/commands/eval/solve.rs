//! Evaluate the host's selected source, never the workflow's claims about tests.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cyber_core::review::{
    Target,
    resolve::{Request, ReviewDiff},
};
use serde_json::{Value, json};

use super::trial::{self, TrialEnv, TrialResult};
use cyber_core::eval::Task;

pub(super) async fn drive(
    env: &TrialEnv,
    task: &Task,
    trial: u32,
    workspace: &Path,
) -> Result<TrialResult, String> {
    let client = env.client.at(&workspace.display().to_string());
    let session = client
        .post(
            "/sessions",
            json!({"model":env.model,"mode":"bypass",
        "rules":trial::rules(env,task),"title":format!("solve eval {} #{trial}",task.id)}),
        )
        .await
        .map_err(|e| e.to_string())?;
    let session_id = session["data"]["id"]
        .as_str()
        .ok_or("missing owner Session")?;
    let checks: Vec<_> = task
        .checks
        .iter()
        .map(|argv| json!({"argv":argv,"timeout_seconds":300}))
        .collect();
    let started = Instant::now();
    let run = client.post("/workflows/runs",json!({"session_id":session_id,"name":"solve",
        "args":{"task":task.prompt,"judge_model":env.model,"checks":checks,
        "candidates":[{"label":"first","model":env.model},{"label":"second","model":env.model}]},
        "budget":{"max_tokens":task.budget.max_tokens,"max_wall_seconds":task.budget.max_wall_seconds.unwrap_or(task.timeout_seconds)},
        "max_concurrent":2})).await.map_err(|e|e.to_string())?;
    let run_id = run["data"]["id"].as_str().ok_or("missing Workflow Run")?;
    let path = format!("/workflows/runs/{run_id}");
    let wall = Duration::from_secs(
        task.timeout_seconds
            .min(task.budget.max_wall_seconds.unwrap_or(task.timeout_seconds))
            .max(1),
    );
    let mut timed_out = false;
    let detail = match tokio::time::timeout(wall, async {
        loop {
            let detail = client.get(&path).await.map_err(|e| e.to_string())?;
            if super::super::workflows::ended(&detail["data"]).is_some() {
                return Ok::<Value, String>(detail["data"].clone());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    {
        Ok(result) => result?,
        Err(_) => {
            client
                .post(&format!("{path}/stop"), json!({}))
                .await
                .map_err(|e| e.to_string())?;
            timed_out = true;
            client.get(&path).await.map_err(|e| e.to_string())?["data"].clone()
        }
    };
    let mut result = TrialResult::failed(task, trial, String::new());
    result.session_id = session_id.into();
    result.latency_seconds = started.elapsed().as_secs_f64();
    result.cost_usd = detail["cost_usd"].as_f64().unwrap_or(0.0);
    // Aggregate workflow tokens do not establish individual classes or complete billing.
    // Keep the authoritative aggregate and all uncertainty in the separate solve record.
    result.unpriced = detail
        .pointer("/result/budget/observed/unpriced")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    result.solve = Some(detail.clone());
    account(&client, session_id, &mut result).await;
    result.stop_reason = if timed_out {
        "timeout"
    } else {
        detail["status"].as_str().unwrap_or("unknown")
    }
    .into();
    if detail["status"] != "completed" {
        result.infrastructure_failure = !timed_out
            && !matches!(
                detail["status"].as_str(),
                Some("budget_exceeded" | "stopped" | "paused")
            );
        result.error = Some("solve did not complete; see retained evidence".into());
        return Ok(result);
    }
    let verification = Instant::now();
    if let Err(error) = grade_selected(env, task, &client, &detail, &mut result).await {
        result.passed = false;
        result.infrastructure_failure = true;
        result.stop_reason = "verification_error".into();
        result.error = Some(error);
    }
    result.verification_seconds = Some(verification.elapsed().as_secs_f64());
    Ok(result)
}

async fn grade_selected(
    env: &TrialEnv,
    task: &Task,
    client: &cyber_client::Client,
    detail: &Value,
    result: &mut TrialResult,
) -> Result<(), String> {
    let Some(candidate) = selected(detail)? else {
        let blocked = verification_blocked(detail);
        result.infrastructure_failure = blocked;
        result.error =
            blocked.then(|| "host verification blocked; candidate quality not established".into());
        result.stop_reason = if blocked {
            "verification_blocked"
        } else {
            "no_recommendation"
        }
        .into();
        return Ok(());
    };
    let id = candidate["session_id"]
        .as_str()
        .ok_or("verified candidate has no Session")?;
    let candidate_session = client
        .get(&format!("/sessions/{id}"))
        .await
        .map_err(|e| e.to_string())?;
    let directory = PathBuf::from(
        candidate_session["data"]["directory"]
            .as_str()
            .ok_or("candidate directory missing")?,
    );
    let base = detail["solve_evidence"]["base"]["commit"]
        .as_str()
        .ok_or("pinned base missing")?;
    let expected = candidate["source_sha256"]
        .as_str()
        .ok_or("source identity missing")?;
    let before = fingerprint(env, &directory, base).await?;
    if before != expected {
        return Err("selected source changed after host verification".into());
    }
    result.infrastructure_failure = false;
    result.error = None;
    trial::grade(env, task, &directory, result).await;
    if fingerprint(env, &directory, base).await? != before {
        return Err("grading changed selected source".into());
    }
    Ok(())
}

fn verification_blocked(detail: &Value) -> bool {
    let Some(candidates) = detail
        .pointer("/solve_evidence/candidates")
        .and_then(Value::as_array)
    else {
        return false;
    };
    let verified = candidates.iter().any(|c| c["host_verified"] == true);
    !verified
        && candidates.iter().any(|c| {
            c["checks"].as_array().is_some_and(|checks| {
                checks
                    .iter()
                    .any(|check| matches!(check["outcome"].as_str(), Some("blocked" | "unknown")))
            })
        })
}

async fn account(client: &cyber_client::Client, session_id: &str, result: &mut TrialResult) {
    match client
        .get(&format!("/usage?scope=session&id={session_id}"))
        .await
    {
        Ok(usage) => {
            let total = &usage["data"]["total"];
            result.cost_usd = total["cost"].as_f64().unwrap_or(result.cost_usd);
            result.unpriced |= total["unpriced_steps"].as_u64().unwrap_or(1) > 0
                || total["usage_complete"] != true
                || total["token_classes_complete"] != true;
            let u = &total["tokens"];
            result.tokens = trial::Tokens {
                input: u["input"].as_u64().unwrap_or(0),
                output: u["output"].as_u64().unwrap_or(0),
                reasoning: u["reasoning"].as_u64().unwrap_or(0),
                cache_read: u["cache_read"].as_u64().unwrap_or(0),
                cache_write: u["cache_write"].as_u64().unwrap_or(0),
            };
            if let Some(solve) = result.solve.as_mut() {
                solve["usage"] = usage["data"].clone();
            }
        }
        Err(_) => {
            result.unpriced = true;
            if let Some(solve) = result.solve.as_mut() {
                solve["usage_unavailable"] = true.into();
            }
        }
    }
}

fn selected(detail: &Value) -> Result<Option<Value>, String> {
    if !detail["result"].is_object() {
        return Err("solve result missing or stored externally; selection unavailable".into());
    }
    let evidence = &detail["solve_evidence"];
    if detail["origin"] != json!({"kind":"bundled","name":"solve"})
        || evidence["run_id"] != detail["id"]
        || evidence["revision"] != detail["revision"]
    {
        return Err("missing authoritative bundled solve evidence".into());
    }
    let Some(label) = detail["result"]["recommended_candidate"].as_str() else {
        return Ok(None);
    };
    let candidates = evidence["candidates"]
        .as_array()
        .ok_or("missing candidate evidence")?;
    let mut matches = candidates.iter().filter(|c| c["label"] == label);
    let candidate = matches
        .next()
        .ok_or("recommended candidate not in host evidence")?;
    if matches.next().is_some()
        || candidate["host_verified"] != true
        || candidate["source_sha256"]
            .as_str()
            .is_none_or(str::is_empty)
        || candidate["checks"].as_array().is_none_or(|c| {
            c.is_empty()
                || c.iter().any(|r| {
                    r["outcome"] != "passed"
                        || r["settled"] != true
                        || r["source_unchanged"] != true
                })
        })
    {
        return Err("recommended candidate is not host verified".into());
    }
    Ok(Some(candidate.clone()))
}

async fn fingerprint(env: &TrialEnv, directory: &Path, base: &str) -> Result<String, String> {
    let helper = cyber_sandbox::find_helper().ok_or("repository inspection helper unavailable")?;
    let scratch = tempfile::tempdir().map_err(|e| e.to_string())?;
    let overrides = tempfile::NamedTempFile::new_in(scratch.path()).map_err(|e| e.to_string())?;
    std::fs::write(
        overrides.path(),
        cyber_core::worktrees::inspection::OVERRIDES,
    )
    .map_err(|e| e.to_string())?;
    let request = Request {
        worktree_base: Some(base.into()),
        location: directory.into(),
        target: Target::Uncommitted,
        overrides: overrides.path().into(),
        merge_base: false,
        excludes: None,
    };
    let arguments = vec![
        "--git-review".into(),
        serde_json::to_string(&request).map_err(|e| e.to_string())?,
    ];
    let output = super::verification::capture(
        env,
        scratch.path(),
        helper.to_str().ok_or("invalid helper path")?,
        &arguments,
        true,
    )
    .await?;
    if !output.status.success() {
        return Err(format!(
            "selected-source inspection failed ({:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(1000)
                .collect::<String>()
        ));
    }
    let diff: ReviewDiff = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    if !diff.limitations.is_empty() {
        return Err("selected source inspection incomplete".into());
    }
    diff.source_sha256
        .ok_or("selected source fingerprint missing".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail() -> Value {
        json!({"id":"r1","revision":1,"origin":{"kind":"bundled","name":"solve"},
        "result":{"recommended_candidate":"first"},"solve_evidence":{"run_id":"r1","revision":1,
        "candidates":[{"label":"first","host_verified":true,"source_sha256":"abc","checks":[{"outcome":"passed","settled":true,"source_unchanged":true}]}]}})
    }

    #[test]
    fn recommendation_requires_matching_host_authority_and_settled_checks() {
        let value = detail();
        assert!(selected(&value).unwrap().is_some());
        for pointer in [
            "/solve_evidence/run_id",
            "/solve_evidence/revision",
            "/solve_evidence/candidates/0/host_verified",
            "/solve_evidence/candidates/0/checks/0/source_unchanged",
            "/solve_evidence/candidates/0/checks/0/settled",
        ] {
            let mut value = value.clone();
            *value.pointer_mut(pointer).unwrap() = Value::Null;
            assert!(selected(&value).is_err(), "{pointer}");
        }
        let mut none = value.clone();
        none["result"]["recommended_candidate"] = Value::Null;
        assert!(selected(&none).unwrap().is_none());
        let mut custom = value;
        custom["origin"] = json!({"kind":"inline"});
        assert!(selected(&custom).is_err());
        let mut spilled = detail();
        spilled["result"] = Value::Null;
        spilled["result_file"] = "retained.json".into();
        assert!(selected(&spilled).is_err());
    }

    #[test]
    fn blocked_checks_are_infrastructure_only_without_a_verified_survivor() {
        let mut value = detail();
        value["solve_evidence"]["candidates"][0]["host_verified"] = false.into();
        value["solve_evidence"]["candidates"][0]["checks"][0]["outcome"] = "blocked".into();
        assert!(verification_blocked(&value));
        value["solve_evidence"]["candidates"]
            .as_array_mut()
            .unwrap()
            .push(detail()["solve_evidence"]["candidates"][0].clone());
        assert!(!verification_blocked(&value));
        value["solve_evidence"]["candidates"]
            .as_array_mut()
            .unwrap()
            .pop();
        value["solve_evidence"]["candidates"][0]["checks"][0]["outcome"] = "failed".into();
        assert!(!verification_blocked(&value));
    }
}
