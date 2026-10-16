//! `GET /api/v1/usage/stats` (`observability-costs` → Usage statistics API) through the
//! production App's HTTP router with a scripted model: two projects, a priced and an
//! unpriced model, streaks over earlier days, the project scope and refused periods; and
//! `GET /api/v1/usage/insights` (Usage insights).

#[path = "support/scripted_model.rs"]
mod scripted_model;

use std::path::{Path, PathBuf};
use std::time::Duration;

use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use serde_json::{Value, json};

const DAY: i64 = 86_400_000;

fn reply(body: &Value) -> Value {
    if body.to_string().contains("Write a short title") {
        return json!({ "content": "Stats" });
    }
    json!({ "content": "Done." })
}

struct Fixture {
    app: App,
    base: String,
    http: reqwest::Client,
    root: PathBuf,
    _model: scripted_model::ScriptedModel,
    _temp: tempfile::TempDir,
}

impl Fixture {
    async fn start() -> Self {
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
            "small": {},
        });
        config["small_model"] = json!("test/small");
        std::fs::write(paths.config.join("cyber.json"), config.to_string()).unwrap();
        let app = App::build(AppOptions {
            paths,
            home: root.join("home"),
            database: DatabaseLocation::Memory,
            default_directory: root.clone(),
            sandbox_policy: None,
            snapshots: false,
            interactive: true,
            password: Some("stats-password".into()),
            mcp_credentials: None,
            remote_skill_fetcher: None,
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
        Self {
            app,
            base,
            http,
            root,
            _model: model,
            _temp: temp,
        }
    }

    fn project(&self, name: &str) -> PathBuf {
        let dir = self.root.join(name);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        dir
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        dir: &Path,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .basic_auth("cyber", Some("stats-password"))
            .header("x-cyber-directory", dir.display().to_string());
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (
            status,
            response.json::<Value>().await.unwrap_or(Value::Null),
        )
    }

    /// One answered Session in `dir`.
    async fn answered(&self, dir: &Path) -> String {
        let (_, created) = self
            .send(
                reqwest::Method::POST,
                "/sessions",
                dir,
                Some(json!({ "model": "test/main" })),
            )
            .await;
        let id = created["data"]["id"].as_str().unwrap().to_owned();
        let prompt = json!({ "parts": [{ "type": "text", "text": "Hello" }] });
        self.send(
            reqwest::Method::POST,
            &format!("/sessions/{id}/prompt"),
            dir,
            Some(prompt),
        )
        .await;
        for _ in 0..1000 {
            let (_, session) = self
                .send(reqwest::Method::GET, &format!("/sessions/{id}"), dir, None)
                .await;
            if !self.app.runtime.is_running(&id) && session["data"]["title"] == "Stats" {
                return id;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the Session did not finish");
    }

    async fn stats(&self, query: &str) -> Value {
        let (status, body) = self
            .send(
                reqwest::Method::GET,
                &format!("/usage/stats{query}"),
                &self.root,
                None,
            )
            .await;
        assert_eq!(status, 200, "{body}");
        body["data"].clone()
    }

    async fn insights(&self, query: &str) -> Value {
        let (status, body) = self
            .send(
                reqwest::Method::GET,
                &format!("/usage/insights{query}"),
                &self.root,
                None,
            )
            .await;
        assert_eq!(status, 200, "{body}");
        body["data"].clone()
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// A finished Turn of `root` in `directory`, `days_ago` days back.
fn earlier_turn(tx: &rusqlite::Connection, root: &str, directory: &Path, days_ago: i64) {
    let at = now_ms() - days_ago * DAY;
    tx.execute(
        "INSERT INTO usage_step (event_id, session_id, root_id, agent, purpose, provider, model,
            started_ms, ended_ms, input_tokens, output_tokens, reasoning_tokens, cache_read_tokens,
            cache_write_tokens, prompt_tokens, cost, directory)
         VALUES (?1, ?2, ?2, 'build', 'turn', 'test', 'main', ?3, ?3, 1000, 0, 0, 0, 0, 1000, 0.01, ?4)",
        rusqlite::params![
            format!("evt_earlier_{days_ago}"),
            root,
            at,
            directory.display().to_string()
        ],
    )
    .unwrap();
}

#[tokio::test]
async fn stats_cover_days_streaks_models_and_the_project_scope() {
    let fx = Fixture::start().await;
    let a = fx.project("a");
    let b = fx.project("b");
    let first = fx.answered(&a.join("src")).await;
    let second = fx.answered(&b).await;

    let all = fx.stats("?tz=UTC").await;
    assert_eq!(all["period"], "all");
    assert_eq!(all["zone"], "UTC");
    assert!(all["since_ms"].is_null());
    assert_eq!(all["totals"]["sessions"], 2, "{all}");
    // Each Session ran one Turn on `main` and one title call on `small`.
    assert_eq!(all["totals"]["turns"], 2);
    assert_eq!(all["totals"]["steps"], 4);
    assert_eq!(all["totals"]["unpriced_steps"], 2);
    assert_eq!(all["favorite_model"]["model"], "main");
    let models: Vec<&str> = all["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["model"].as_str().unwrap())
        .collect();
    assert_eq!(models, ["main", "small"]);
    assert!(all["models"][1]["cost"].is_null(), "{all}");
    let share: f64 = all["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["share"].as_f64().unwrap())
        .sum();
    assert!((share - 1.0).abs() < 1e-9);
    // The ledger agrees with the Session totals.
    let mut tokens = 0;
    for (id, dir) in [(&first, &a), (&second, &b)] {
        let (_, usage) = fx
            .send(
                reqwest::Method::GET,
                &format!("/usage?scope=session&id={id}"),
                dir,
                None,
            )
            .await;
        tokens += usage["data"]["total"]["total_tokens"].as_u64().unwrap();
    }
    assert_eq!(all["totals"]["total_tokens"], tokens);
    assert_eq!(all["active_days"], 1);
    assert_eq!(all["period_days"], 1);
    assert_eq!(all["current_streak"], 1);
    assert_eq!(all["days"].as_array().unwrap().len(), 1);
    assert_eq!(all["longest_session"]["title"], "Stats");
    assert_eq!(all["approximate_days"], false);
    assert_eq!(all["backfilling"], false);

    // Only Sessions of the project of `a/src` (the git root `a`).
    let scoped = fx
        .stats(&format!(
            "?tz=UTC&directory={}",
            a.join("src").display().to_string().replace('\\', "%5C")
        ))
        .await;
    assert_eq!(scoped["totals"]["sessions"], 1, "{scoped}");
    assert_eq!(scoped["longest_session"]["id"], first.as_str());

    // Turns on each of the 13 days before today make a 14-day streak.
    let directory = a.clone();
    let root = first.clone();
    fx.app
        .store
        .transaction(move |tx| {
            for days_ago in 1..=13 {
                earlier_turn(tx, &root, &directory, days_ago);
            }
            earlier_turn(tx, &root, &directory, 20);
            Ok(())
        })
        .unwrap();
    let month = fx.stats("?period=30d&tz=UTC").await;
    assert_eq!(month["period"], "30d");
    assert_eq!(month["days"].as_array().unwrap().len(), 30);
    assert_eq!(month["current_streak"], 14, "{month}");
    assert_eq!(month["longest_streak"], 14);
    assert_eq!(month["active_days"], 15);
    assert_eq!(month["period_days"], 30);
    assert!(month["since_ms"].as_i64().unwrap() <= now_ms() - 29 * DAY);
    let week = fx.stats("?period=7d&tz=UTC").await;
    assert_eq!(week["current_streak"], 7);
    assert_eq!(week["active_days"], 7);
    assert_eq!(
        week["totals"]["turns"].as_u64().unwrap(),
        2 + 6,
        "two Turns today and one on each of the six days before"
    );
    let all = fx.stats("?tz=UTC").await;
    assert_eq!(all["period_days"], 21);
    assert_eq!(all["longest_streak"], 14);
    assert_eq!(all["active_days"], 15);
}

#[tokio::test]
async fn bad_periods_are_refused_and_unknown_zones_fall_back() {
    let fx = Fixture::start().await;
    for bad in ["0d", "372d", "week", "7"] {
        let (status, body) = fx
            .send(
                reqwest::Method::GET,
                &format!("/usage/stats?period={bad}"),
                &fx.root,
                None,
            )
            .await;
        assert_eq!(status, 400, "{bad}: {body}");
    }
    let (status, _) = fx
        .send(reqwest::Method::GET, "/usage/stats?scope=x", &fx.root, None)
        .await;
    assert_eq!(status, 400);
    let fallback = fx.stats("?tz=..%2F..%2Fetc%2Fpasswd").await;
    let server = fx.stats("").await;
    assert_eq!(fallback["zone"], server["zone"]);
    assert_eq!(fx.stats("?tz=Asia%2FTokyo").await["zone"], "Asia/Tokyo");
    // Nothing ran: zero days, no favorites, cost unknown rather than zero.
    let empty = fx.stats("?period=7d").await;
    assert_eq!(empty["days"].as_array().unwrap().len(), 7);
    assert_eq!(empty["active_days"], 0);
    assert!(empty["favorite_model"].is_null());
    assert!(empty["totals"]["cost"].is_null());
}

/// A finished call of `session` (in `root`'s tree) with `tokens` prompt tokens.
fn ledger_call(
    tx: &rusqlite::Connection,
    id: &str,
    session: &str,
    root: &str,
    run: Option<&str>,
    tokens: i64,
) {
    let at = now_ms() - 60_000;
    tx.execute(
        "INSERT INTO usage_step (event_id, session_id, root_id, parent_id, workflow_run_id, agent,
            purpose, provider, model, started_ms, ended_ms, input_tokens, output_tokens,
            reasoning_tokens, cache_read_tokens, cache_write_tokens, prompt_tokens, cost, directory)
         VALUES (?1, ?2, ?3, ?3, ?4, 'helper', 'turn', 'test', 'main', ?5, ?5, ?6, 0, 0, 0, 0, ?6,
            0.01, '/elsewhere')",
        rusqlite::params![id, session, root, run, at, tokens],
    )
    .unwrap();
}

/// `GET /api/v1/usage/insights` (`observability-costs` → Usage insights) through the
/// production App: a Session whose forked child and Workflow Run child used most of the
/// tree, at a context above 150,000 tokens, with an unpriced title call.
#[tokio::test]
async fn insights_report_factors_subagent_groups_and_the_scope_note() {
    let fx = Fixture::start().await;
    let dir = fx.project("a");
    let root = fx.answered(&dir).await;
    let fork = fx
        .app
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: dir.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(root.clone()),
            fork_from: Some(root.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let before = fx.insights("").await;
    assert_eq!(before["window"], "24h");
    // The title call on `small` is unpriced, so shares are shares of tokens.
    assert_eq!(before["basis"], "tokens", "{before}");
    assert_eq!(before["scope"], "server");
    assert_eq!(before["note"], "Covers only Sessions stored by this server");
    let base = before["total"].as_f64().unwrap();
    assert!(base > 0.0);
    assert!(before["subagents"].as_array().unwrap().is_empty());

    let (r, f) = (root.clone(), fork.clone());
    fx.app
        .store
        .transaction(move |tx| {
            ledger_call(tx, "evt_fork", &f, &r, None, 200_000);
            ledger_call(tx, "evt_run", "ses_runchild", &r, Some("wfr_1"), 100_000);
            Ok(())
        })
        .unwrap();
    let after = fx.insights("?window=24h").await;
    let total = after["total"].as_f64().unwrap();
    assert_eq!(total, base + 300_000.0, "{after}");
    let factor = |name: &str| {
        after["factors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["factor"] == name)
            .unwrap_or_else(|| panic!("{name}: {after}"))["share"]
            .as_f64()
            .unwrap()
    };
    // Descendants used more than half of the only tree: all of it is subagent-heavy.
    assert!((factor("subagent_heavy") - 1.0).abs() < 1e-9);
    assert!((factor("large_context") - 200_000.0 / total).abs() < 1e-9);
    assert_eq!(factor("long_running"), 0.0);
    let groups: Vec<(&str, &str)> = after["subagents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["group"].as_str().unwrap(), s["kind"].as_str().unwrap()))
        .collect();
    assert_eq!(groups, [("fork", "fork"), ("workflow", "workflow")]);

    // The project scope leaves out Sessions elsewhere; the week window is accepted.
    let scoped = fx
        .insights(&format!(
            "?window=7d&directory={}",
            dir.display().to_string().replace('\\', "%5C")
        ))
        .await;
    assert_eq!(scoped["window"], "7d");
    assert_eq!(scoped["total"].as_f64().unwrap(), base, "{scoped}");
    for bad in ["1d", "week", "24"] {
        let (status, body) = fx
            .send(
                reqwest::Method::GET,
                &format!("/usage/insights?window={bad}"),
                &fx.root,
                None,
            )
            .await;
        assert_eq!(status, 400, "{bad}: {body}");
    }
}
