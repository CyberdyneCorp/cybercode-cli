//! `GET /api/v1/files`, `/files/search` and `/files/content` (`server-api` → File
//! browsing routes) through the production App's HTTP router with a scripted model:
//! ignore handling, git status, per-entry `read` decisions from the Session's rules, an
//! `ask` that raises the normal permission request, project confinement and binary
//! detection.

#[path = "support/scripted_model.rs"]
mod scripted_model;

use std::path::{Path, PathBuf};
use std::time::Duration;

use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use serde_json::{Value, json};

const PASSWORD: &str = "files-password";

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    project: PathBuf,
    _app: App,
    base: String,
    http: reqwest::Client,
    _model: scripted_model::ScriptedModel,
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

/// A git project with an ignored `target/`, a committed then modified file, an
/// untracked one, a denied key, an `.env` and a binary blob.
fn project(root: &Path) -> PathBuf {
    let project = root.join("project");
    for dir in ["crates/a/src", "crates/b", "target/debug", "secrets"] {
        std::fs::create_dir_all(project.join(dir)).unwrap();
    }
    std::fs::write(project.join(".gitignore"), "target/\n").unwrap();
    std::fs::write(project.join("crates/a/src/lib.rs"), "fn a() {}\n").unwrap();
    std::fs::write(project.join("crates/b/main.rs"), "fn main() {}\n").unwrap();
    git(&project, &["init", "-q", "-b", "main"]);
    git(&project, &["add", "."]);
    git(&project, &["commit", "-q", "-m", "one"]);
    std::fs::write(project.join("crates/a/src/lib.rs"), "fn a() { 1 }\n").unwrap();
    std::fs::write(project.join("crates/new.rs"), "new\n").unwrap();
    std::fs::write(project.join("target/debug/out"), "x").unwrap();
    std::fs::write(project.join("secrets/key.pem"), "PRIVATE").unwrap();
    std::fs::write(project.join(".env"), "TOKEN=1\n").unwrap();
    std::fs::write(project.join("blob.bin"), [0u8, 159, 146, 150]).unwrap();
    project
}

impl Fixture {
    async fn new() -> Self {
        Self::with(json!({ "read": { "secrets/*": "deny" } })).await
    }

    /// A Fixture whose config sets `permissions`.
    async fn with(permissions: Value) -> Self {
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
        let mut config = model.config();
        config["permissions"] = permissions;
        std::fs::write(paths.config.join("cyber.json"), config.to_string()).unwrap();
        let project = project(&root);
        let app = App::build(AppOptions {
            paths,
            home: root.join("home"),
            database: DatabaseLocation::Memory,
            default_directory: project.clone(),
            sandbox_policy: None,
            snapshots: false,
            interactive: true,
            password: Some(PASSWORD.into()),
            mcp_credentials: None,
            remote_skill_fetcher: None,
            workflow_cores: None,
            isolation: None,
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
            project,
            _app: app,
            base,
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            _model: model,
        }
    }

    /// The status and JSON body of a request in the project's Location.
    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        let mut request = self
            .http
            .request(method, format!("{}/api/v1{path}", self.base))
            .basic_auth("cyber", Some(PASSWORD))
            .header("x-cyber-directory", self.project.display().to_string());
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        self.send("GET", path, None).await
    }

    async fn session(&self) -> String {
        let (status, created) = self
            .send(
                "POST",
                "/sessions",
                Some(json!({ "model": "test/main", "mode": "default" })),
            )
            .await;
        assert_eq!(status, 201, "{created}");
        created["data"]["id"].as_str().unwrap().to_owned()
    }
}

fn names(listing: &Value) -> Vec<&str> {
    listing["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect()
}

fn entry<'a>(listing: &'a Value, name: &str) -> &'a Value {
    listing["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == name)
        .unwrap_or_else(|| panic!("{name} in {listing}"))
}

