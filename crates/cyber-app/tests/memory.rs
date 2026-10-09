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

#[tokio::test]
async fn recovery_auth_absent_and_malformed_requests_create_no_scope() {
    let f = Fixture::new(true).await;
    assert_eq!(
        f.client
            .get(format!("{}/memory/recovery/project", f.url))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = f.get("/memory/recovery/project").send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result: Value = response.json().await.unwrap();
    assert!(result["data"].is_null());
    assert_eq!(
        result["location"]["directory"],
        f.directory.to_str().unwrap()
    );
    for payload in [
        json!({}),
        json!({"storage_fingerprint":"bad","admission_fingerprint":"bad"}),
        json!({"storage_fingerprint":"a".repeat(64),"admission_fingerprint":format!("sha256:{}","b".repeat(64)),"unexpected":true}),
    ] {
        assert_eq!(
            f.client
                .post(format!("{}/memory/recovery/project", f.url))
                .basic_auth("cyber", Some("memory-test"))
                .json(&payload)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        f.get("/memory/recovery/invalid")
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
impl Fixture {
    fn prepare_recovery(&self, scope: &str) -> cyber_core::memory::MemoryStore {
        use cyber_server::runtime::{MemoryAdmission, MemoryWrite};
        let memory = self.store(scope);
        let project = memory.path().file_name().unwrap().to_str().unwrap();
        let content = text("policy", "Recovered preference");
        let MemoryAdmission::Owned(mut owner) = self
            .app
            .runtime
            .admit_memory_write(MemoryWrite {
                directory: &self.directory,
                project_id: project,
                name: "policy",
                deleted: false,
                identity: Some("http-recovery-fixture"),
                content: &content,
                http_hash: None,
            })
            .unwrap()
        else {
            panic!("owner")
        };
        let mut claim = memory.claim().unwrap();
        let prepared = claim.prepare_write(&content).unwrap();
        owner
            .bind_journal(prepared.journal_identity().unwrap())
            .unwrap();
        drop(prepared);
        drop(owner);
        drop(claim);
        memory
    }
    fn recover(&self, scope: &str, review: &Value) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}/memory/recovery/{scope}", self.url))
            .basic_auth("cyber", Some("memory-test"))
            .json(&json!({
                "storage_fingerprint":review["storage"]["fingerprint"],
                "admission_fingerprint":review["admission"]["fingerprint"],
            }))
    }
    async fn recovery(&self, scope: &str) -> Value {
        let response = self
            .get(&format!("/memory/recovery/{scope}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response.json::<Value>().await.unwrap()["data"].clone()
    }
}

#[cfg(unix)]
#[tokio::test]
async fn paired_recovery_acknowledges_once_and_cached_replay_preserves_receipt() {
    use cyber_server::runtime::LiveEvent;
    let f = Fixture::new(true).await;
    let memory = f.prepare_recovery("project");
    let review = f.recovery("project").await;
    assert_eq!(
        review["storage"]["proposed_note"]["body"],
        "Recovered preference"
    );
    assert_eq!(review["storage"]["journal"], review["admission"]["journal"]);
    assert!(!review.to_string().contains("mwo_"));
    assert!(!memory.path().join("policy.md").exists());
    let mut live = f.app.runtime.subscribe();
    let response = f
        .recover("project", &review)
        .header("idempotency-key", "reviewed-recovery")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let change: Value = response.json().await.unwrap();
    assert!(matches!(
        live.try_recv().unwrap(),
        LiveEvent::MemoryUpdated { seq: 3, .. }
    ));
    let replay = f
        .recover("project", &review)
        .header("idempotency-key", "reviewed-recovery")
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(replay.json::<Value>().await.unwrap(), change);
    assert!(live.try_recv().is_err());
    f.app
        .store
        .transaction(|tx| {
            tx.execute("DELETE FROM idempotency_key", [])?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        f.recover("project", &review)
            .header("idempotency-key", "reviewed-recovery")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert!(live.try_recv().is_err());
    assert!(f.recovery("project").await.is_null());
    assert_eq!(
        memory.claim().unwrap().read("policy").unwrap().body,
        "Recovered preference"
    );
    assert_eq!(
        f.app
            .store
            .read_events(review["admission"]["id"].as_str().unwrap(), -1, 20)
            .unwrap()
            .events
            .len(),
        4
    );
    f.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn recovery_rechecks_settings_and_preserves_stale_target_without_takeover() {
    let f = Fixture::new(false).await;
    let memory = f.prepare_recovery("global");
    let review = f.recovery("global").await;
    let config = f.app.paths.config.join("cyber.json");
    for settings in [
        json!({"memory":{"enabled":false}}),
        json!({"memory":{"generate":false}}),
    ] {
        std::fs::write(&config, settings.to_string()).unwrap();
        assert_eq!(f.recovery("global").await, review);
        assert_eq!(
            f.recover("global", &review).send().await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }
    std::fs::write(&config, "{}").unwrap();
    let mut stale = review.clone();
    stale["admission"]["fingerprint"] = json!(format!("sha256:{}", "0".repeat(64)));
    assert_eq!(
        f.recover("global", &stale).send().await.unwrap().status(),
        StatusCode::CONFLICT
    );
    let target = memory.path().join("policy.md");
    std::fs::write(&target, text("policy", "User edit")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        f.recover("global", &review).send().await.unwrap().status(),
        StatusCode::CONFLICT
    );
    assert!(
        std::fs::read_to_string(target)
            .unwrap()
            .contains("User edit")
    );
    assert!(
        memory
            .path()
            .join(".memory-transaction/note.after")
            .exists()
    );
    assert_eq!(
        f.app
            .store
            .aggregate_seq(review["admission"]["id"].as_str().unwrap())
            .unwrap(),
        Some(1)
    );
    f.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn recovery_busy_retry_and_closed_runtime_preserve_owned_fencing() {
    let f = Fixture::new(false).await;
    let memory = f.prepare_recovery("global");
    let review = f.recovery("global").await;
    let guard = memory.claim().unwrap();
    assert_eq!(
        f.recover("global", &review)
            .header("idempotency-key", "busy-recovery")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    drop(guard);
    assert_eq!(
        f.recover("global", &review)
            .header("idempotency-key", "busy-recovery")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    f.close().await;
    let closed = Fixture::new(false).await;
    let memory = closed.prepare_recovery("global");
    let review = closed.recovery("global").await;
    closed.app.runtime.shutdown().await;
    assert_eq!(
        closed
            .recover("global", &review)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(!memory.path().join("policy.md").exists());
    assert!(
        memory
            .path()
            .join(".memory-transaction/note.after")
            .exists()
    );
    closed.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn recovery_refuses_foreign_location_and_unbound_journal_without_releasing_evidence() {
    let f = Fixture::new(false).await;
    let memory = f.prepare_recovery("global");
    let review = f.recovery("global").await;
    let other = f.directory.join("other-location");
    std::fs::create_dir(&other).unwrap();
    assert_eq!(
        f.get("/memory/recovery/global")
            .header("x-cyber-directory", other.to_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.recover("global", &review)
            .header("x-cyber-directory", other.to_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.app
            .store
            .aggregate_seq(review["admission"]["id"].as_str().unwrap())
            .unwrap(),
        Some(1)
    );
    assert!(!memory.path().join("policy.md").exists());
    f.close().await;
    let local = Fixture::new(false).await;
    let memory = local.store("global");
    let mut owner = memory.claim().unwrap();
    drop(
        owner
            .prepare_write(&text("policy", "Unbound preference"))
            .unwrap(),
    );
    drop(owner);
    let response = local.get("/memory/recovery/global").send().await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(
        !response
            .text()
            .await
            .unwrap()
            .contains("Unbound preference")
    );
    assert!(
        memory
            .path()
            .join(".memory-transaction/note.after")
            .exists()
    );
    assert!(!memory.path().join("policy.md").exists());
    local.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn durable_recovery_lookup_and_global_key_conflicts_survive_cache_loss() {
    let f = Fixture::new(true).await;
    let _memory = f.prepare_recovery("project");
    let review = f.recovery("project").await;
    let key = "durable/recovery+key";
    let response = f
        .recover("project", &review)
        .header("idempotency-key", key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let change: Value = response.json().await.unwrap();
    f.app
        .store
        .transaction(|tx| {
            tx.execute("DELETE FROM idempotency_key", [])?;
            Ok(())
        })
        .unwrap();
    let lookup = f
        .get("/memory/recovery/requests")
        .query(&[("scope", "project"), ("key", key)])
        .send()
        .await
        .unwrap();
    assert_eq!(lookup.status(), StatusCode::OK);
    let status: Value = lookup.json().await.unwrap();
    assert_eq!(status["data"]["completed"], change["data"]);
    assert_eq!(status["data"]["mutation_id"], review["admission"]["id"]);
    let mut altered = review.clone();
    altered["storage"]["fingerprint"] = json!("f".repeat(64));
    assert_eq!(
        f.recover("project", &altered)
            .header("idempotency-key", key)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.put("policy", &text("policy", "Different fact"))
            .header("idempotency-key", key)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.get("/memory/recovery/requests")
            .query(&[("scope", "global"), ("key", key)])
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let absent = f
        .get("/memory/recovery/requests")
        .query(&[("scope", "project"), ("key", "absent")])
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert!(absent["data"].is_null());
    assert_eq!(
        f.get("/memory/recovery/requests")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.app
            .store
            .read_events(review["admission"]["id"].as_str().unwrap(), -1, 20)
            .unwrap()
            .events
            .len(),
        4
    );
    f.close().await;
}

#[cfg(unix)]
struct DelayedMemoryReply {
    inner: std::sync::Arc<dyn cyber_server::http::Services>,
    reached: std::sync::Arc<tokio::sync::Notify>,
    release: std::sync::Arc<tokio::sync::Notify>,
}
#[cfg(unix)]
impl cyber_server::http::Services for DelayedMemoryReply {
    fn models(
        &self,
        location: &std::path::Path,
    ) -> futures::future::BoxFuture<'_, Result<Vec<cyber_server::http::ModelInfo>, String>> {
        self.inner.models(location)
    }
    fn default_model(&self, location: &std::path::Path) -> Option<String> {
        self.inner.default_model(location)
    }
    fn agents(&self, location: &std::path::Path) -> Vec<cyber_server::http::AgentInfo> {
        self.inner.agents(location)
    }
    fn tools(
        &self,
        turn: &cyber_server::runtime::TurnContext,
    ) -> Vec<cyber_server::runtime::ToolDef> {
        self.inner.tools(turn)
    }
    fn commands(&self, location: &std::path::Path) -> Vec<cyber_server::http::CommandInfo> {
        self.inner.commands(location)
    }
    fn find_files(&self, location: &std::path::Path, query: &str, limit: usize) -> Vec<String> {
        self.inner.find_files(location, query, limit)
    }
    fn expand_command(
        &self,
        location: &std::path::Path,
        name: &str,
        arguments: &str,
    ) -> Option<String> {
        self.inner.expand_command(location, name, arguments)
    }
    fn memory_edit(
        &self,
        runtime: cyber_server::runtime::Runtime,
        edit: cyber_server::http::MemoryEdit,
    ) -> futures::future::BoxFuture<
        '_,
        Result<cyber_server::runtime::MemoryChange, cyber_server::http::ApiError>,
    > {
        Box::pin(async move {
            let result = self.inner.memory_edit(runtime, edit).await?;
            self.reached.notify_one();
            self.release.notified().await;
            Ok(result)
        })
    }
}

#[cfg(unix)]
#[tokio::test]
async fn acknowledged_memory_retry_is_not_blocked_by_a_delayed_first_response() {
    let f = Fixture::new(false).await;
    let reached = std::sync::Arc::new(tokio::sync::Notify::new());
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let mut state = f.app.state.clone();
    state.services = std::sync::Arc::new(DelayedMemoryReply {
        inner: state.services.clone(),
        reached: reached.clone(),
        release: release.clone(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/api/v1/memory/global/policy",
        listener.local_addr().unwrap()
    );
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(cyber_server::http::serve_tcp(
        cyber_server::http::router(state),
        listener,
        async {
            let _ = stopped.await;
        },
    ));
    let content = text("policy", "Delayed response");
    let key = "acknowledged-delayed-reply";
    let first = tokio::spawn(
        f.client
            .put(url)
            .basic_auth("cyber", Some("memory-test"))
            .header("idempotency-key", key)
            .json(&json!({"content":content}))
            .send(),
    );
    tokio::time::timeout(std::time::Duration::from_secs(10), reached.notified())
        .await
        .unwrap();
    let replay = f
        .put("policy", &content)
        .header("idempotency-key", key)
        .send()
        .await
        .unwrap();
    let status = replay.status();
    // Always release the real first response before asserting the regression outcome.
    release.notify_one();
    let first = first.await.unwrap().unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    let expected: Value = first.json().await.unwrap();
    let actual: Value = replay.json().await.unwrap();
    f.close().await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(actual, expected);
}

#[tokio::test]
async fn edit_review_routing_and_authentication_do_not_create_missing_storage() {
    let f = Fixture::new(true).await;
    assert_eq!(
        f.client
            .get(format!("{}/memory/edit/global/policy", f.url))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    for (path, expected) in [
        ("/memory/edit/global/policy", StatusCode::NOT_FOUND),
        ("/memory/edit/project/policy", StatusCode::NOT_FOUND),
        ("/memory/edit/invalid/policy", StatusCode::BAD_REQUEST),
        ("/memory/edit/global/bad.name", StatusCode::BAD_REQUEST),
    ] {
        assert_eq!(f.get(path).send().await.unwrap().status(), expected);
    }
    assert_eq!(
        std::fs::read_dir(f.app.paths.data.join("memory"))
            .unwrap()
            .count(),
        0
    );
    f.close().await;
}

#[cfg(unix)]
impl Fixture {
    fn reviewed_put(&self, name: &str, content: &str, review: &str) -> reqwest::RequestBuilder {
        self.client
            .put(format!("{}/memory/global/{name}", self.url))
            .basic_auth("cyber", Some("memory-test"))
            .json(&json!({"content": content, "review_fingerprint": review}))
    }
    async fn edit_review(&self, scope: &str, name: &str) -> Value {
        let response = self
            .get(&format!("/memory/edit/{scope}/{name}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response.json::<Value>().await.unwrap()["data"].clone()
    }
}

#[cfg(unix)]
#[tokio::test]
async fn conditional_edits_preserve_external_changes_without_durable_admission() {
    let f = Fixture::new(true).await;
    let store = f.store("global");
    for target in ["policy.md", "MEMORY.md", "other.md"] {
        {
            let mut scope = store.claim().unwrap();
            scope.write(&text("policy", "Original fact")).unwrap();
            scope.write(&text("other", "Other fact")).unwrap();
        }
        let review = f.edit_review("global", "policy").await;
        assert_eq!(review["original"], text("policy", "Original fact"));
        let fingerprint = review["fingerprint"].as_str().unwrap();
        let changed = if target == "MEMORY.md" {
            "External index change".into()
        } else {
            text(target.trim_end_matches(".md"), "External fact")
        };
        std::fs::write(store.path().join(target), &changed).unwrap();
        assert_eq!(
            f.reviewed_put("policy", &text("policy", "Draft"), fingerprint)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            f.delete("policy")
                .header("x-cyber-memory-review", fingerprint)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            std::fs::read_to_string(store.path().join(target)).unwrap(),
            changed
        );
        assert!(!store.path().join(".memory-transaction").exists());
    }
    assert_eq!(
        f.app
            .store
            .read(|db| db
                .query_row("SELECT COUNT(*) FROM memory_mutation", [], |row| row
                    .get::<_, i64>(0))
                .map_err(Into::into))
            .unwrap(),
        0
    );
    f.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn conditional_write_and_delete_replay_consumed_reviews_and_bind_header_identity() {
    let f = Fixture::new(true).await;
    let store = f.store("global");
    store
        .claim()
        .unwrap()
        .write(&text("policy", "Original fact"))
        .unwrap();
    let review = f.edit_review("global", "policy").await;
    let fingerprint = review["fingerprint"].as_str().unwrap();
    let content = text("policy", "Updated fact");
    let saved = f
        .reviewed_put("policy", &content, fingerprint)
        .header("idempotency-key", "conditional-write")
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::OK);
    let saved = saved.json::<Value>().await.unwrap();
    f.app
        .store
        .transaction(|tx| {
            tx.execute("DELETE FROM idempotency_key", [])?;
            Ok(())
        })
        .unwrap();
    let replay = f
        .reviewed_put("policy", &content, fingerprint)
        .header("idempotency-key", "conditional-write")
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(replay.json::<Value>().await.unwrap(), saved);
    assert_eq!(
        f.reviewed_put("policy", &content, &"a".repeat(64))
            .header("idempotency-key", "conditional-write")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    let review = f.edit_review("global", "policy").await;
    let fingerprint = review["fingerprint"].as_str().unwrap();
    let deleted = f
        .delete("policy")
        .header("x-cyber-memory-review", fingerprint)
        .header("idempotency-key", "conditional-delete")
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    let deleted = deleted.json::<Value>().await.unwrap();
    f.app
        .store
        .transaction(|tx| {
            tx.execute("DELETE FROM idempotency_key", [])?;
            Ok(())
        })
        .unwrap();
    let replay = f
        .delete("policy")
        .header("x-cyber-memory-review", fingerprint)
        .header("idempotency-key", "conditional-delete")
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(replay.json::<Value>().await.unwrap(), deleted);
    for review in [None, Some("a".repeat(64))] {
        let request = f
            .delete("policy")
            .header("idempotency-key", "conditional-delete");
        let request = match review {
            Some(value) => request.header("x-cyber-memory-review", value),
            None => request,
        };
        assert_eq!(request.send().await.unwrap().status(), StatusCode::CONFLICT);
    }
    assert_eq!(
        f.app
            .store
            .read(|db| db
                .query_row("SELECT COUNT(*) FROM memory_mutation", [], |row| row
                    .get::<_, i64>(0))
                .map_err(Into::into))
            .unwrap(),
        2
    );
    f.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn conditional_edit_validation_missing_scope_foreign_reviews_and_settings_are_fenced() {
    let f = Fixture::new(true).await;
    assert_eq!(
        f.reviewed_put("policy", &text("policy", "Fact"), &"a".repeat(64))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        std::fs::read_dir(f.app.paths.data.join("memory"))
            .unwrap()
            .count(),
        0
    );
    let store = f.store("global");
    store
        .claim()
        .unwrap()
        .write(&text("policy", "Original fact"))
        .unwrap();
    let project = f.store("project");
    project
        .claim()
        .unwrap()
        .write(&text("policy", "Original fact"))
        .unwrap();
    let foreign = f.edit_review("project", "policy").await;
    assert_eq!(
        f.reviewed_put(
            "policy",
            &text("policy", "Draft"),
            foreign["fingerprint"].as_str().unwrap()
        )
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::CONFLICT
    );
    for fingerprint in ["", "invalid", &"A".repeat(64)] {
        assert_eq!(
            f.reviewed_put("policy", &text("policy", "Draft"), fingerprint)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            f.delete("policy")
                .header("x-cyber-memory-review", fingerprint)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        f.delete("policy")
            .header("x-cyber-memory-review", "a".repeat(64))
            .header("x-cyber-memory-review", "b".repeat(64))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    std::fs::write(
        f.app.paths.config.join("cyber.json"),
        r#"{"memory":{"enabled":false}}"#,
    )
    .unwrap();
    let review = f.edit_review("global", "policy").await;
    let fingerprint = review["fingerprint"].as_str().unwrap();
    assert_eq!(
        f.reviewed_put("policy", &text("policy", "Draft"), fingerprint)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.delete("policy")
            .header("x-cyber-memory-review", fingerprint)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        store.claim().unwrap().read("policy").unwrap().body,
        "Original fact"
    );
    assert!(!store.path().join(".memory-transaction").exists());
    f.close().await;
}
