//! Goals on the production App (`goals`): set and list over HTTP with the scripted
//! OpenAI-compatible provider answering both the agent and the evaluator, persisted
//! across an App rebuild.

#[path = "support/scripted_model.rs"]
mod scripted_model;
#[path = "support/mode_app.rs"]
mod support;

use std::time::Duration;

use serde_json::{Value, json};
use support::{ModeApp, Setup, prompted_call};

/// The evaluator (its system prompt judges goals) says met; the agent answers as usual.
fn reply(body: &Value) -> Value {
    let judging = body["messages"].as_array().into_iter().flatten().any(|m| {
        m["role"] == "system"
            && m["content"]
                .as_str()
                .is_some_and(|c| c.contains("judge whether a coding agent"))
    });
    if judging {
        return json!({"content": json!({"verdict": "met", "reason": "the evidence shows it"}).to_string()});
    }
    prompted_call(body)
}

async fn session(app: &ModeApp) -> String {
    let (status, body) = app.create("default").await;
    assert_eq!(status, 201, "{body}");
    body["data"]["id"].as_str().unwrap().to_string()
}

async fn wait_goals(app: &ModeApp, id: &str, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let (status, body) = app
            .send("GET", &format!("/sessions/{id}/goals"), None)
            .await;
        assert_eq!(status, 200, "{body}");
        if done(&body["data"]) {
            return body["data"].clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "goals never settled: {body}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn a_goal_set_over_http_is_met_and_survives_an_app_rebuild() {
    let temp = tempfile::tempdir().unwrap();
    let setup = || Setup {
        database: Some(temp.path().join("cyber.db")),
        reply: Some(reply),
        ..Setup::default()
    };
    let app = ModeApp::start(setup()).await;
    let id = session(&app).await;
    let (status, body) = app
        .send(
            "POST",
            &format!("/sessions/{id}/goals"),
            Some(json!({"condition": "the README mentions goals"})),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(body["data"]["status"], "active");
    let goal_id = body["data"]["goal_id"].as_str().unwrap().to_owned();

    let done = wait_goals(&app, &id, |g| {
        g["current"].is_null() && !g["ended"].as_array().unwrap().is_empty()
    })
    .await;
    assert_eq!(done["ended"][0]["goal_id"], goal_id);
    assert_eq!(done["ended"][0]["status"], "met");
    assert_eq!(
        done["ended"][0]["evaluations"][0]["reason"],
        "the evidence shows it"
    );

    let app = app.restart(setup()).await;
    let (status, body) = app
        .send("GET", &format!("/sessions/{id}/goals/{goal_id}"), None)
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["status"], "met");
    let (_, health) = app.send("GET", "/health", None).await;
    assert!(
        health["features"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "goals"),
        "{health}"
    );
}

#[tokio::test]
async fn http_errors_follow_the_goal_contract() {
    let app = ModeApp::start(Setup {
        reply: Some(reply),
        ..Setup::default()
    })
    .await;
    let id = session(&app).await;
    let (status, body) = app
        .send(
            "POST",
            &format!("/sessions/{id}/goals"),
            Some(json!({"condition": "x".repeat(5000)})),
        )
        .await;
    assert_eq!(
        (status, body["_tag"].as_str()),
        (400, Some("GoalConditionTooLongError"))
    );
    assert_eq!(
        body["message"],
        "Goal conditions are limited to 4000 characters; put details in a file and reference it"
    );
    let (status, body) = app
        .send("POST", &format!("/sessions/{id}/goals/pause"), None)
        .await;
    assert_eq!(
        (status, body["_tag"].as_str()),
        (404, Some("GoalNotFoundError"))
    );
    let (status, body) = app
        .send("GET", &format!("/sessions/{id}/goals/gol_nope"), None)
        .await;
    assert_eq!(
        (status, body["_tag"].as_str()),
        (404, Some("GoalNotFoundError"))
    );
}

/// The JSON a hook wrote to `path`, once it is complete.
async fn written(path: &std::path::Path) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(value) = serde_json::from_str(&text)
        {
            return value;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{} never written",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// `goals` → Goal events: `GoalEvaluated` gets `{goal, verdict, reason}` and
/// `GoalCompleted` the Goal JSON on stdin; both are observational command hooks.
#[tokio::test]
async fn goal_completed_hook_receives_goal_json() {
    let hook =
        |file: &str| json!([{"hooks": [{"type": "command", "command": format!("cat > {file}")}]}]);
    let app = ModeApp::start(Setup {
        reply: Some(reply),
        config: json!({"hooks": {"GoalEvaluated": hook("evaluated.json"),
            "GoalCompleted": hook("completed.json")}}),
        ..Setup::default()
    })
    .await;
    let id = session(&app).await;
    let (status, body) = app
        .send(
            "POST",
            &format!("/sessions/{id}/goals"),
            Some(json!({"condition": "the hooks fire", "check": "true"})),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    let goal_id = body["data"]["goal_id"].clone();
    wait_goals(&app, &id, |g| g["current"].is_null()).await;
    let evaluated = written(&app.repo.join("evaluated.json")).await;
    assert_eq!(evaluated["verdict"], "met", "{evaluated}");
    assert_eq!(evaluated["reason"], "the evidence shows it");
    assert_eq!(evaluated["goal"]["goal_id"], goal_id);
    // The Goal lives on the Session stream, which a Session export carries whole.
    let kinds: Vec<String> = app.events(&id).into_iter().map(|e| e.kind).collect();
    for kind in [
        "session.goal.set.1",
        "session.goal.activated.1",
        "session.goal.evaluated.1",
        "session.goal.completed.1",
    ] {
        assert!(kinds.iter().any(|k| k == kind), "{kind} in {kinds:?}");
    }
    let completed = written(&app.repo.join("completed.json")).await;
    assert_eq!(completed["goal"]["goal_id"], goal_id, "{completed}");
    assert_eq!(completed["goal"]["status"], "met");
    assert_eq!(completed["goal"]["condition"], "the hooks fire");
    assert_eq!(completed["goal"]["check"], "true");
}

/// 0 before the restart, 1 after.
static PHASE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Before the restart the evaluator says not met, with a reason that holds the
/// continuation Turn at a gate; after it, met.
fn restart_reply(body: &Value) -> Value {
    let judging = body.to_string().contains("judge whether a coding agent");
    match (judging, PHASE.load(std::sync::atomic::Ordering::SeqCst)) {
        (true, 0) => json!({"content": json!({"verdict": "not_met",
            "reason": "the report is missing GATE<<hold>>"}).to_string()}),
        (true, _) => json!({"content": json!({"verdict": "met",
            "reason": "the report exists"}).to_string()}),
        (false, _) => prompted_call(body),
    }
}

/// `goals` → Goals survive resume and restart, on a real App restart: the server stops
/// mid-goal (a continuation Turn in flight); the rebuilt App evaluates the active Goal
/// once on its last Turn and, the verdict being met, completes it without running
/// another work Turn.
#[tokio::test]
async fn restart_mid_goal_evaluates_before_continuing() {
    let temp = tempfile::tempdir().unwrap();
    let setup = || Setup {
        database: Some(temp.path().join("cyber.db")),
        reply: Some(restart_reply),
        ..Setup::default()
    };
    let app = ModeApp::start(setup()).await;
    let id = session(&app).await;
    let (status, body) = app
        .send(
            "POST",
            &format!("/sessions/{id}/goals"),
            Some(json!({"condition": "the report exists"})),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    let goal_id = body["data"]["goal_id"].as_str().unwrap().to_owned();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while app.model.held() != ["hold"] {
        assert!(std::time::Instant::now() < deadline, "never held");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    PHASE.store(1, std::sync::atomic::Ordering::SeqCst);
    let before = app.model.requests().len();

    let app = app.restart(setup()).await;
    app.model.open_gate();
    let done = wait_goals(&app, &id, |g| g["current"].is_null()).await;
    let goal = &done["ended"][0];
    assert_eq!(
        (&goal["goal_id"], &goal["status"]),
        (&json!(goal_id), &json!("met"))
    );
    let evaluations = goal["evaluations"].as_array().unwrap();
    assert_eq!(evaluations.len(), 2, "{goal}");
    assert_eq!(evaluations[1]["reason"], "the report exists");
    let after: Vec<String> = app.model.requests()[before..]
        .iter()
        .map(Value::to_string)
        .filter(|r| !r.contains("judge whether a coding agent"))
        .collect();
    assert!(
        after.is_empty(),
        "no work Turn ran after the restart: {after:?}"
    );
}

/// `goals` → Goal templates: `POST .../goals` with `template` substitutes `$ARGUMENTS`
/// and takes the template's check; an unknown template is `GoalTemplateNotFoundError`.
#[tokio::test]
async fn goal_use_template_substitutes_arguments() {
    let app = ModeApp::start(Setup {
        reply: Some(reply),
        config: json!({"goals": {"templates": {"green": {
            "condition": "$ARGUMENTS and all tests pass", "check": "true"}}}}),
        ..Setup::default()
    })
    .await;
    let id = session(&app).await;
    let (status, body) = app
        .send(
            "POST",
            &format!("/sessions/{id}/goals"),
            Some(json!({"template": "green", "args": "auth module refactored"})),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(
        body["data"]["condition"],
        "auth module refactored and all tests pass"
    );
    assert_eq!(body["data"]["check"], "true");
    assert_eq!(body["data"]["template"], "green");
    let (status, body) = app
        .send(
            "POST",
            &format!("/sessions/{id}/goals"),
            Some(json!({"template": "blue"})),
        )
        .await;
    assert_eq!(
        (status, body["_tag"].as_str()),
        (404, Some("GoalTemplateNotFoundError")),
        "{body}"
    );
}

fn vcs(repo: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

/// A checkout with one commit.
fn init_repository(repo: &std::path::Path) {
    vcs(repo, &["init", "--quiet", "-b", "main"]);
    vcs(repo, &["config", "user.name", "Goals"]);
    vcs(repo, &["config", "user.email", "goals@example.invalid"]);
    std::fs::write(repo.join("README.md"), "goals\n").unwrap();
    vcs(repo, &["add", "README.md"]);
    vcs(repo, &["commit", "--quiet", "-m", "initial"]);
}

/// `goals` → Parallel goals across sessions: `POST .../goals/fork` starts a root Session
/// in its own `goal-<ulid>` worktree on `cyber/goal-<ulid>`, listed with the others,
/// which works on the Goal until it is met while the source Session has none.
#[tokio::test]
async fn goal_fork_starts_background_session_in_worktree() {
    let app = ModeApp::start(Setup {
        reply: Some(reply),
        ..Setup::default()
    })
    .await;
    init_repository(&app.repo);
    let id = session(&app).await;
    let (status, body) = app
        .send(
            "POST",
            &format!("/sessions/{id}/goals/fork"),
            Some(json!({"condition": "all dependencies upgraded with green tests"})),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    let fork = &body["data"];
    let forked = fork["session_id"].as_str().unwrap().to_owned();
    assert_ne!(forked, id);
    let branch = fork["branch"].as_str().unwrap();
    assert!(branch.starts_with("cyber/goal-"), "{fork}");
    let branches = vcs(&app.repo, &["branch", "--list", "cyber/goal-*"]);
    assert!(branches.contains(branch), "{branches}");
    assert_ne!(fork["directory"], json!(app.repo.display().to_string()));
    assert_eq!(fork["goal"]["status"], "active");

    // A root Session of its worktree's Location: `cyber sessions` there lists it.
    let listed = app
        .app
        .runtime
        .list(&cyber_server::runtime::ListFilter {
            directory: fork["directory"].as_str().map(str::to_owned),
            roots_only: true,
            ..Default::default()
        })
        .unwrap();
    assert!(
        listed.sessions.iter().any(|s| s.id == forked),
        "{:?}",
        listed.sessions
    );
    let info = app.app.runtime.state(&forked).await.unwrap().info;
    assert_eq!((info.parent_id, info.mode.as_str()), (None, "default"));
    // Regression: `cyber sessions list` in the source checkout lists the fork too.
    let (status, listed) = app.send("GET", "/sessions", None).await;
    assert_eq!(status, 200, "{listed}");
    let rows = listed["data"]["data"].as_array().unwrap();
    let row = rows.iter().find(|s| s["id"] == json!(forked));
    assert!(row.is_some(), "{listed}");
    assert_eq!(
        info.source_directory.as_deref(),
        Some(app.repo.display().to_string().as_str())
    );
    let done = wait_goals(&app, &forked, |g| g["current"].is_null()).await;
    assert_eq!(done["ended"][0]["status"], "met", "{done}");
    let (_, source) = app
        .send("GET", &format!("/sessions/{id}/goals"), None)
        .await;
    assert!(source["data"]["current"].is_null(), "{source}");
}

/// `goals` → Goals on workflow agents and subagents: the `agent` tool's `goal` becomes
/// the child's active Goal, the call settles once it is met (its check passing) and the
/// result carries `goal_status`.
#[tokio::test]
async fn agent_tool_goal_returns_goal_status() {
    let app = ModeApp::start(Setup {
        reply: Some(reply),
        config: json!({"permissions": {"agent": "allow"}}),
        ..Setup::default()
    })
    .await;
    let id = session(&app).await;
    let call = json!({"prompt": "write the summary", "description": "Write the summary file",
        "goal": {"condition": "the summary is written", "check": "true"}});
    app.run(&id, &format!("TOOL<<agent>>{call}")).await;
    let output = app.outputs(&id).await.pop().unwrap();
    let result: Value = serde_json::from_str(&output).unwrap_or_else(|_| panic!("{output}"));
    assert_eq!(result["goal_status"], "met", "{result}");
    let child = result["id"].as_str().unwrap();
    let goals = app.app.runtime.goals(child).await.unwrap();
    let goal = goals.goals.values().next().unwrap();
    assert_eq!(goal.condition, "the summary is written");
    assert_eq!(goal.evaluations.len(), 1);
    assert_eq!(
        goal.evaluations[0].check.as_ref().unwrap().exit_code,
        Some(0)
    );
}

/// `goals` → Set the active goal: the model's `goal` tool checks `goal.set`, which asks
/// by default (so an unattended Session refuses it); allowed, it sets the Goal.
#[tokio::test]
async fn the_model_goal_tool_asks_by_default() {
    let call = json!({"condition": "the changelog lists every fix"});
    let app = ModeApp::start(Setup {
        reply: Some(reply),
        ..Setup::default()
    })
    .await;
    let id = session(&app).await;
    app.run(&id, &format!("TOOL<<goal>>{call}")).await;
    let output = app.outputs(&id).await.pop().unwrap();
    assert!(output.contains("no interactive approver"), "{output}");
    assert!(app.app.runtime.goals(&id).await.unwrap().goals.is_empty());

    let app = ModeApp::start(Setup {
        reply: Some(reply),
        config: json!({"permissions": {"goal.set": "allow"}}),
        ..Setup::default()
    })
    .await;
    let id = session(&app).await;
    app.run(&id, &format!("TOOL<<goal>>{call}")).await;
    let output = app.outputs(&id).await.pop().unwrap();
    let set: Value = serde_json::from_str(&output).unwrap_or_else(|_| panic!("{output}"));
    let golden = include_str!("../../cyber-tools/tests/goldens/goal.txt");
    let goal_id = set["goal_id"].as_str().unwrap();
    assert_eq!(output.replace(goal_id, "<goal>"), golden.trim_end());
    let done = wait_goals(&app, &id, |g| g["current"].is_null()).await;
    assert_eq!(done["ended"][0]["status"], "met", "{done}");
}