#[tokio::test]
async fn a_level_lists_directories_first_with_git_status_and_ignored_entries_only_when_hidden() {
    let f = Fixture::new().await;
    let (status, top) = f.get("/files").await;
    assert_eq!(status, 200, "{top}");
    assert_eq!(names(&top), ["crates", "secrets", "blob.bin"], "{top}");
    assert_eq!(top["data"]["hidden"], 4, "{top}");
    // lib.rs modified, new.rs and blob.bin untracked (`.env`, `secrets/` too).
    assert_eq!(top["data"]["changed"], 5, "{top}");
    assert_eq!(entry(&top, "crates")["git_status"], "M");
    assert_eq!(entry(&top, "crates")["kind"], "directory");
    assert_eq!(entry(&top, "blob.bin")["git_status"], "?");
    assert_eq!(entry(&top, "blob.bin")["size"], 4);
    assert_eq!(entry(&top, "crates")["read"], "allow");

    let (_, all) = f.get("/files?hidden=true").await;
    assert_eq!(entry(&all, "target")["ignored"], true);
    assert_eq!(entry(&all, ".git")["ignored"], true);
    assert_eq!(entry(&all, ".env")["ignored"], false);
    assert_eq!(entry(&all, ".env")["read"], "ask");

    let (_, crates) = f.get("/files?path=crates").await;
    assert_eq!(names(&crates), ["a", "b", "new.rs"]);
    assert_eq!(entry(&crates, "a")["git_status"], "M");
    assert_eq!(entry(&crates, "b")["git_status"], Value::Null);
    assert_eq!(entry(&crates, "new.rs")["git_status"], "?");
    assert_eq!(entry(&crates, "new.rs")["path"], "crates/new.rs");

    let (_, found) = f.get("/files/search?query=alib").await;
    assert_eq!(
        found["data"]["results"],
        json!([{ "path": "crates/a/src/lib.rs", "read": "allow" }])
    );
    let (_, ignored) = f.get("/files/search?query=debugout").await;
    assert_eq!(ignored["data"]["results"], json!([]));
}

#[tokio::test]
async fn paths_and_symlinks_outside_the_project_are_refused_without_their_bytes() {
    let f = Fixture::new().await;
    std::fs::write(f.root.join("outside.txt"), "OUTSIDE SECRET").unwrap();
    for path in ["../outside.txt", "../../etc/passwd", "/etc/passwd"] {
        let (status, body) = f.get(&format!("/files/content?path={path}")).await;
        assert_eq!(status, 403, "{path}: {body}");
        assert_eq!(body["reason"], "outside_project", "{body}");
        assert!(!body.to_string().contains("OUTSIDE SECRET"));
    }
    let (status, body) = f.get("/files?path=..").await;
    assert_eq!((status, &body["reason"]), (403, &json!("outside_project")));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(f.root.join("outside.txt"), f.project.join("link")).unwrap();
        let (status, body) = f.get("/files/content?path=link").await;
        assert_eq!(status, 403, "{body}");
        assert_eq!(body["reason"], "outside_project");
        assert!(!body.to_string().contains("OUTSIDE SECRET"));
        let (_, top) = f.get("/files").await;
        assert_eq!(entry(&top, "link")["kind"], "symlink");
    }
}

#[tokio::test]
async fn a_denied_file_is_marked_and_refused_and_a_binary_file_has_no_text() {
    let f = Fixture::new().await;
    let id = f.session().await;
    let (_, secrets) = f.get(&format!("/files?path=secrets&session_id={id}")).await;
    assert_eq!(entry(&secrets, "key.pem")["read"], "deny", "{secrets}");
    let (status, body) = f
        .get(&format!(
            "/files/content?path=secrets/key.pem&session_id={id}"
        ))
        .await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(
        (&body["reason"], &body["decision"]),
        (&json!("permission"), &json!("deny"))
    );
    assert!(!body.to_string().contains("PRIVATE"));

    let (status, blob) = f
        .get(&format!("/files/content?path=blob.bin&session_id={id}"))
        .await;
    assert_eq!(status, 200, "{blob}");
    assert_eq!(blob["data"]["binary"], true);
    assert_eq!(blob["data"]["size"], 4);
    assert!(blob["data"].get("text").is_none());

    let (_, text) = f.get("/files/content?path=crates/a/src/lib.rs").await;
    assert_eq!(text["data"]["text"], "fn a() { 1 }\n");
    assert_eq!(text["data"]["language"], "rust");
    assert_eq!(text["data"]["lines"], 1);
}

