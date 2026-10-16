//! `GET /api/v1/vcs` (`vcs-integration` → Git awareness) and the Session-wide
//! `GET .../diff`, through the production App's HTTP router with a scripted model.

#[path = "support/scripted_model.rs"]
mod scripted_model;

use std::path::Path;
use std::time::Duration;

use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use serde_json::{Value, json};

struct Fixture {
    _temp: tempfile::TempDir,
    root: std::path::PathBuf,
    app: App,
    base: String,
    http: reqwest::Client,
    _model: scripted_model::ScriptedModel,
}

/// Each prompt names a file; the model writes it, then answers once the tool result is in.
fn reply(body: &Value) -> Value {
    let text = body.to_string();
    if text.contains("Write a short title") {
        return json!({ "content": "Files" });
    }
    let last = body["messages"]
        .as_array()
        .and_then(|m| m.last())
        .cloned()
        .unwrap_or_default();
    if last["role"] == "tool" {
        return json!({ "content": "Done." });
    }
    let file = if last.to_string().contains("SECOND") {
        "b.txt"
    } else {
        "a.txt"
    };
    let arguments = json!({ "path": file, "content": "one\ntwo\n" }).to_string();
    json!({ "tool_calls": [{ "index": 0, "id": format!("call_{file}"), "type": "function",
        "function": { "name": "write", "arguments": arguments } }] })
}

impl Fixture {
    async fn new() -> Self {
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
        std::fs::write(paths.config.join("cyber.json"), model.config().to_string()).unwrap();
        let app = App::build(AppOptions {
            paths,
            home: root.join("home"),
            database: DatabaseLocation::Memory,
            default_directory: root.clone(),
            sandbox_policy: None,
            snapshots: true,
            interactive: true,
            password: Some("vcs-password".into()),
            mcp_credentials: None,
            remote_skill_fetcher: None,
        })
        .await
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(cyber_server::http::serve_tcp(
            app.router(),
            listener,
            std::future::pending::<()>(),
        ));
        Self {
            _temp: temp,
            root,
            app,
            base,
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            _model: model,
        }
    }

    /// A request in `directory` (the `x-cyber-directory` Location).
    async fn send(&self, method: &str, directory: &Path, path: &str, body: Option<Value>) -> Value {
        let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        let mut request = self
            .http
            .request(method, format!("{}/api/v1{path}", self.base))
            .basic_auth("cyber", Some("vcs-password"))
            .header("x-cyber-directory", directory.display().to_string());
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        assert!(
            response.status().is_success(),
            "{path}: {}",
            response.status()
        );
        response.json().await.unwrap_or(Value::Null)
    }

    async fn get(&self, directory: &Path, path: &str) -> Value {
        self.send("GET", directory, path, None).await
    }

    async fn vcs(&self, directory: &Path) -> Value {
        self.get(directory, "/vcs").await
    }
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
async fn vcs_reports_null_outside_git_and_the_branch_state_inside() {
    let f = Fixture::new().await;
    let plain = f.root.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let outside = f.vcs(&plain).await;
    assert_eq!(outside["data"], json!({ "vcs": null }), "{outside}");

    let origin = f.root.join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "-q", "-b", "main"]);
    git(&origin, &["commit", "-q", "--allow-empty", "-m", "one"]);
    git(&f.root, &["clone", "-q", "origin", "repo"]);
    let repo = f.root.join("repo");
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "two"]);
    std::fs::write(repo.join("notes.txt"), "x").unwrap();
    let inside = f.vcs(&repo).await;
    let vcs = &inside["data"]["vcs"];
    assert_eq!(vcs["branch"], "main", "{inside}");
    assert_eq!(vcs["upstream"], "origin/main");
    assert_eq!(
        (vcs["ahead"].as_u64(), vcs["behind"].as_u64()),
        (Some(1), Some(0))
    );
    assert_eq!(vcs["dirty"], true);
    assert_eq!(vcs["worktree"], Value::Null);
    assert_eq!(
        Path::new(vcs["root"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        repo.canonicalize().unwrap()
    );
    let location = inside["location"]["directory"].as_str().unwrap();
    assert_eq!(
        Path::new(location).canonicalize().unwrap(),
        repo.canonicalize().unwrap()
    );
}

async fn wait_idle(f: &Fixture, directory: &Path, id: &str, turns: usize) {
    for _ in 0..1000 {
        let page = f
            .get(
                directory,
                &format!("/sessions/{id}/messages?order=asc&limit=50"),
            )
            .await;
        let done = page["data"].to_string().matches("Done.").count();
        if !f.app.runtime.is_running(id) && done >= turns {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{id} did not finish");
}

#[tokio::test]
async fn the_session_diff_without_a_message_lists_every_changed_file() {
    let f = Fixture::new().await;
    let work = f.root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    // Snapshots track git worktrees.
    git(&work, &["init", "-q", "-b", "main"]);
    let created = f
        .send(
            "POST",
            &work,
            "/sessions",
            Some(json!({ "model": "test/main", "mode": "accept-edits" })),
        )
        .await;
    let id = created["data"]["id"].as_str().unwrap().to_owned();
    let empty = f.get(&work, &format!("/sessions/{id}/diff")).await;
    assert_eq!(empty["data"], json!([]), "{empty}");
    for (n, text) in ["write FIRST", "write SECOND"].iter().enumerate() {
        f.send(
            "POST",
            &work,
            &format!("/sessions/{id}/prompt"),
            Some(json!({ "parts": [{ "type": "text", "text": text }] })),
        )
        .await;
        wait_idle(&f, &work, &id, n + 1).await;
    }
    let diff = f.get(&work, &format!("/sessions/{id}/diff")).await;
    let files: Vec<(&str, u64)> = diff["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["file"].as_str().unwrap(),
                d["additions"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(files, vec![("a.txt", 2), ("b.txt", 2)], "{diff}");
}
