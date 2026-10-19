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