/// The pending permission requests of `id`.
async fn pending(f: &Fixture, id: &str) -> Vec<Value> {
    let (_, list) = f
        .get(&format!("/permissions/requests?session_id={id}"))
        .await;
    list["data"].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn an_ask_file_raises_the_permission_request_and_content_comes_only_after_approval() {
    let f = Fixture::new().await;
    // Without a Session no one can be asked.
    let (status, body) = f.get("/files/content?path=.env").await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["decision"], "ask");

    let id = f.session().await;
    let fetch = |path: String| {
        let (http, base) = (f.http.clone(), f.base.clone());
        let directory = f.project.display().to_string();
        tokio::spawn(async move {
            let response = http
                .get(format!("{base}/api/v1{path}"))
                .basic_auth("cyber", Some(PASSWORD))
                .header("x-cyber-directory", directory)
                .send()
                .await
                .unwrap();
            let status = response.status().as_u16();
            (status, response.json::<Value>().await.unwrap())
        })
    };
    for (reply, approved) in [("once", true), ("reject", false)] {
        let request = fetch(format!("/files/content?path=.env&session_id={id}"));
        let mut asked = Vec::new();
        for _ in 0..500 {
            asked = pending(&f, &id).await;
            if !asked.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let [ask] = &asked[..] else {
            panic!("one request: {asked:?}")
        };
        assert_eq!(ask["action"], "read");
        assert_eq!(ask["resources"], json!([".env"]));
        assert!(!request.is_finished(), "no content before the reply");
        let request_id = ask["id"].as_str().unwrap();
        let (status, _) = f
            .send(
                "POST",
                &format!("/sessions/{id}/permissions/{request_id}/reply"),
                Some(json!({ "reply": reply })),
            )
            .await;
        assert!(status < 300, "{status}");
        let (status, body) = request.await.unwrap();
        if approved {
            assert_eq!(status, 200, "{body}");
            assert_eq!(body["data"]["text"], "TOKEN=1\n");
        } else {
            assert_eq!(status, 403, "{body}");
            assert_eq!(body["reason"], "permission");
            assert!(!body.to_string().contains("TOKEN"));
        }
    }
}

/// Regression: content was read by re-resolving the name after the `ask` reply, so a
/// denied file swapped in under the approved name during the wait was served.
#[tokio::test]
async fn a_file_swapped_while_its_ask_waits_is_refused_without_its_bytes() {
    let f = Fixture::new().await;
    let id = f.session().await;
    let fetch = |path: String| {
        let (http, base) = (f.http.clone(), f.base.clone());
        let directory = f.project.display().to_string();
        tokio::spawn(async move {
            let response = http
                .get(format!("{base}/api/v1{path}"))
                .basic_auth("cyber", Some(PASSWORD))
                .header("x-cyber-directory", directory)
                .send()
                .await
                .unwrap();
            let status = response.status().as_u16();
            (status, response.text().await.unwrap())
        })
    };
    let denied = f.project.join("secrets/key.pem");
    let swaps: Vec<fn(&Path, &Path) -> std::io::Result<()>> = vec![
        |from, to| std::fs::hard_link(from, to),
        #[cfg(unix)]
        |from, to| std::os::unix::fs::symlink(from, to),
    ];
    for swap in swaps {
        std::fs::write(f.project.join(".env"), "TOKEN=1\n").unwrap();
        let request = fetch(format!("/files/content?path=.env&session_id={id}"));
        let mut asked = Vec::new();
        for _ in 0..500 {
            asked = pending(&f, &id).await;
            if !asked.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let [ask] = &asked[..] else {
            panic!("one request: {asked:?}")
        };
        std::fs::remove_file(f.project.join(".env")).unwrap();
        swap(&denied, &f.project.join(".env")).unwrap();
        let request_id = ask["id"].as_str().unwrap();
        let (status, _) = f
            .send(
                "POST",
                &format!("/sessions/{id}/permissions/{request_id}/reply"),
                Some(json!({ "reply": "once" })),
            )
            .await;
        assert!(status < 300, "{status}");
        let (status, body) = request.await.unwrap();
        assert_eq!(status, 403, "{body}");
        assert!(body.contains("\"changed\""), "{body}");
        assert!(!body.contains("PRIVATE"), "{body}");
        std::fs::remove_file(f.project.join(".env")).unwrap();
    }
}

/// Regression: search walked the whole project without `list` on anything, so names
/// under a directory the rules keep from listing (or a project that may not be listed
/// at all) leaked through it.
#[tokio::test]
async fn search_shows_only_names_the_rules_let_a_listing_show() {
    let f = Fixture::with(json!({ "list": { "secrets": "deny", "secrets/**": "deny" } })).await;
    let id = f.session().await;
    let (status, body) = f.get(&format!("/files?path=secrets&session_id={id}")).await;
    assert_eq!(
        (status, &body["reason"]),
        (403, &json!("permission")),
        "{body}"
    );
    let (status, found) = f
        .get(&format!("/files/search?query=e&limit=200&session_id={id}"))
        .await;
    assert_eq!(status, 200, "{found}");
    let paths: Vec<&str> = found["data"]["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| hit["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"crates/new.rs"), "{found}");
    assert!(!found.to_string().contains("key.pem"), "{found}");

    let f = Fixture::with(json!({ "list": "deny" })).await;
    let id = f.session().await;
    let (status, body) = f
        .get(&format!("/files/search?query=lib&session_id={id}"))
        .await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(
        (&body["reason"], &body["decision"]),
        (&json!("permission"), &json!("deny"))
    );
    assert!(!body.to_string().contains("lib.rs"), "{body}");

    // Without a Session no one can approve an `ask`.
    let f = Fixture::with(json!({ "list": "ask" })).await;
    let (status, body) = f.get("/files/search?query=lib").await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(
        (&body["reason"], &body["decision"]),
        (&json!("permission"), &json!("ask"))
    );
}

/// Regression: resolving a path opened it blocking on the async runtime before the
/// permission check, so a FIFO in the project pinned a server worker for good.
#[cfg(unix)]
#[tokio::test]
async fn a_fifo_is_refused_promptly_by_every_route() {
    let f = Fixture::new().await;
    let made = std::process::Command::new("mkfifo")
        .arg(f.project.join("pipe"))
        .status()
        .unwrap();
    assert!(made.success());
    for path in ["/files/content?path=pipe", "/files?path=pipe"] {
        let (status, body) = tokio::time::timeout(Duration::from_secs(10), f.get(path))
            .await
            .expect("answered promptly");
        assert_eq!(status, 400, "{path}: {body}");
    }
    let (status, _) = f.get("/files").await;
    assert_eq!(status, 200, "the server still answers");
}

impl Fixture {
    fn set_permissions(&self, permissions: Value) {
        let path = self.root.join("config/cyber.json");
        let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        config["permissions"] = permissions;
        std::fs::write(path, config.to_string()).unwrap();
    }

    fn background(
        &self,
        method: &str,
        path: String,
        body: Option<Value>,
    ) -> tokio::task::JoinHandle<(u16, Value)> {
        let (http, base, directory) = (
            self.http.clone(),
            self.base.clone(),
            self.project.display().to_string(),
        );
        let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        tokio::spawn(async move {
            let mut request = http
                .request(method, format!("{base}/api/v1{path}"))
                .basic_auth("cyber", Some(PASSWORD))
                .header("x-cyber-directory", directory);
            if let Some(body) = body {
                request = request.json(&body);
            }
            let response = request.send().await.unwrap();
            (response.status().as_u16(), response.json().await.unwrap())
        })
    }

    async fn binary(&self, session: &str, path: &str) -> Value {
        let (status, opened) = self
            .send(
                "POST",
                "/files/binary/documents",
                Some(json!({"session_id":session,"path":path})),
            )
            .await;
        assert_eq!(status, 200, "{opened}");
        opened["data"].clone()
    }
}

#[tokio::test]
async fn binary_pages_and_search_use_the_production_owner_scope_without_inference() {
    use base64::Engine;
    let f = Fixture::new().await;
    let session = f.session().await;
    let opened = f.binary(&session, "blob.bin").await;
    assert_eq!(opened["length"], "4");
    assert_eq!(opened["source"], "live");
    let id = opened["document_id"].as_str().unwrap();
    let revision = opened["revision"].as_str().unwrap();
    let path = format!(
        "/files/binary/documents/{id}/bytes?session_id={session}&revision={revision}&offset=1&limit=3"
    );
    let (status, page) = f.get(&path).await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["data"]["offset"], "1");
    assert_eq!(page["data"]["length"], "3");
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(page["data"]["base64"].as_str().unwrap())
            .unwrap(),
        [159, 146, 150]
    );
    assert_eq!(page["data"]["page_sha256"].as_str().unwrap().len(), 64);
    let (status, found) = f
        .send(
            "POST",
            &format!("/files/binary/documents/{id}/search?session_id={session}"),
            Some(json!({"session_id":session,"revision":revision,"hex":"9f92","offset":"0"})),
        )
        .await;
    assert_eq!(status, 200, "{found}");
    assert_eq!(found["data"]["offsets"], json!(["1"]));
    let (_, messages) = f.get(&format!("/sessions/{session}/messages")).await;
    assert_eq!(
        messages["data"],
        json!([]),
        "manual binary reads must not start a model Turn"
    );
    let other = f.session().await;
    let (status, _) = f.get(&path.replace(&session, &other)).await;
    assert_eq!(status, 404, "opaque ID is not Session authority");
    let (status, _) = f
        .send(
            "DELETE",
            &format!("/files/binary/documents/{id}?session_id={session}"),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(f.get(&path).await.0, 404);
}

