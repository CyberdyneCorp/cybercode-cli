//! Actual authenticated memory review over the application's shared storage.
use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};

struct Fixture {
    root: tempfile::TempDir,
    directory: std::path::PathBuf,
    app: App,
    url: String,
    client: Client,
    stop: tokio::sync::oneshot::Sender<()>,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new(repository: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().canonicalize().unwrap();
        if repository {
            assert!(
                std::process::Command::new("git")
                    .args(["init", "--quiet"])
                    .arg(&directory)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let paths = Paths {
            data: directory.join("data"),
            config: directory.join("config"),
            state: directory.join("state"),
            cache: directory.join("cache"),
            tmp: directory.join("tmp"),
        };
        paths.ensure().unwrap();
        let app = App::build(AppOptions {
            paths,
            home: directory.join("home"),
            database: DatabaseLocation::Memory,
            default_directory: directory.clone(),
            sandbox_policy: None,
            snapshots: false,
            interactive: true,
            password: Some("memory-test".into()),
        })
        .await
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/api/v1", listener.local_addr().unwrap());
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let router = cyber_server::http::router(app.state.clone());
        let server = tokio::spawn(async move {
            cyber_server::http::serve_tcp(router, listener, async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
        });
        Self {
            root,
            directory,
            app,
            url,
            client: Client::builder().no_proxy().build().unwrap(),
            stop,
            server,
        }
    }
    fn get(&self, route: &str) -> reqwest::RequestBuilder {
        self.client
            .get(format!("{}{}", self.url, route))
            .basic_auth("cyber", Some("memory-test"))
    }
    async fn close(self) {
        self.stop.send(()).unwrap();
        self.server.await.unwrap();
        self.app.runtime.shutdown().await;
        drop(self.root);
    }
    #[cfg(unix)]
    fn store(&self, scope: &str) -> cyber_core::memory::MemoryStore {
        let project = if scope == "global" {
            "global".into()
        } else {
            cyber_core::project::identify(&self.directory).id
        };
        cyber_core::memory::MemoryStore::open(&self.app.paths.data, &project).unwrap()
    }
}

#[tokio::test]
async fn empty_review_authentication_and_invalid_routing_create_no_scope() {
    let f = Fixture::new(true).await;
    let response = f
        .client
        .get(format!("{}/memory", f.url))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = f.get("/memory").send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value = response.json().await.unwrap();
    assert_eq!(value["data"], json!({"memories":[],"invalid":[]}));
    assert_eq!(
        value["location"]["directory"],
        f.directory.to_str().unwrap()
    );
    for route in [
        "/memory?scope=other",
        "/memory/other/note",
        "/memory/project/Bad_Name",
    ] {
        assert_eq!(
            f.get(route).send().await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }
    let missing = f.get("/memory/project/absent").send().await.unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        missing.json::<Value>().await.unwrap()["_tag"],
        "MemoryNotFoundError"
    );
    assert_eq!(
        f.get("/memory")
            .header(
                "x-cyber-directory",
                f.directory.join("missing").to_str().unwrap()
            )
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        std::fs::read_dir(f.app.paths.data.join("memory"))
            .unwrap()
            .count(),
        0
    );
    f.close().await;
}

#[cfg(unix)]
fn text(name: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: Durable policy\ntype: reference\n---\n\n{body}\n")
}

#[cfg(unix)]
#[tokio::test]
async fn scope_and_location_review_match_cli_storage_without_model_settings() {
    let f = Fixture::new(true).await;
    for (scope, body) in [("project", "repository policy"), ("global", "user policy")] {
        let store = f.store(scope);
        store.claim().unwrap().write(&text("policy", body)).unwrap();
    }
    std::fs::write(
        f.app.paths.config.join("cyber.json"),
        r#"{"memory":{"enabled":false}}"#,
    )
    .unwrap();
    for (scope, body) in [("project", "repository policy"), ("global", "user policy")] {
        let response = f
            .get(&format!("/memory/{scope}/policy"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value = response.json().await.unwrap();
        assert_eq!(value["data"]["body"], body);
        assert_eq!(value["data"]["metadata"]["type"], "reference");
        let catalog: Value = f
            .get(&format!("/memory?scope={scope}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(catalog["data"]["memories"][0]["name"], "policy");
    }
    let outside = tempfile::tempdir().unwrap();
    let sibling = outside.path().canonicalize().unwrap();
    let response = f
        .get("/memory/project/policy")
        .header("x-cyber-directory", f.directory.to_str().unwrap())
        .query(&[("location[directory]", sibling.to_str().unwrap())])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["data"]["body"],
        "user policy"
    );
    f.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn busy_pending_invalid_and_alias_review_preserve_evidence() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new(false).await;
    let store = f.store("global");
    let mut owner = store.claim().unwrap();
    owner.write(&text("policy", "durable fact")).unwrap();
    assert_eq!(
        f.get("/memory").send().await.unwrap().status(),
        StatusCode::CONFLICT
    );
    drop(owner);
    let malformed = store.path().join("malformed.md");
    std::fs::write(&malformed, "private invalid text").unwrap();
    std::fs::set_permissions(&malformed, std::fs::Permissions::from_mode(0o600)).unwrap();
    let response: Value = f.get("/memory").send().await.unwrap().json().await.unwrap();
    assert_eq!(response["data"]["memories"].as_array().unwrap().len(), 1);
    assert_eq!(response["data"]["invalid"][0]["filename"], "malformed.md");
    assert!(!response.to_string().contains("private invalid text"));
    assert_eq!(
        f.get("/memory/global/malformed")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    symlink(
        store.path().join("policy.md"),
        store.path().join("alias.md"),
    )
    .unwrap();
    assert_eq!(
        f.get("/memory/global/alias").send().await.unwrap().status(),
        StatusCode::CONFLICT
    );
    let mut owner = store.claim().unwrap();
    let prepared = owner
        .prepare_write(&text("policy", "updated fact"))
        .unwrap();
    drop(prepared);
    drop(owner);
    for route in ["/memory", "/memory/global/policy"] {
        assert_eq!(
            f.get(route).send().await.unwrap().status(),
            StatusCode::CONFLICT
        );
    }
    assert!(store.path().join(".memory-transaction").exists());
    assert!(
        std::fs::read_to_string(store.path().join("policy.md"))
            .unwrap()
            .contains("durable fact")
    );
    f.close().await;
}

impl Fixture {
    fn put(&self, name: &str, content: &str) -> reqwest::RequestBuilder {
        self.client
            .put(format!("{}/memory/global/{name}", self.url))
            .basic_auth("cyber", Some("memory-test"))
            .json(&json!({"content":content}))
    }
    #[cfg(unix)]
    fn delete(&self, name: &str) -> reqwest::RequestBuilder {
        self.client
            .delete(format!("{}/memory/global/{name}", self.url))
            .basic_auth("cyber", Some("memory-test"))
    }
}

#[tokio::test]
async fn mutation_validation_settings_and_shutdown_refuse_before_scope_creation() {
    let f = Fixture::new(false).await;
    let content =
        "---\nname: policy\ndescription: Durable policy\ntype: reference\n---\n\nDurable fact\n";
    for config in [
        json!({"memory":{"enabled":false}}),
        json!({"memory":{"generate":false}}),
    ] {
        std::fs::write(f.app.paths.config.join("cyber.json"), config.to_string()).unwrap();
        let response = f.put("policy", content).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            response.json::<Value>().await.unwrap()["_tag"],
            "ForbiddenError"
        );
    }
    std::fs::write(f.app.paths.config.join("cyber.json"), "{}").unwrap();
    for (name, text) in [
        ("Bad_Name", content),
        ("other", content),
        ("policy", "not frontmatter"),
        (
            "policy",
            "---\nname: policy\ndescription: Durable policy\ntype: reference\n---\npassword=private-credential\n",
        ),
    ] {
        let response = f.put(name, text).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response.json::<Value>().await.unwrap();
        assert_eq!(body["_tag"], "InvalidRequestError");
        assert!(!body.to_string().contains("private-credential"));
    }
    let invalid = f
        .client
        .put(format!("{}/memory/global/policy", f.url))
        .basic_auth("cyber", Some("memory-test"))
        .json(&json!({"content":content,"force":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        invalid.json::<Value>().await.unwrap()["_tag"],
        "InvalidRequestError"
    );
    #[cfg(not(unix))]
    assert_eq!(
        f.put("policy", content).send().await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    f.app.runtime.shutdown().await;
    assert_eq!(
        f.put("policy", content).send().await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        std::fs::read_dir(f.app.paths.data.join("memory"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        f.app
            .store
            .read(|db| db
                .query_row("SELECT COUNT(*) FROM memory_mutation", [], |r| r
                    .get::<_, i64>(0))
                .map_err(Into::into))
            .unwrap(),
        0
    );
    f.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn actual_http_crud_publishes_once_and_replays_after_response_cache_disposal() {
    use cyber_server::runtime::LiveEvent;
    let f = Fixture::new(false).await;
    let mut live = f.app.runtime.subscribe();
    let mut stream = f
        .client
        .get(format!("{}/event", f.url))
        .basic_auth("cyber", Some("memory-test"))
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    stream.chunk().await.unwrap().unwrap();
    let content = text("policy", "durable preference");
    let response = f
        .put("policy", &content)
        .header("idempotency-key", "memory-write")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let saved: Value = response.json().await.unwrap();
    assert_eq!(saved["data"]["receipt"]["name"], "policy");
    assert_eq!(saved["data"]["receipt"]["deleted"], false);
    assert!(matches!(
        live.try_recv().unwrap(),
        LiveEvent::MemoryUpdated { seq: 2, .. }
    ));
    let frame = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let mut frame = String::new();
        while !frame.contains("\n\n") {
            let chunk = stream.chunk().await.unwrap().unwrap();
            frame.push_str(std::str::from_utf8(&chunk).unwrap());
        }
        frame
    })
    .await
    .unwrap();
    let notification: Value = serde_json::from_str(
        frame
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(notification["type"], "memory.updated.1");
    assert_eq!(notification["data"], saved["data"]);
    assert_eq!(notification["durable"]["aggregateID"], saved["data"]["id"]);
    assert!(!frame.contains("previous_owner_key"));
    assert!(!frame.contains("durable preference"));
    drop(stream);
    let again = f
        .put("policy", &content)
        .header("idempotency-key", "memory-write")
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::OK);
    assert_eq!(again.headers()["idempotent-replayed"], "true");
    assert_eq!(again.json::<Value>().await.unwrap(), saved);
    assert!(live.try_recv().is_err());
    f.app
        .store
        .transaction(|tx| {
            tx.execute("DELETE FROM idempotency_key", [])?;
            Ok(())
        })
        .unwrap();
    let recovered = f
        .put("policy", &content)
        .header("idempotency-key", "memory-write")
        .send()
        .await
        .unwrap();
    assert_eq!(recovered.status(), StatusCode::OK);
    assert_eq!(recovered.json::<Value>().await.unwrap(), saved);
    assert!(live.try_recv().is_err());
    f.app
        .store
        .transaction(|tx| {
            tx.execute("DELETE FROM idempotency_key", [])?;
            Ok(())
        })
        .unwrap();
    let conflict = f
        .put("policy", &text("policy", "different preference"))
        .header("idempotency-key", "memory-write")
        .send()
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let elsewhere = f
        .client
        .post(format!("{}/sessions", f.url))
        .basic_auth("cyber", Some("memory-test"))
        .header("idempotency-key", "memory-write")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(elsewhere.status(), StatusCode::CONFLICT);
    let note: Value = f
        .get("/memory/global/policy")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(note["data"]["body"], "durable preference");
    let removed = f
        .delete("policy")
        .header("idempotency-key", "memory-delete")
        .send()
        .await
        .unwrap();
    assert_eq!(removed.status(), StatusCode::OK);
    let deletion: Value = removed.json().await.unwrap();
    assert_eq!(deletion["data"]["receipt"]["deleted"], true);
    assert!(matches!(
        live.try_recv().unwrap(),
        LiveEvent::MemoryUpdated { seq: 2, .. }
    ));
    f.app
        .store
        .transaction(|tx| {
            tx.execute("DELETE FROM idempotency_key", [])?;
            Ok(())
        })
        .unwrap();
    let retry = f
        .delete("policy")
        .header("idempotency-key", "memory-delete")
        .send()
        .await
        .unwrap();
    assert_eq!(retry.status(), StatusCode::OK);
    assert_eq!(retry.json::<Value>().await.unwrap(), deletion);
    assert!(live.try_recv().is_err());
    assert_eq!(
        f.get("/memory/global/policy")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let store = f.store("global");
    assert_eq!(store.claim().unwrap().index().unwrap().text, "");
    assert_eq!(
        std::fs::read_dir(store.path().join(".memory-history"))
            .unwrap()
            .count(),
        2
    );
    f.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn busy_http_mutation_remains_retryable_with_the_same_key() {
    let f = Fixture::new(false).await;
    let store = f.store("global");
    let owner = store.claim().unwrap();
    let content = text("policy", "durable preference");
    assert_eq!(
        f.put("policy", &content)
            .header("idempotency-key", "busy")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    drop(owner);
    assert_eq!(
        f.put("policy", &content)
            .header("idempotency-key", "busy")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    f.close().await;
}
