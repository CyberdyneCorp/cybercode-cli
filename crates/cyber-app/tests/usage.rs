//! `GET /api/v1/usage?scope=session` with its breakdown (`observability-costs` → Session
//! usage breakdown), through the production App's HTTP router with a scripted model: a
//! Session on `test/main` writes a file and delegates to a subagent on `test/small`.

#[path = "support/scripted_model.rs"]
mod scripted_model;

use std::path::Path;
use std::time::Duration;

use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use serde_json::{Value, json};

/// The root writes `a.txt`, then asks the `helper` subagent, then answers; the subagent
/// answers at once.
fn reply(body: &Value) -> Value {
    if body.to_string().contains("Write a short title") {
        return json!({ "content": "Usage" });
    }
    if body["model"] == "small" {
        return json!({ "content": "Helped." });
    }
    let results = body["messages"]
        .as_array()
        .map_or(0, |m| m.iter().filter(|m| m["role"] == "tool").count());
    let (id, name, arguments) = match results {
        0 => (
            "call_write",
            "write",
            json!({ "path": "a.txt", "content": "one\ntwo\n" }),
        ),
        1 => (
            "call_agent",
            "agent",
            json!({ "prompt": "HELP", "agent": "helper" }),
        ),
        _ => return json!({ "content": "Done." }),
    };
    json!({ "tool_calls": [{ "index": 0, "id": id, "type": "function",
        "function": { "name": name, "arguments": arguments.to_string() } }] })
}

fn git(dir: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?}");
}