#[tokio::test]
async fn binary_read_denial_and_owner_origin_auth_are_not_bypassed_by_edit_allowance() {
    let f = Fixture::with(json!({"read":{"blob.bin":"deny"},"edit":"allow"})).await;
    let session = f.session().await;
    let input = json!({"session_id":session,"path":"blob.bin"});
    let (status, error) = f
        .send("POST", "/files/binary/documents", Some(input.clone()))
        .await;
    assert_eq!(status, 403, "{error}");
    assert_eq!(error["decision"], "deny");
    assert!(!error.to_string().contains("AJ+Slg=="));
    for (auth, origin, expected) in [
        (false, None, 401),
        (true, Some("https://untrusted.invalid"), 403),
    ] {
        let mut request = f
            .http
            .post(format!("{}/api/v1/files/binary/documents", f.base))
            .json(&input);
        if auth {
            request = request.basic_auth("cyber", Some(PASSWORD));
        }
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        assert_eq!(request.send().await.unwrap().status().as_u16(), expected);
    }
}

#[tokio::test]
async fn binary_document_limits_close_capacity_and_stale_file_are_enforced() {
    let f = Fixture::new().await;
    let session = f.session().await;
    let mut opened = Vec::new();
    for _ in 0..4 {
        opened.push(f.binary(&session, "blob.bin").await);
    }
    let (status, body) = f
        .send(
            "POST",
            "/files/binary/documents",
            Some(json!({"session_id":session,"path":"blob.bin"})),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["reason"], "binary_limit");
    let id = opened[0]["document_id"].as_str().unwrap();
    let revision = opened[0]["revision"].as_str().unwrap();
    for query in [
        "offset=01&limit=1",
        "offset=0&limit=1048577",
        "offset=18446744073709551615&limit=1",
    ] {
        assert_eq!(f.get(&format!("/files/binary/documents/{id}/bytes?session_id={session}&revision={revision}&{query}")).await.0,400);
    }
    std::fs::write(f.project.join("blob.bin"), [0, 1, 2, 3, 4]).unwrap();
    let (status, body) = f
        .get(&format!(
            "/files/binary/documents/{id}/bytes?session_id={session}&revision={revision}"
        ))
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["reason"], "binary_changed");
    assert_eq!(
        f.send(
            "DELETE",
            &format!("/files/binary/documents/{id}?session_id={session}"),
            None
        )
        .await
        .0,
        200
    );
    let fresh = f.binary(&session, "blob.bin").await;
    assert_eq!(fresh["length"], "5");
}

