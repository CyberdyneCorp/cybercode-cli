//! `cyber exec --goal` (`goals` → Non-interactive goals): queue the Goals, keep following
//! the Session until every Goal is terminal, and map the first non-met one to the exit
//! code: 0 met, 3 impossible, 4 budget exceeded, 1 anything else (paused, cleared).

use cyber_client::Client;
use serde_json::{Value, json};

use super::report::Run;
use crate::error::CliError;

pub const EXIT_IMPOSSIBLE: u8 = 3;

/// One `--goal` with the `--goal-check` that followed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalArg {
    pub condition: String,
    pub check: Option<String>,
}

/// Pair each `--goal-check` with the `--goal` before it, from the raw arguments in order.
/// A check before any goal, or two checks for one goal, is a usage error.
pub fn pair<I: IntoIterator<Item = String>>(args: I) -> Result<Vec<GoalArg>, CliError> {
    let mut goals: Vec<GoalArg> = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) => (flag.to_owned(), Some(value.to_owned())),
            None => (arg.clone(), None),
        };
        let is_goal = flag == "--goal";
        if !is_goal && flag != "--goal-check" {
            continue;
        }
        let value = match inline {
            Some(value) => value,
            None => args
                .next()
                .ok_or_else(|| CliError::usage(format!("{flag} needs a value")))?,
        };
        if is_goal {
            cyber_core::goals::check_condition(&value).map_err(CliError::usage)?;
            goals.push(GoalArg {
                condition: value,
                check: None,
            });
            continue;
        }
        let Some(last) = goals.last_mut() else {
            return Err(CliError::usage(
                "--goal-check applies to the --goal before it",
            ));
        };
        if last.check.replace(value).is_some() {
            return Err(CliError::usage(
                "each --goal takes at most one --goal-check",
            ));
        }
    }
    Ok(goals)
}

/// Queue the Goals: the first is set (starting a Turn), the rest are added in order.
/// Returns their IDs.
pub async fn queue(
    client: &Client,
    session_id: &str,
    goals: &[GoalArg],
    max_turns: Option<u32>,
) -> Result<Vec<String>, CliError> {
    let mut ids = Vec::new();
    for (i, goal) in goals.iter().enumerate() {
        let mut body = json!({
            "condition": goal.condition,
            "mode": if i == 0 { "set" } else { "add" },
        });
        if let Some(check) = &goal.check {
            body["check"] = check.clone().into();
        }
        if let Some(turns) = max_turns {
            body["budget"] = json!({ "max_turns": turns });
        }
        let created = client
            .post(&format!("/sessions/{session_id}/goals"), body)
            .await
            .map_err(|e| CliError::runtime(e.to_string()))?;
        ids.push(
            created["data"]["goal_id"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        );
    }
    Ok(ids)
}

/// Whether the run's Goals still have work: a Goal is active. A met Goal activates the
/// next queued one in the same commit, so a queue with nothing current has stopped
/// (after `impossible` or a clear), and a paused Goal waits for a user exec cannot ask.
pub async fn unfinished(client: &Client, session_id: &str) -> Result<bool, CliError> {
    let view = client
        .get(&format!("/sessions/{session_id}/goals"))
        .await
        .map_err(|e| CliError::runtime(e.to_string()))?;
    Ok(view["data"]["current"]["status"] == "active")
}

/// Send the prompt (unless the run only has Goals, whose prompts start the work), then
/// queue the Goals. The message ID and the Goal IDs; the inner error is the server's
/// refusal of the prompt.
pub async fn submit(
    client: &Client,
    session_id: &str,
    args: &super::ExecArgs,
    prompt: String,
) -> Result<Result<(Option<String>, Vec<String>), cyber_client::ClientError>, CliError> {
    let mut message = None;
    if !prompt.is_empty() || args.goals.is_empty() {
        let (path, body) = super::setup::request(session_id, args, prompt)?;
        match client.post(&path, body).await {
            Ok(receipt) => message = receipt["data"]["message_id"].as_str().map(str::to_string),
            Err(error) => return Ok(Err(error)),
        }
    }
    let ids = queue(client, session_id, &args.goals, args.max_turns).await?;
    Ok(Ok((message, ids)))
}

/// Record the Goals' outcomes on the run: `goals[]` for the JSON result and the first
/// non-met Goal's status for the exit code.
pub async fn settle(client: &Client, session_id: &str, ids: &[String], run: &mut Run) {
    if ids.is_empty() {
        return;
    }
    let mut outcomes = Vec::new();
    for id in ids {
        let Ok(goal) = client
            .get(&format!("/sessions/{session_id}/goals/{id}"))
            .await
        else {
            continue;
        };
        let g = &goal["data"];
        outcomes.push(json!({
            "id": id,
            "condition": g["condition"],
            "status": g["status"],
            "turns": g["turns"],
            "cost_usd": g["cost_usd"],
            "last_reason": g["reason"],
        }));
    }
    run.goals = Some(outcomes);
}

/// The exit code of the first non-met Goal; `None` when every Goal was met.
pub fn exit_code(goals: &[Value]) -> Option<(u8, String)> {
    let first = goals.iter().find(|g| g["status"] != "met")?;
    let reason = first["last_reason"].as_str().unwrap_or_default();
    let status = first["status"].as_str().unwrap_or("unknown");
    let code = match status {
        "impossible" => EXIT_IMPOSSIBLE,
        "budget_exceeded" => super::EXIT_BUDGET,
        _ => super::EXIT_ERROR,
    };
    Some((code, format!("goal {status}: {reason}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn checks_bind_to_the_goal_before_them() {
        let goals = pair(args(&[
            "exec",
            "--goal",
            "lint passes",
            "--goal-check",
            "npm run lint",
            "--goal=tests pass",
            "--max-turns",
            "9",
        ]))
        .unwrap();
        assert_eq!(
            goals,
            vec![
                GoalArg {
                    condition: "lint passes".into(),
                    check: Some("npm run lint".into())
                },
                GoalArg {
                    condition: "tests pass".into(),
                    check: None
                },
            ]
        );
        assert!(pair(args(&["--goal-check", "x", "--goal", "y"])).is_err());
        assert!(
            pair(args(&[
                "--goal",
                "y",
                "--goal-check",
                "a",
                "--goal-check",
                "b"
            ]))
            .is_err()
        );
        assert!(pair(args(&["--goal"])).is_err());
    }

    #[test]
    fn the_first_non_met_goal_decides_the_exit() {
        let met = json!({"status": "met"});
        assert_eq!(exit_code(std::slice::from_ref(&met)), None);
        let impossible = json!({"status": "impossible", "last_reason": "no access"});
        let budget = json!({"status": "budget_exceeded", "last_reason": "max_turns 50/50"});
        assert_eq!(
            exit_code(&[met.clone(), impossible, budget.clone()])
                .unwrap()
                .0,
            3
        );
        assert_eq!(exit_code(&[budget]).unwrap().0, 4);
        assert_eq!(
            exit_code(&[json!({"status": "paused", "last_reason": "auth"})]).unwrap(),
            (1, "goal paused: auth".to_string())
        );
    }
}