#[tokio::test]
async fn session_usage_breaks_down_two_models_a_subagent_lines_and_budgets() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = Paths {
        data: root.join("data"),
        config: root.join("config"),
        state: root.join("state"),
        cache: root.join("cache"),
        tmp: root.join("tmp"),
    };
    paths.ensure().unwrap();
    let model = scripted_model::ScriptedModel::start(reply);
    let mut config = model.config();
    config["providers"]["test"]["models"] = json!({
        "main": { "cost": { "input": 10.0, "output": 20.0 } },
        "small": { "cost": { "input": 1.0, "output": 2.0 } },
    });
    config["agents"] = json!({ "helper": {
        "description": "Helps", "mode": "subagent", "model": "test/small" } });
    config["permissions"] = json!({ "agent": "allow" });
    config["budgets"] = json!({
        "session": { "max_cost_usd": 5.0, "max_turns": 40 },
        "daily": { "max_cost_usd": 20.0 },
    });
    std::fs::write(paths.config.join("cyber.json"), config.to_string()).unwrap();
    let app = App::build(AppOptions {
        paths,
        home: root.join("home"),
        database: DatabaseLocation::Memory,
        default_directory: root.clone(),
        sandbox_policy: None,
        snapshots: true,
        interactive: true,
        password: Some("usage-password".into()),
        mcp_credentials: None,
        remote_skill_fetcher: None,
        workflow_cores: None,
    })
    .await
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/api/v1", listener.local_addr().unwrap());
    tokio::spawn(cyber_server::http::serve_tcp(
        app.router(),
        listener,
        std::future::pending::<()>(),
    ));
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    let send = |method: reqwest::Method, path: String, body: Option<Value>| {
        let mut request = http
            .request(method, format!("{base}{path}"))
            .basic_auth("cyber", Some("usage-password"))
            .header("x-cyber-directory", work.display().to_string());
        if let Some(body) = body {
            request = request.json(&body);
        }
        async move {
            let response = request.send().await.unwrap();
            assert!(
                response.status().is_success(),
                "{path}: {}",
                response.status()
            );
            response.json::<Value>().await.unwrap()
        }
    };
    let created = send(
        reqwest::Method::POST,
        "/sessions".into(),
        Some(json!({ "model": "test/main", "mode": "accept-edits" })),
    )
    .await;
    let id = created["data"]["id"].as_str().unwrap().to_owned();
    send(
        reqwest::Method::POST,
        format!("/sessions/{id}/prompt"),
        Some(json!({ "parts": [{ "type": "text", "text": "Write and delegate" }] })),
    )
    .await;
    let mut done = false;
    for _ in 0..1000 {
        let page = send(
            reqwest::Method::GET,
            format!("/sessions/{id}/messages?order=asc&limit=50"),
            None,
        )
        .await;
        if !app.runtime.is_running(&id) && page["data"].to_string().contains("Done.") {
            done = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    if !done {
        let page = send(
            reqwest::Method::GET,
            format!("/sessions/{id}/messages?order=asc&limit=50"),
            None,
        )
        .await;
        panic!("the Session did not finish: {page}");
    }

    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let usage = send(
        reqwest::Method::GET,
        format!("/usage?scope=session&id={id}"),
        None,
    )
    .await;
    let data = &usage["data"];
    let models: Vec<String> = data["by_model"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            format!(
                "{}/{}",
                m["provider"].as_str().unwrap(),
                m["model"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(models, ["test/main", "test/small"], "{data}");
    let cost: f64 = data["by_model"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["cost"].as_f64().unwrap())
        .sum();
    let total = data["total"]["cost"].as_f64().unwrap();
    assert!((cost - total).abs() < 1e-12, "{cost} != {total}");
    assert!(
        data["descendants"]["cost"].as_f64().unwrap() > 0.0,
        "{data}"
    );
    let tokens: u64 = data["by_model"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["total_tokens"].as_u64().unwrap())
        .sum();
    assert_eq!(tokens, data["total"]["total_tokens"].as_u64().unwrap());
    // Existing fields keep their meaning.
    assert_eq!(data["scope"], "session");
    assert_eq!(data["id"], id.as_str());

    let api = data["api_duration_ms"].as_u64().unwrap();
    let wall = data["wall_duration_ms"].as_u64().unwrap();
    assert!(api <= wall, "{api} > {wall}");
    assert_eq!(data["lines_added"], 2, "{data}");
    assert_eq!(data["lines_removed"], 0);
    assert_eq!(data["backfilling"], false);

    let budgets = data["budgets"].as_array().unwrap();
    assert_eq!(budgets.len(), 2, "{data}");
    assert_eq!(budgets[0]["scope"], "session");
    assert_eq!(
        budgets[0]["limits"],
        json!([
            { "name": "max_cost_usd", "limit": 5.0, "used": total },
            { "name": "max_turns", "limit": 40.0, "used": 4.0 },
        ])
    );
    assert!(budgets[0].get("reset_ms").is_none());
    assert_eq!(budgets[1]["scope"], "daily");
    assert_eq!(budgets[1]["limits"][0]["limit"], 20.0);
    assert!(budgets[1]["limits"][0]["used"].as_f64().unwrap() >= total);
    let reset = budgets[1]["reset_ms"].as_i64().unwrap();
    assert!(
        reset > before && reset <= before + 25 * 3_600_000,
        "{reset}"
    );
    assert!(!budgets[1]["zone"].as_str().unwrap().is_empty());
}

/// Regression: a configuration error in the Session's directory failed the whole
/// report, so `/cost` and the Usage tab broke over a `budgets.daily` typo or a
/// configuration file that does not parse.
#[tokio::test]
async fn session_usage_is_served_when_the_daily_budget_cannot_be_read() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = Paths {
        data: root.join("data"),
        config: root.join("config"),
        state: root.join("state"),
        cache: root.join("cache"),
        tmp: root.join("tmp"),
    };
    paths.ensure().unwrap();
    let model = scripted_model::ScriptedModel::start(|_| json!({ "content": "ok" }));
    let global = paths.config.join("cyber.json");
    std::fs::write(&global, model.config().to_string()).unwrap();
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let app = App::build(AppOptions {
        paths,
        home: root.join("home"),
        database: DatabaseLocation::Memory,
        default_directory: work.clone(),
        sandbox_policy: None,
        snapshots: false,
        interactive: true,
        password: Some("usage-password".into()),
        mcp_credentials: None,
        remote_skill_fetcher: None,
        workflow_cores: None,
    })
    .await
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/api/v1", listener.local_addr().unwrap());
    tokio::spawn(cyber_server::http::serve_tcp(
        app.router(),
        listener,
        std::future::pending::<()>(),
    ));
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let request = |method: reqwest::Method, path: String| {
        http.request(method, format!("{base}{path}"))
            .basic_auth("cyber", Some("usage-password"))
            .header("x-cyber-directory", work.display().to_string())
    };
    let created: Value = request(reqwest::Method::POST, "/sessions".into())
        .json(&json!({ "model": "test/main", "mode": "default" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["data"]["id"].as_str().unwrap().to_owned();
    let mut malformed = model.config();
    malformed["budgets"] = json!({ "daily": { "max_cost_usd": "lots" } });
    for broken in [malformed.to_string(), "{ not json".to_owned()] {
        std::fs::write(&global, &broken).unwrap();
        let response = request(
            reqwest::Method::GET,
            format!("/usage?scope=session&id={id}"),
        )
        .send()
        .await
        .unwrap();
        let status = response.status().as_u16();
        let body: Value = response.json().await.unwrap();
        assert_eq!(status, 200, "{broken}: {body}");
        let data = &body["data"];
        assert_eq!(data["scope"], "session", "{data}");
        assert!(data["total"].is_object(), "{data}");
        assert_eq!(data["budgets"], json!([]), "{data}");
        assert!(
            !data["budgets_error"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "{data}"
        );
    }
}