#[tokio::test]
async fn binary_existing_document_rechecks_changed_read_policy() {
    let f = Fixture::new().await;
    let session = f.session().await;
    let opened = f.binary(&session, "blob.bin").await;
    let config_path = f.root.join("config/cyber.json");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
    config["permissions"] = json!({"read":"deny"});
    std::fs::write(config_path, config.to_string()).unwrap();
    let id = opened["document_id"].as_str().unwrap();
    let revision = opened["revision"].as_str().unwrap();
    let (status, body) = f
        .get(&format!(
            "/files/binary/documents/{id}/bytes?session_id={session}&revision={revision}"
        ))
        .await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["decision"], "deny");
    assert_eq!(
        f.send(
            "DELETE",
            &format!("/files/binary/documents/{id}?session_id={session}"),
            None
        )
        .await
        .0,
        200,
        "revoked read must not prevent releasing a view"
    );
}

#[tokio::test]
async fn binary_global_capacity_is_released_by_session_deletion_and_project_mismatch_is_refused() {
    let f = Fixture::new().await;
    let mut sessions = Vec::new();
    for _ in 0..4 {
        let session = f.session().await;
        for _ in 0..4 {
            f.binary(&session, "blob.bin").await;
        }
        sessions.push(session);
    }
    let next = f.session().await;
    let (status, error) = f
        .send(
            "POST",
            "/files/binary/documents",
            Some(json!({"session_id":next,"path":"blob.bin"})),
        )
        .await;
    assert_eq!(status, 400, "{error}");
    assert_eq!(error["reason"], "binary_limit");
    let (status, error) = f
        .send(
            "POST",
            "/files/binary/documents",
            Some(json!({"session_id":next,"path":"blob.bin","expected_root":"/another/project"})),
        )
        .await;
    assert_eq!(status, 403, "{error}");
    assert_eq!(error["reason"], "binary_scope");
    let (status, body) = f
        .send("DELETE", &format!("/sessions/{}", sessions[0]), None)
        .await;
    assert!(status < 300, "{status}: {body}");
    f.binary(&next, "blob.bin").await;
}

#[tokio::test]
async fn binary_open_revalidates_the_pinned_file_after_a_real_permission_wait() {
    let f = Fixture::new().await;
    let session = f.session().await;
    let request = {
        let http = f.http.clone();
        let base = f.base.clone();
        let directory = f.project.display().to_string();
        let session = session.clone();
        tokio::spawn(async move {
            let response = http
                .post(format!("{base}/api/v1/files/binary/documents"))
                .basic_auth("cyber", Some(PASSWORD))
                .header("x-cyber-directory", directory)
                .json(&json!({"session_id":session,"path":".env"}))
                .send()
                .await
                .unwrap();
            (
                response.status().as_u16(),
                response.json::<Value>().await.unwrap(),
            )
        })
    };
    let mut asked = Vec::new();
    for _ in 0..500 {
        asked = pending(&f, &session).await;
        if !asked.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let [ask] = &asked[..] else {
        panic!("one read request: {asked:?}");
    };
    assert_eq!(ask["action"], "read");
    assert!(!request.is_finished());
    std::fs::rename(f.project.join(".env"), f.project.join("old-env")).unwrap();
    std::fs::write(f.project.join(".env"), b"REPLACED PRIVATE CONTENT").unwrap();
    let reply = format!(
        "/sessions/{session}/permissions/{}/reply",
        ask["id"].as_str().unwrap()
    );
    assert!(
        f.send("POST", &reply, Some(json!({"reply":"once"})))
            .await
            .0
            < 300
    );
    let (status, body) = request.await.unwrap();
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["reason"], "binary_changed");
    assert!(body.get("data").is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn binary_aliases_authorize_the_canonical_target_and_refuse_retargeting() {
    let f = Fixture::new().await;
    let session = f.session().await;
    std::os::unix::fs::symlink(
        f.project.join("secrets/key.pem"),
        f.project.join("denied-alias"),
    )
    .unwrap();
    let (status, error) = f
        .send(
            "POST",
            "/files/binary/documents",
            Some(json!({"session_id":session,"path":"denied-alias"})),
        )
        .await;
    assert_eq!(status, 403, "{error}");
    assert_eq!(error["decision"], "deny");
    std::os::unix::fs::symlink(f.project.join("blob.bin"), f.project.join("alias")).unwrap();
    let document = f.binary(&session, "alias").await;
    assert_eq!(document["path"], "blob.bin");
    std::fs::remove_file(f.project.join("alias")).unwrap();
    std::os::unix::fs::symlink(f.project.join("secrets/key.pem"), f.project.join("alias")).unwrap();
    let id = document["document_id"].as_str().unwrap();
    let revision = document["revision"].as_str().unwrap();
    let (status, error) = f
        .get(&format!(
            "/files/binary/documents/{id}/bytes?session_id={session}&revision={revision}"
        ))
        .await;
    assert_eq!(status, 400, "{error}");
    assert_eq!(error["reason"], "binary_changed");
    assert!(error.get("data").is_none());
}

#[tokio::test]
async fn binary_page_rechecks_read_denial_after_an_approval_wait() {
    let f = Fixture::new().await;
    let session = f.session().await;
    let document = f.binary(&session, "blob.bin").await;
    f.set_permissions(json!({"read":{"blob.bin":"ask"}}));
    let id = document["document_id"].as_str().unwrap();
    let revision = document["revision"].as_str().unwrap();
    let request = f.background(
        "GET",
        format!("/files/binary/documents/{id}/bytes?session_id={session}&revision={revision}"),
        None,
    );
    let mut asked = Vec::new();
    for _ in 0..500 {
        asked = pending(&f, &session).await;
        if !asked.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let [ask] = &asked[..] else {
        panic!("one read request: {asked:?}");
    };
    assert_eq!(ask["action"], "read");
    assert!(!request.is_finished());
    f.set_permissions(json!({"read":"deny"}));
    let reply = format!(
        "/sessions/{session}/permissions/{}/reply",
        ask["id"].as_str().unwrap()
    );
    assert!(
        f.send("POST", &reply, Some(json!({"reply":"once"})))
            .await
            .0
            < 300
    );
    let (status, body) = request.await.unwrap();
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["decision"], "deny");
    assert!(body.get("data").is_none());
}

#[tokio::test]
async fn binary_close_requires_its_session_but_no_longer_needs_a_live_location() {
    let f = Fixture::new().await;
    let session = f.session().await;
    let other = f.session().await;
    let document = f.binary(&session, "blob.bin").await;
    let id = document["document_id"].as_str().unwrap();
    assert_eq!(
        f.send(
            "DELETE",
            &format!("/files/binary/documents/{id}?session_id={other}"),
            None
        )
        .await
        .0,
        404
    );
    let missing = f.root.join("no-longer-present");
    let response = f
        .http
        .delete(format!(
            "{}/api/v1/files/binary/documents/{id}?session_id={session}",
            f.base
        ))
        .basic_auth("cyber", Some(PASSWORD))
        .header("x-cyber-directory", missing.display().to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"data":true})
    );
}
